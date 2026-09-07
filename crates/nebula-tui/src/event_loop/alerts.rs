//! The two ways nebula reaches a user who is not looking at it: the DONE
//! SOUND and FEEDBACK SOUND (`Config::done_sound` / `Config::feedback_sound`,
//! played here), and the desktop notification a session that stops to ask
//! posts while the terminal window is in the background. `event_loop.rs`
//! decides *when* — the status edges, the per-frame drain of
//! `App::pending_ding` and `App::pending_feedback`, the focus and ssh
//! gates — this module only does the reaching, and fails soft: an `afplay`
//! that won't start falls back to the bell, a notifier that is missing or
//! exits non-zero is a debug line in tui.log.

use crate::app::{FeedbackAlert, Tree};
use crate::config::Sound;
use nebula_core::AgentId;
use std::process::{Command, Stdio};

/// Ring `sound` once: a file goes to `afplay` (detached; a helper thread
/// reaps it so no zombie lingers), the bell — and a file whose `afplay`
/// won't start — is the BEL written through `backend`, so over ssh it
/// rings the terminal the user is sitting at.
pub(super) fn play_sound<W: std::io::Write>(backend: &mut W, sound: Sound) {
    if let Sound::File(path) = &sound {
        if let Ok(mut child) = Command::new("afplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
    }
    let _ = backend.write_all(b"\x07");
    let _ = backend.flush();
}

/// The alert for `agent`, as its row reads on screen: the session's name
/// and `<project> · <branch>`. `None` for a row the tree doesn't hold (a
/// status that raced a delete).
pub(super) fn alert_for(tree: &Tree, agent: &AgentId) -> Option<FeedbackAlert> {
    let a = tree.agents.iter().find(|a| a.id == *agent)?;
    let worktree = tree.worktrees.iter().find(|w| w.id == a.worktree_id);
    let project = worktree.and_then(|w| tree.projects.iter().find(|p| p.id == w.project_id));
    let place = match (project, worktree) {
        (Some(p), Some(w)) => format!("{} · {}", p.name, w.branch),
        (None, Some(w)) => w.branch.clone(),
        _ => String::new(),
    };
    Some(FeedbackAlert {
        session: a.name.clone(),
        place,
    })
}

/// Post one desktop notification per alert — `osascript` on macOS,
/// `notify-send` elsewhere — detached, reaped on a helper thread. A
/// notifier that is missing or exits non-zero is logged at debug and
/// otherwise ignored: the sound already rang, and a box with no desktop is
/// not an error.
pub(super) fn notify_desktop(alerts: &[FeedbackAlert]) {
    for alert in alerts {
        let (program, args) = notifier_command(alert, cfg!(target_os = "macos"));
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
}

/// The notifier to run for `alert`, as a program and its argv — never a
/// shell line, so the only quoting is AppleScript's own. macOS shows
/// *nebula* / *`<session>` needs feedback* / *`<project> · <branch>`*;
/// `notify-send` gets the same as app name, summary and body.
fn notifier_command(alert: &FeedbackAlert, macos: bool) -> (&'static str, Vec<String>) {
    let summary = format!("{} needs feedback", alert.session);
    if macos {
        let mut script = format!(
            "display notification {} with title \"nebula\" subtitle {}",
            applescript_str(&alert.place),
            applescript_str(&summary)
        );
        if alert.place.is_empty() {
            // No body to show: the subtitle carries the whole message.
            script = format!(
                "display notification {} with title \"nebula\"",
                applescript_str(&summary)
            );
        }
        ("osascript", vec!["-e".into(), script])
    } else {
        (
            "notify-send",
            vec!["--app-name=nebula".into(), summary, alert.place.clone()],
        )
    }
}

/// `s` as an AppleScript string literal: backslashes and double quotes
/// escaped, control characters dropped — a session name is one line.
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

    fn alert(session: &str, place: &str) -> FeedbackAlert {
        FeedbackAlert {
            session: session.into(),
            place: place.into(),
        }
    }

    /// A session name is user text — a quote or a backslash in it must
    /// not break out of the AppleScript literal, and a newline can't make
    /// the script two statements.
    #[test]
    fn applescript_literals_are_escaped() {
        assert_eq!(applescript_str("agent-2"), r#""agent-2""#);
        assert_eq!(
            applescript_str(r#"say "hi" \ now"#),
            r#""say \"hi\" \\ now""#
        );
        assert_eq!(applescript_str("one\ntwo\ttab"), r#""onetwotab""#);
        assert_eq!(applescript_str("demo · main"), r#""demo · main""#);
    }

    /// The macOS notifier is `osascript -e <one display notification>`,
    /// nebula as the title, the session as the subtitle, the place as the
    /// body — and just the session when there is no place to name. Linux
    /// gets `notify-send` with the same three under its own names.
    #[test]
    fn notifier_command_names_the_session_and_its_place() {
        let (program, args) = notifier_command(&alert("Fix Login", "demo · main"), true);
        assert_eq!(program, "osascript");
        assert_eq!(
            args,
            [
                "-e",
                r#"display notification "demo · main" with title "nebula" subtitle "Fix Login needs feedback""#,
            ]
        );

        let (_, args) = notifier_command(&alert("agent-2", ""), true);
        assert_eq!(
            args[1],
            r#"display notification "agent-2 needs feedback" with title "nebula""#
        );

        let (program, args) = notifier_command(&alert("Fix Login", "demo · main"), false);
        assert_eq!(program, "notify-send");
        assert_eq!(
            args,
            [
                "--app-name=nebula",
                "Fix Login needs feedback",
                "demo · main"
            ]
        );
    }

    /// The bell path writes exactly one BEL through the backend; `off` is
    /// the caller's business (a `None` never reaches here).
    #[test]
    fn the_bell_is_one_bel_byte() {
        let mut out = Vec::new();
        play_sound(&mut out, Sound::Bell);
        assert_eq!(out, b"\x07");
    }
}
