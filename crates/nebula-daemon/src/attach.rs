//! One client's attachment to a session (ATTACH): the replay-then-follow
//! binding behind `ClientRequest::Attach`, and the forward task that keeps
//! that client's pane current from then on — across broadcast lag (a ring
//! resync) and across a respawn of the session's PTY (a fresh binding to
//! the new one).
//!
//! The respawn case is what a restart, a WORKTREE RELOCATION
//! (`nebula worktree` → kill + `--resume` at the turn's end) and a cloud
//! re-entry all do: the registry swaps a new `PtySession` in under the same
//! `SessionRef`. A forward task bound to the old PTY would go quiet forever
//! — the client still believes it is attached, so it never asks again — and
//! the pane looks frozen until the user clicks away and back. So the task
//! listens for `Daemon::session_installs` and, when its ref gets a new PTY,
//! rebinds: the client sees a `Scrollback` (its parser resets, an `exited`
//! mark clears) and then the new process's output, on the attachment it
//! already holds.

use crate::pty::{PtyEvent, PtySession};
use crate::registry::Daemon;
use nebula_core::{ServerEvent, SessionRef};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc};

/// The grid the client asked for at Attach; a rebind sizes the new PTY to
/// it the same way the first binding did.
#[derive(Debug, Clone, Copy)]
pub struct PaneSize {
    pub cols: u16,
    pub rows: u16,
}

/// The forward task's hold on one PTY: its event stream and how far the
/// client has been brought up to date.
struct Bound {
    session: Arc<PtySession>,
    rx: broadcast::Receiver<PtyEvent>,
    /// First byte offset the client has NOT seen; frames ending at or before
    /// it are already covered by a replay.
    min_seq: u64,
}

/// Replay `session`'s ring to the client and hand back the event receiver
/// plus the seq the replay ended at, so the forward loop can skip what it
/// covered. Subscribes BEFORE snapshotting so nothing falls in the gap.
pub async fn bind(
    session: &Arc<PtySession>,
    sref: &SessionRef,
    out_tx: &mpsc::Sender<ServerEvent>,
    size: PaneSize,
    from_seq: Option<u64>,
) -> (broadcast::Receiver<PtyEvent>, u64) {
    let rx = session.events.subscribe();
    let (base_seq, data) = session.snapshot(from_seq);
    let replay_end = base_seq + data.len() as u64;
    let _ = out_tx
        .send(ServerEvent::Scrollback {
            session: sref.clone(),
            base_seq,
            data,
        })
        .await;
    let _ = out_tx
        .send(ServerEvent::KittyFlags {
            session: sref.clone(),
            flags: session.kitty_flags(),
        })
        .await;
    let _ = session.resize_with_jiggle(size.cols, size.rows);
    (rx, replay_end)
}

