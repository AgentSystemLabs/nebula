pub mod cloud;
pub mod cursor;
pub mod kitty;
pub mod progress;
pub mod ring;
pub mod title;

use anyhow::{Context, Result};
use cloud::CloudScanner;
use cursor::CursorTracker;
use nebula_core::SessionRef;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use progress::ProgressScanner;
use ring::ScrollbackRing;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

const RING_CAPACITY: usize = 1024 * 1024;
/// Flush coalesced output at this size…
const COALESCE_BYTES: usize = 8 * 1024;
/// …or this long after the previous flush, whichever comes first. A hard
/// deadline (not a quiet-gap timer): a child streaming continuously in small
/// chunks must still flush on time, or output arrives in laggy 8KB lumps.
/// Counted from the last flush rather than from the first pending byte, so
/// output that breaks a silence — the echo of a typed character, a prompt
/// redrawn after Enter — is not held at all (`flush_deadline`).
const COALESCE_HOLD: std::time::Duration = std::time::Duration::from_millis(5);
/// Reader thread → pump channel bound; blocking_send gives natural
/// backpressure against a fire-hosing child.
const READER_CHANNEL_BOUND: usize = 64;
/// After the polite SIGHUP, how long the child gets to exit before its whole
/// process group is SIGKILLed.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
/// Size a session is spawned at when no client is attached to say better
/// (prewarms, respawns after a move, restarts). The first attach resizes it
/// to the real pane, so these only shape the child's first paint.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
/// The two bytes every output scanner in this module keys on: ESC opens a
/// CSI/OSC sequence, BEL is the classic OSC terminator.
pub(crate) const ESC: u8 = 0x1b;
pub(crate) const BEL: u8 = 0x07;

/// A `PtySize` in cells only. Nothing here knows pixel dimensions, and
/// leaving them zero is what every caller wants.
fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The live process table, one process per line, as `ps -axo
/// pid=,ppid=,pgid=,stat=` prints it; None when `ps` itself fails.
fn ps_table() -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat="])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()
}

/// One process as the `ps` sweep reports it.
struct ProcRow {
    pid: u32,
    ppid: u32,
    pgid: u32,
    /// The `s` flag of `stat`: the process leads a session of its own, so
    /// something called `setsid` to cut it loose from its terminal.
    session_leader: bool,
}

/// Parse [`ps_table`] output. A row without a `stat` column parses too (as
/// no session leader), so a bare `pid ppid pgid` table works.
fn parse_ps_table(table: &str) -> Vec<ProcRow> {
    table
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let pid = cols.next()?.parse().ok()?;
            let ppid = cols.next()?.parse().ok()?;
            let pgid = cols.next()?.parse().ok()?;
            let session_leader = cols.next().is_some_and(|stat| stat.contains('s'));
            Some(ProcRow {
                pid,
                ppid,
                pgid,
                session_leader,
            })
        })
        .collect()
}

/// `root`'s subtree — children, grandchildren, and on down — without
/// `root` itself.
fn descendants(rows: &[ProcRow], root: u32) -> Vec<&ProcRow> {
    let mut children: HashMap<u32, Vec<&ProcRow>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut found = Vec::new();
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(kids) = children.remove(&pid) {
            stack.extend(kids.iter().map(|kid| kid.pid));
            found.extend(kids);
        }
    }
    found
}

/// Every process group with a member in `root`'s subtree, `root`'s own
/// first (it leads its PTY session, so that group is its pid, and it is
/// named even when the `ps` sweep fails). An interactive shell running the
/// agent as a job puts it in a group of its own; SIGKILLing the leader's
/// group alone would miss it. Taken while the tree is intact — see `kill`.
fn process_groups_under(root: u32) -> Vec<u32> {
    process_groups_in_table(&ps_table().unwrap_or_default(), root)
}

/// Pure core of [`process_groups_under`] over a [`ps_table`].
fn process_groups_in_table(table: &str, root: u32) -> Vec<u32> {
    let rows = parse_ps_table(table);
    let mut groups = vec![root];
    let own = rows.iter().filter(|row| row.pid == root);
    for row in own.chain(descendants(&rows, root)) {
        if !groups.contains(&row.pgid) {
            groups.push(row.pgid);
        }
    }
    groups
}

