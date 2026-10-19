//! Daemon-owned desktop notifications for status changes.
//!
//! The TUI can ring its terminal bell only while it is attached. Desktop
//! notifications live here so a session that asks, finishes, or fails can
//! still reach the user after every TUI window has gone away.

use nebula_core::{AgentId, AgentStatus};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const REPEAT_COOLDOWN: Duration = Duration::from_secs(30);
const BATCH_WINDOW: Duration = Duration::from_millis(900);
const BATCH_MAX: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NotificationKind {
    NeedsFeedback,
    Finished,
    Failed,
}

impl NotificationKind {
    pub(crate) fn of(status: AgentStatus) -> Option<Self> {
        match status {
            AgentStatus::NeedsFeedback => Some(Self::NeedsFeedback),
            AgentStatus::Finished => Some(Self::Finished),
            AgentStatus::Terminated | AgentStatus::Disconnected => Some(Self::Failed),
            AgentStatus::Fresh | AgentStatus::Running => None,
        }
    }

    fn single_summary(self, session: &str) -> String {
        match self {
            Self::NeedsFeedback => format!("{session} needs feedback"),
            Self::Finished => format!("{session} finished"),
            Self::Failed => format!("{session} failed"),
        }
    }

    fn batch_summary(self, count: usize) -> String {
        match self {
            Self::NeedsFeedback => format!("{count} sessions need feedback"),
            Self::Finished => format!("{count} sessions finished"),
            Self::Failed => format!("{count} sessions failed"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Notification {
    kind: NotificationKind,
    session: String,
    place: String,
}

impl Notification {
    pub(crate) fn new(kind: NotificationKind, session: String, place: String) -> Self {
        Self {
            kind,
            session,
            place,
        }
    }
}

#[derive(Default)]
pub(crate) struct NotificationGate {
    last: HashMap<(AgentId, NotificationKind), Instant>,
}

impl NotificationGate {
    fn allow(&mut self, agent: &AgentId, kind: NotificationKind, now: Instant) -> bool {
        let key = (agent.clone(), kind);
        if self
            .last
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < REPEAT_COOLDOWN)
        {
            return false;
        }
        self.last.insert(key, now);
        true
    }
}

pub(crate) struct NotificationCenter {
    gate: Mutex<NotificationGate>,
    tx: mpsc::Sender<Notification>,
}

impl NotificationCenter {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("nebula-notifications".into())
            .spawn(move || worker(rx))
            .expect("spawn notification worker");
        Self {
            gate: Mutex::new(NotificationGate::default()),
            tx,
        }
    }

    pub(crate) fn notify(&self, agent: &AgentId, notification: Notification) {
        if !self
            .gate
            .lock()
            .allow(agent, notification.kind, Instant::now())
        {
            return;
        }
        let _ = self.tx.send(notification);
    }
}

fn worker(rx: mpsc::Receiver<Notification>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let started = Instant::now();
        while batch.len() < BATCH_MAX {
            let elapsed = started.elapsed();
            let Some(left) = BATCH_WINDOW.checked_sub(elapsed) else {
                break;
            };
            match rx.recv_timeout(left) {
                Ok(next) => batch.push(next),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        post_desktop(&batch);
    }
}

fn post_desktop(batch: &[Notification]) {
    let (program, args) = notifier_command(batch, cfg!(target_os = "macos"));
    match Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(status) if !status.success() => {
                    tracing::debug!(program, %status, "desktop notification not posted");
                }
                Err(err) => tracing::debug!(program, %err, "desktop notification not reaped"),
                Ok(_) => {}
            });
        }
        Err(err) => tracing::debug!(program, %err, "desktop notification not posted"),
    }
}

fn notifier_command(batch: &[Notification], macos: bool) -> (&'static str, Vec<String>) {
    let summary = summary(batch);
    let body = body(batch);
    if macos {
        let script = if body.is_empty() {
            format!(
                "display notification {} with title \"nebula\"",
                applescript_str(&summary)
            )
        } else {
            format!(
                "display notification {} with title \"nebula\" subtitle {}",
                applescript_str(&body),
                applescript_str(&summary)
            )
        };
        ("osascript", vec!["-e".into(), script])
    } else {
        (
            "notify-send",
            vec!["--app-name=nebula".into(), summary, body],
        )
    }
}

fn summary(batch: &[Notification]) -> String {
    match batch {
        [] => "nebula".into(),
        [one] => one.kind.single_summary(&one.session),
        many if many.iter().all(|n| n.kind == many[0].kind) => {
            many[0].kind.batch_summary(many.len())
        }
        many => format!("{} session updates", many.len()),
    }
}

fn body(batch: &[Notification]) -> String {
    if batch.is_empty() {
        return String::new();
    }
    if let [one] = batch {
        return one.place.clone();
    }
    batch
        .iter()
        .map(|n| {
            if n.place.is_empty() {
                n.session.clone()
            } else {
                format!("{} ({})", n.session, n.place)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `s` as an AppleScript string literal: backslashes and double quotes
/// escaped, control characters dropped.
fn applescript_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(kind: NotificationKind, session: &str, place: &str) -> Notification {
        Notification::new(kind, session.into(), place.into())
    }

    #[test]
    fn gate_debounces_repeated_status_for_one_session() {
        let mut gate = NotificationGate::default();
        let id = AgentId("a1".into());
        let now = Instant::now();
        assert!(gate.allow(&id, NotificationKind::Finished, now));
        assert!(!gate.allow(
            &id,
            NotificationKind::Finished,
            now + Duration::from_secs(2)
        ));
        assert!(
            gate.allow(
                &id,
                NotificationKind::NeedsFeedback,
                now + Duration::from_secs(2)
            ),
            "a different event is distinct"
        );
        assert!(gate.allow(
            &id,
            NotificationKind::Finished,
            now + REPEAT_COOLDOWN + Duration::from_millis(1)
        ));
    }

    #[test]
    fn status_mapping_covers_user_visible_edges() {
        assert_eq!(
            NotificationKind::of(AgentStatus::NeedsFeedback),
            Some(NotificationKind::NeedsFeedback)
        );
        assert_eq!(
            NotificationKind::of(AgentStatus::Finished),
            Some(NotificationKind::Finished)
        );
        assert_eq!(
            NotificationKind::of(AgentStatus::Terminated),
            Some(NotificationKind::Failed)
        );
        assert_eq!(NotificationKind::of(AgentStatus::Running), None);
    }

    #[test]
    fn notifier_command_names_single_and_batched_updates() {
        let (program, args) = notifier_command(
            &[n(
                NotificationKind::NeedsFeedback,
                "Fix Login",
                "demo - main",
            )],
            false,
        );
        assert_eq!(program, "notify-send");
        assert_eq!(
            args,
            [
                "--app-name=nebula",
                "Fix Login needs feedback",
                "demo - main"
            ]
        );

        let (_, args) = notifier_command(
            &[
                n(NotificationKind::Finished, "one", "demo - main"),
                n(NotificationKind::Finished, "two", "api - feat"),
            ],
            false,
        );
        assert_eq!(args[1], "2 sessions finished");
        assert_eq!(args[2], "one (demo - main)\ntwo (api - feat)");

        let (program, args) = notifier_command(
            &[n(NotificationKind::Failed, r#"say "hi" \ now"#, "")],
            true,
        );
        assert_eq!(program, "osascript");
        assert_eq!(
            args[1],
            r#"display notification "say \"hi\" \\ now failed" with title "nebula""#
        );
    }
}