/// Forward live PTY output/exit to one client, skipping bytes the attach
/// replay already delivered, for as long as the client holds the
/// attachment. Outlives the PTY it started on: once that one exits the task
/// parks (the client has its `SessionExited`), and a session the registry
/// installs under the same ref later — a respawn, or another client's
/// Attach booting it — is bound in its place.
pub async fn forward(
    daemon: Arc<Daemon>,
    session: Arc<PtySession>,
    sref: SessionRef,
    rx: broadcast::Receiver<PtyEvent>,
    out_tx: mpsc::Sender<ServerEvent>,
    min_seq: u64,
    size: PaneSize,
) {
    let mut installs = daemon.session_installs.subscribe();
    // `None` once the PTY has exited: nothing more comes from it, but the
    // task stays to pick up a replacement.
    let mut live = Some(Bound {
        session,
        rx,
        min_seq,
    });
    loop {
        tokio::select! {
            ev = async { live.as_mut().expect("bound").rx.recv().await }, if live.is_some() => {
                match step(live.as_mut().expect("bound"), &sref, &out_tx, ev).await {
                    Step::Continue => {}
                    Step::Exited => live = None,
                    Step::ClientGone => return,
                }
            }
            installed = installs.recv() => {
                match installed {
                    Ok(installed) if installed != sref => continue,
                    // Our ref — or a lagged receiver, which may have missed
                    // it: either way, compare against what the registry holds.
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
                let Some(current) = daemon.session(&sref) else {
                    continue;
                };
                if live
                    .as_ref()
                    .is_some_and(|b| Arc::ptr_eq(&b.session, &current))
                {
                    continue;
                }
                // The old receiver goes with the old binding: a late Exited
                // from the PTY being replaced must not mark the new one dead.
                let (rx, min_seq) = bind(&current, &sref, &out_tx, size, None).await;
                live = Some(Bound {
                    session: current,
                    rx,
                    min_seq,
                });
            }
        }
    }
}

enum Step {
    Continue,
    /// The PTY is gone; keep the task for a possible replacement.
    Exited,
    /// The connection's writer is gone; nothing left to forward to.
    ClientGone,
}

/// Apply one PTY event to the client.
async fn step(
    bound: &mut Bound,
    sref: &SessionRef,
    out_tx: &mpsc::Sender<ServerEvent>,
    ev: Result<PtyEvent, RecvError>,
) -> Step {
    match ev {
        Ok(PtyEvent::Output { seq, data }) => {
            let end = seq + data.len() as u64;
            if end <= bound.min_seq {
                return Step::Continue; // fully covered by the replay
            }
            let skip = bound.min_seq.saturating_sub(seq) as usize;
            let payload = if skip > 0 {
                data[skip..].to_vec()
            } else {
                data
            };
            let send_seq = seq + skip as u64;
            bound.min_seq = end;
            deliver(
                out_tx,
                ServerEvent::Output {
                    session: sref.clone(),
                    seq: send_seq,
                    data: payload,
                },
            )
            .await
        }
        Ok(PtyEvent::Exited { exit_code }) => {
            let _ = out_tx
                .send(ServerEvent::SessionExited {
                    session: sref.clone(),
                    exit_code,
                })
                .await;
            Step::Exited
        }
        Ok(PtyEvent::KittyFlags { flags }) => {
            deliver(
                out_tx,
                ServerEvent::KittyFlags {
                    session: sref.clone(),
                    flags,
                },
            )
            .await
        }
        // Daemon-side only: the progress edge drives the status machine
        // and reaches clients as a StatusChanged, not as session output;
        // the cloud sightings reach them as the row's own upsert.
        Ok(
            PtyEvent::Progress { .. }
            | PtyEvent::CloudSession { .. }
            | PtyEvent::CloudAttachRejected,
        ) => Step::Continue,
        Err(RecvError::Lagged(_)) => {
            // Catch up from the ring. If the missed bytes are still
            // retained, send them as a plain Output continuation so the
            // client keeps its parser state; only when the gap has fallen
            // off the ring do we force a full replay (parser reset —
            // expensive on the client, so avoid it when possible).
            let wanted = bound.min_seq;
            let (base_seq, data) = bound.session.snapshot(Some(wanted));
            bound.min_seq = base_seq + data.len() as u64;
            let ev = if base_seq == wanted {
                ServerEvent::Output {
                    session: sref.clone(),
                    seq: base_seq,
                    data,
                }
            } else {
                ServerEvent::Scrollback {
                    session: sref.clone(),
                    base_seq,
                    data,
                }
            };
            deliver(out_tx, ev).await
        }
        // The PTY's event sender is gone (the session was dropped): as dead
        // as an Exited, only without the frame — the exit already went out
        // or never happened.
        Err(RecvError::Closed) => Step::Exited,
    }
}

async fn deliver(out_tx: &mpsc::Sender<ServerEvent>, ev: ServerEvent) -> Step {
    if out_tx.send(ev).await.is_err() {
        Step::ClientGone
    } else {
        Step::Continue
    }
}