/// Is a job the agent cut loose from its terminal still running under
/// `root` — a descendant leading a session of its own? That is how Claude
/// Code runs a backgrounded Bash call or a Monitor watch and how Codex runs
/// a shell command: work that outlives the turn which started it, and that
/// the hook-fed status machine therefore no longer sees. The helpers an
/// agent keeps inside its own session — MCP servers, `caffeinate`, Codex's
/// code-mode host — are not counted, so an idle agent still reads as idle.
/// A failed `ps` counts as busy: never kill what can't be inspected.
pub(crate) fn detached_job_under(root: u32) -> bool {
    match ps_table() {
        Some(table) => detached_job_in_table(&table, root),
        None => true,
    }
}

/// Pure core of [`detached_job_under`] over a [`ps_table`].
fn detached_job_in_table(table: &str, root: u32) -> bool {
    let rows = parse_ps_table(table);
    descendants(&rows, root)
        .iter()
        .any(|row| row.session_leader)
}

/// Broadcast to attached clients (and, later, the status machine).
#[derive(Clone, Debug)]
pub enum PtyEvent {
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    Exited {
        exit_code: Option<i32>,
    },
    /// The child pushed/popped kitty keyboard flags; clients re-encode keys.
    KittyFlags {
        flags: u8,
    },
    /// The child's OSC 9;4 progress state flipped. For agent CLIs this is a
    /// busy/idle edge the status machine trusts — notably it is the *only*
    /// end-of-turn signal after the user cancels, which fires no hook.
    Progress {
        busy: bool,
    },
    /// The child set its window title (OSC 0/2). Claude Code's carries the
    /// session's name (`✳ Fix Login Redirect`), and `/rename` — which
    /// fires no hook — shows up here first (see `pty::title`).
    Title {
        title: String,
    },
    /// The child printed the id of the Claude Cloud session it created.
    /// Only scanned for on `--cloud` launches (`arm_cloud_scan`).
    CloudSession {
        id: String,
    },
}

enum ReaderMsg {
    Data(Vec<u8>),
    Eof { exit_code: Option<i32> },
}

pub struct PtySession {
    pub sref: SessionRef,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Child pid: drives the SIGHUP → SIGKILL escalation (the child is its
    /// PTY session's leader — portable-pty does setsid — so pgid == pid,
    /// though an agent run as a job under a login shell sits in a group of
    /// its own below it) and the metrics modal's process-tree sums.
    pub child_pid: Option<u32>,
    pub ring: Mutex<ScrollbackRing>,
    pub events: broadcast::Sender<PtyEvent>,
    /// Last applied size, for the attach-time SIGWINCH jiggle.
    last_size: Mutex<(u16, u16)>,
    /// Kitty keyboard negotiation state, fed by the pump from live output.
    kitty: Mutex<kitty::KittyScanner>,
    /// OSC 9;4 busy/idle tracking, likewise fed from live output.
    progress: Mutex<ProgressScanner>,
    /// OSC 0/2 window-title tracking, likewise fed from live output.
    title: Mutex<title::TitleScanner>,
    /// Claude Cloud session id scanner; `None` until a `--cloud` launch
    /// arms it, so ordinary sessions pay nothing.
    cloud: Mutex<Option<CloudScanner>>,
    /// Screen for answering cursor position reports; `None` until the child
    /// first asks, so sessions that never do parse nothing.
    cursor: Mutex<Option<CursorTracker>>,
}

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    /// Extra env vars (NEBULA_* for agents). Plain terminals get none.
    pub env: Vec<(String, String)>,
    /// Env var names to scrub from the inherited environment. Only ever a
    /// fixed list (the agent-session vars), so it is borrowed, not built.
    pub scrub_env: &'static [&'static str],
    pub cols: u16,
    pub rows: u16,
}

impl PtySession {
    /// Spawn the child in a fresh PTY and start its reader thread + pump task.
    pub fn spawn(sref: SessionRef, spec: SpawnSpec) -> Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(spec.cols, spec.rows))
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        // The child paints nebula's grid, not the terminal the daemon was
        // started from: name that grid and drop the colour overrides the
        // daemon inherited. Agent launches restate all three after the
        // login shell's profile too (`login_shell_wrap`).
        cmd.env("TERM", nebula_core::env::PANE_TERM);
        cmd.env("COLORTERM", nebula_core::env::PANE_COLORTERM);
        for name in nebula_core::env::PANE_COLOR_OVERRIDES {
            cmd.env_remove(name);
        }
        for name in spec.scrub_env {
            cmd.env_remove(name);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn child in pty")?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let child_pid = child.process_id();
        let reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;

        let (events, _) = broadcast::channel(256);
        let session = Arc::new(Self {
            sref,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            child_pid,
            ring: Mutex::new(ScrollbackRing::new(RING_CAPACITY)),
            events,
            last_size: Mutex::new((spec.cols, spec.rows)),
            kitty: Mutex::new(kitty::KittyScanner::new()),
            progress: Mutex::new(ProgressScanner::new()),
            title: Mutex::new(title::TitleScanner::new()),
            cloud: Mutex::new(None),
            cursor: Mutex::new(None),
        });

        let (tx, rx) = mpsc::channel::<ReaderMsg>(READER_CHANNEL_BOUND);
        spawn_reader_thread(reader, child, tx);
        tokio::spawn(pump(session.clone(), rx));
        Ok(session)
    }

    pub fn write_input(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self.master.lock().unwrap();
        master.resize(pty_size(cols, rows))?;
        *self.last_size.lock().unwrap() = (cols, rows);
        if let Some(cursor) = self.cursor.lock().unwrap().as_mut() {
            cursor.resize(cols, rows);
        }
        Ok(())
    }

    /// Resize on attach. The kernel only delivers SIGWINCH on a *change*, so
    /// when the requested size equals the current one, jiggle (rows-1 then
    /// back) to force a full-screen repaint — the dtach trick.
    pub fn resize_with_jiggle(&self, cols: u16, rows: u16) -> Result<()> {
        let same = { *self.last_size.lock().unwrap() == (cols, rows) };
        if same && rows > 1 {
            let master = self.master.lock().unwrap();
            master.resize(pty_size(cols, rows - 1))?;
            master.resize(pty_size(cols, rows))?;
            Ok(())
        } else {
            self.resize(cols, rows)
        }
    }

    /// SIGHUP the child, then SIGKILL every process group under it if it
    /// hasn't exited within [`KILL_GRACE`]. The sweep also reaps grandchildren
    /// that would otherwise hold the slave fd open (no EOF → reader thread,
    /// pump task, and the 1MB ring all pinned forever) — including an agent
    /// the login shell forked into a job group of its own, which a kill of
    /// the leader's group alone would leave running with its pane gone.
    pub fn kill(&self) {
        // Subscribe before signalling so an immediate exit can't be missed.
        let mut rx = self.events.subscribe();
        // Sweep before the polite signal: a shell that dies of it leaves the
        // job it was running to init, where no walk from `pid` would find
        // it afterwards. ~10ms, and a kill is rare.
        let mut groups = self.child_pid.map(process_groups_under).unwrap_or_default();
        let _ = self.killer.lock().unwrap().kill();
        let Some(pid) = self.child_pid else { return };
        let sref = self.sref.clone();
        // Watchdog on a plain thread: it must not hold the session Arc (that
        // would pin the ring), and it outlives any tokio context `kill` was
        // called from.
        std::thread::Builder::new()
            .name("pty-kill-watchdog".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                let deadline = std::time::Instant::now() + KILL_GRACE;
                let nix_pid = Pid::from_raw(pid as i32);
                let mut child_gone = false;
                while std::time::Instant::now() < deadline {
                    loop {
                        use tokio::sync::broadcast::error::TryRecvError;
                        match rx.try_recv() {
                            Ok(PtyEvent::Exited { .. }) | Err(TryRecvError::Closed) => {
                                child_gone = true;
                                break;
                            }
                            Ok(_) | Err(TryRecvError::Lagged(_)) => continue,
                            Err(TryRecvError::Empty) => break,
                        }
                    }
                    // Reaped (ESRCH) strictly precedes the Exited broadcast,
                    // so this also covers an Exited lost to channel lag.
                    if child_gone || nix::sys::signal::kill(nix_pid, None).is_err() {
                        child_gone = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if child_gone {
                    // A shell that exited on its own hung up its jobs first;
                    // one the signal took outright left them to init (and on
                    // macOS the leader's death revokes the slave, so the
                    // session reads as exited with the job still running).
                    // Any group of the sweep still standing is the latter:
                    // it gets the hangup the shell owed it, and its own
                    // grace period.
                    let orphans: Vec<Pid> = groups
                        .iter()
                        .filter(|&&g| g != pid)
                        .map(|&g| Pid::from_raw(g as i32))
                        .filter(|&g| killpg(g, None).is_ok())
                        .collect();
                    if orphans.is_empty() {
                        return;
                    }
                    for g in &orphans {
                        let _ = killpg(*g, Signal::SIGHUP);
                    }
                    let deadline = std::time::Instant::now() + KILL_GRACE;
                    while std::time::Instant::now() < deadline
                        && orphans.iter().any(|&g| killpg(g, None).is_ok())
                    {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if orphans.iter().all(|&g| killpg(g, None).is_err()) {
                        return;
                    }
                }
                // Whatever appeared since the first sweep, then the lot.
                for pgid in process_groups_under(pid) {
                    if !groups.contains(&pgid) {
                        groups.push(pgid);
                    }
                }
                tracing::warn!(session = ?sref, pid, ?groups, "still running after SIGHUP — SIGKILLing its process groups");
                for pgid in groups {
                    let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGKILL);
                }
            })
            .expect("spawn pty kill watchdog");
    }

    /// Ring snapshot for attach replay: (base_seq, bytes).
    pub fn snapshot(&self, from_seq: Option<u64>) -> (u64, Vec<u8>) {
        self.ring.lock().unwrap().snapshot_from(from_seq)
    }

    /// The child's current kitty keyboard flags (0 = legacy).
    pub fn kitty_flags(&self) -> u8 {
        self.kitty.lock().unwrap().flags()
    }

    /// The child's advertised OSC 9;4 busy state, or `None` if it never
    /// advertised one (a CLI without a progress bar, or one not started yet).
    pub fn progress_busy(&self) -> Option<bool> {
        self.progress.lock().unwrap().busy()
    }

    /// The child's current window title, or `None` if it never set one.
    pub fn window_title(&self) -> Option<String> {
        self.title.lock().unwrap().title().map(str::to_string)
    }

    /// Start watching this child's output for the Claude Cloud session id
    /// it prints on creation (see `pty::cloud`). Output that already landed
    /// in the ring is scanned first, so arming a moment after spawn cannot
    /// miss a fast-printing child; the sighting then arrives as
    /// `PtyEvent::CloudSession`.
    pub fn arm_cloud_scan(&self) {
        let mut scanner = CloudScanner::new();
        let (_, replay) = self.snapshot(None);
        let sightings = scanner.feed(&replay);
        *self.cloud.lock().unwrap() = Some(scanner);
        for sighting in sightings {
            let _ = self.events.send(sighting.into());
        }
    }

    /// The bytes owed to the child for one chunk's queries, in query order.
    /// A cursor report reads the screen as it stood just past its query, so
    /// the tracker takes `chunk` up to each one in turn, then the rest: once
    /// built — from the ring, which does not hold `chunk` yet, at the PTY's
    /// size — it follows every byte.
    fn query_replies(&self, replies: Vec<kitty::Reply>, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cursor = self.cursor.lock().unwrap();
        let mut fed = 0;
        for reply in replies {
            match reply {
                kitty::Reply::Bytes(bytes) => out.extend_from_slice(&bytes),
                kitty::Reply::CursorPosition { at } => {
                    let tracker = cursor.get_or_insert_with(|| {
                        let (cols, rows) = *self.last_size.lock().unwrap();
                        let (_, history) = self.snapshot(None);
                        CursorTracker::new(cols, rows, &history)
                    });
                    tracker.feed(&chunk[fed..at]);
                    fed = at;
                    out.extend_from_slice(&tracker.report());
                }
            }
        }
        if let Some(tracker) = cursor.as_mut() {
            tracker.feed(&chunk[fed..]);
        }
        out
    }
}

/// PTY reads are blocking → dedicated thread per session. After EOF it reaps
/// the child to get the exit code.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    tx: mpsc::Sender<ReaderMsg>,
) {
    std::thread::Builder::new()
        .name("pty-reader".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx
                            .blocking_send(ReaderMsg::Data(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let exit_code = child.wait().ok().map(|st| st.exit_code() as i32);
            let _ = tx.blocking_send(ReaderMsg::Eof { exit_code });
        })
        .expect("spawn pty reader thread");
}

/// When output that arrived at `now` has to be on its way to the clients.
/// The hold exists to turn a stream of small writes into fewer, larger
/// events — so it is spent only while there is a stream: a flush less than
/// [`COALESCE_HOLD`] ago means more is likely right behind, and the bytes
/// wait out the rest of that window. After a quiet spell they go at once.
/// That is every keystroke's echo: held, it reached the TUI 5 ms late on
/// every character typed into a pane, which the INPUT LATENCY PROBE put at
/// half of what the key took to show. A stream still flushes at most once
/// per hold, exactly as before; the one extra event is at its head.
fn flush_deadline(
    now: tokio::time::Instant,
    last_flush: Option<tokio::time::Instant>,
) -> tokio::time::Instant {
    match last_flush {
        Some(last) if now < last + COALESCE_HOLD => last + COALESCE_HOLD,
        _ => now,
    }
}

/// Drains the reader channel: append to the ring (always — detach is free),
/// coalesce bursts, broadcast to whoever is attached.
async fn pump(session: Arc<PtySession>, mut rx: mpsc::Receiver<ReaderMsg>) {
    let mut pending: Vec<u8> = Vec::new();

    let flush = |session: &PtySession, pending: &mut Vec<u8>| {
        if pending.is_empty() {
            return;
        }
        // Terminal queries (kitty keyboard, DA1, DSR) ride in the output
        // stream; nothing else would ever answer them (tmux does the same).
        let actions = session.kitty.lock().unwrap().feed(pending);
        let reply = session.query_replies(actions.replies, pending);
        if !reply.is_empty() {
            if let Err(e) = session.write_input(&reply) {
                tracing::warn!(error = %e, "terminal query reply write failed");
            }
        }
        let busy_edge = session.progress.lock().unwrap().feed(pending);
        let title_change = session.title.lock().unwrap().feed(pending);
        let cloud_sightings = match session.cloud.lock().unwrap().as_mut() {
            Some(scanner) => scanner.feed(pending),
            None => Vec::new(),
        };
        let seq = session.ring.lock().unwrap().append(pending);
        let _ = session.events.send(PtyEvent::Output {
            seq,
            data: std::mem::take(pending),
        });
        if let Some(flags) = actions.flags_changed {
            tracing::debug!(session = ?session.sref, flags, "child kitty flags changed");
            let _ = session.events.send(PtyEvent::KittyFlags { flags });
        }
        if let Some(busy) = busy_edge {
            tracing::debug!(session = ?session.sref, busy, "child progress state changed");
            let _ = session.events.send(PtyEvent::Progress { busy });
        }
        if let Some(title) = title_change {
            tracing::debug!(session = ?session.sref, %title, "child window title changed");
            let _ = session.events.send(PtyEvent::Title { title });
        }
        for sighting in cloud_sightings {
            tracing::info!(session = ?session.sref, ?sighting, "cloud sighting in child output");
            let _ = session.events.send(sighting.into());
        }
    };

    let mut last_flush: Option<tokio::time::Instant> = None;
    'outer: loop {
        if pending.is_empty() {
            match rx.recv().await {
                Some(ReaderMsg::Data(d)) => pending.extend_from_slice(&d),
                Some(ReaderMsg::Eof { exit_code }) => {
                    let _ = session.events.send(PtyEvent::Exited { exit_code });
                    break;
                }
                None => break,
            }
        }
        // Coalesce until the deadline or the size cap; the deadline is fixed
        // when the first pending byte arrives so continuous streams still
        // flush on time. Biased toward the channel: a deadline that has
        // already passed (the quiet-spell case) still takes along whatever
        // the reader has queued, so one write read in two pieces is one event.
        let deadline = flush_deadline(tokio::time::Instant::now(), last_flush);
        while pending.len() < COALESCE_BYTES {
            tokio::select! {
                biased;
                msg = rx.recv() => match msg {
                    Some(ReaderMsg::Data(d)) => pending.extend_from_slice(&d),
                    Some(ReaderMsg::Eof { exit_code }) => {
                        flush(&session, &mut pending);
                        let _ = session.events.send(PtyEvent::Exited { exit_code });
                        break 'outer;
                    }
                    None => {
                        flush(&session, &mut pending);
                        break 'outer;
                    }
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        flush(&session, &mut pending);
        last_flush = Some(tokio::time::Instant::now());
    }
    tracing::info!(session = ?session.sref, "pty pump ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_core::AgentId;

    /// Output that breaks a silence is not held: a typed character's echo
    /// leaves the DAEMON the moment it is read.
    #[test]
    fn output_after_a_quiet_spell_is_flushed_at_once() {
        let now = tokio::time::Instant::now();
        assert_eq!(flush_deadline(now, None), now, "the session's first bytes");
        let long_ago = now - COALESCE_HOLD * 10;
        assert_eq!(flush_deadline(now, Some(long_ago)), now);
        assert_eq!(flush_deadline(now, Some(now - COALESCE_HOLD)), now);
    }

    /// A stream is still coalesced: bytes arriving inside the hold of the
    /// last flush wait for that hold to end, so the event rate under
    /// sustained output is what it was — one flush per hold at most.
    #[test]
    fn output_inside_the_hold_waits_for_it_to_end() {
        let now = tokio::time::Instant::now();
        let just_flushed = now - std::time::Duration::from_millis(1);
        assert_eq!(
            flush_deadline(now, Some(just_flushed)),
            just_flushed + COALESCE_HOLD
        );
    }

    fn echo_session() -> Arc<PtySession> {
        PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/cat".into(),
                args: vec![],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap()
    }

    /// A window title set by the child reaches subscribers as its own
    /// event, and the session remembers it.
    #[tokio::test]
    async fn window_title_is_read_off_the_output() {
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '\\033]0;✳ Fix Login\\007'; sleep 2".into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        let mut rx = session.events.subscribe();
        let title = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Title { title }) => break title,
                    Ok(PtyEvent::Exited { .. }) => panic!("child exited before setting a title"),
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("title event within 10s");
        assert_eq!(title, "✳ Fix Login");
        assert_eq!(session.window_title().as_deref(), Some("✳ Fix Login"));
        session.kill();
    }

    /// The pane is nebula's grid, not the terminal the daemon was started
    /// from: every child hears `TERM`/`COLORTERM` for that grid, and a
    /// `NO_COLOR` or `FORCE_COLOR` the daemon inherited never reaches it.
    #[tokio::test]
    async fn child_is_told_the_pane_is_a_truecolor_terminal() {
        // The daemon's own environment is the base the child is built from,
        // so the overrides have to sit there for the scrub to be exercised.
        // Only this test sets them, and only until the spawn has read them;
        // whatever was there before is put back.
        let before: Vec<(&str, Option<std::ffi::OsString>)> = ["NO_COLOR", "FORCE_COLOR", "TERM"]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("FORCE_COLOR", "0");
        std::env::set_var("TERM", "foot");
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    r#"printf 'env=%s|%s|%s|%s' "$TERM" "$COLORTERM" "${NO_COLOR-unset}" "${FORCE_COLOR-unset}""#
                        .into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        for (name, value) in before {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let mut rx = session.events.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("child exits within 10s");
        let (_, bytes) = session.snapshot(None);
        let out = String::from_utf8_lossy(&bytes);
        assert!(
            out.contains("env=xterm-256color|truecolor|unset|unset"),
            "child saw: {out:?}"
        );
    }

    /// A child asking where its cursor is — crossterm's `cursor::position()`,
    /// which timed out inside nebula before (#66) — is answered from the
    /// screen as it stood at the query, not after the rest of the chunk.
    #[tokio::test]
    async fn cursor_position_query_is_answered_from_the_screen() {
        let session = PtySession::spawn(
            SessionRef::Agent(AgentId::generate()),
            SpawnSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    // `min 6`: the read waits for the whole six-byte reply.
                    "stty -icanon -echo min 6 time 50; \
                     printf 'hi\\033[3;7H\\033[6nbye'; \
                     printf 'REPLY:'; dd bs=6 count=1 2>/dev/null | tr '\\033' E"
                        .into(),
                ],
                cwd: std::env::temp_dir(),
                env: vec![],
                scrub_env: &[],
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
        )
        .unwrap();
        let mut rx = session.events.subscribe();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { .. }) => break,
                    Ok(_) => continue,
                    Err(e) => panic!("event stream ended: {e}"),
                }
            }
        })
        .await
        .expect("child reads its reply and exits within 10s");
        let (_, bytes) = session.snapshot(None);
        let out = String::from_utf8_lossy(&bytes);
        assert!(out.contains("REPLY:E[3;7R"), "child saw: {out:?}");
    }

    /// The kill escalation must reach an agent the login shell forked into
    /// a job group of its own, not just the PTY leader's group.
    #[test]
    fn process_groups_cover_a_job_the_shell_forked_into_its_own_group() {
        // shell 20 leads its session (group 20); the agent 21 is a job in
        // group 21 with a worker 22; 30 is another session, 99 unrelated.
        let table = "\
 20    10    20
 21    20    21
 22    21    21
 30    10    30
 99     1    99
";
        assert_eq!(process_groups_in_table(table, 20), vec![20, 21]);
        // A failed sweep still names the leader's own group.
        assert_eq!(process_groups_in_table("", 20), vec![20]);
    }

    /// The reaper's "still working?" question is whether anything under the
    /// agent leads a session of its own: a backgrounded Bash call does
    /// (Claude spawns it detached), an MCP server or a shell job does not.
    #[test]
    fn detached_job_is_a_session_leader_below_the_agent() {
        // Agent 20 leads its PTY session and is its foreground job (`+`).
        // Its MCP server 21 shares that group; 22 is a helper in a group of
        // its own but the same session (Codex's code-mode host). 30 is
        // another session entirely, 99 unrelated.
        let idle = "\
 20    10    20  Ss+
 21    20    20  S+
 22    20    22  S
 30    10    30  Ss+
 99     1    99  S
";
        assert!(!detached_job_in_table(idle, 20));
        // A backgrounded Bash call: shell 23 started its own session (`s`)
        // and runs sleep 24 inside it.
        let busy = format!("{idle} 23    20    23  Ss\n 24    23    23  S\n");
        assert!(detached_job_in_table(&busy, 20));
        // Under a login shell that forked the agent as a job: 20 is bash,
        // 21 the agent in a group of its own but not a session of its own,
        // 25 its detached Bash call.
        let wrapped = "\
 20    10    20  Ss
 21    20    21  S+
 25    21    25  Ss
";
        assert!(detached_job_in_table(wrapped, 20));
        assert!(!detached_job_in_table(
            " 20    10    20  Ss\n 21    20    21  S+\n",
            20
        ));
        // Another session's job, a missing root, an empty sweep: no work.
        assert!(!detached_job_in_table(idle, 30));
        assert!(!detached_job_in_table(idle, 40));
        assert!(!detached_job_in_table("", 20));
    }
}
