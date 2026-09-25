//! Claude Code's background sessions, as far as a nebula pane has to know
//! them. `/background` (or `claude --bg`) hands a session to Claude's own
//! daemon: the pane's CLI exits and the conversation carries on in a worker
//! that inherited the pane's `NEBULA_*` env — so its hooks keep reporting
//! against the same row, the forked session id included. `claude --resume
//! <id>` then refuses ("Session … is running as a background session … Run
//! `claude attach <short>` to open it"), which left the row a dead pane on
//! every view. A spawn that would resume such a session opens `claude
//! attach <short>` instead: the same conversation, live, and detaching
//! (Ctrl+Z) leaves it running.
//!
//! Two looks, cheapest first. The *hint* is one `stat`: Claude keeps a job
//! directory per background session, `<config>/jobs/<short>/`, `<short>`
//! being the session id's first eight characters. That layout is nobody's
//! contract, so the hint only decides whether the *probe* is worth its
//! cost: `claude agents --json` — the listing Claude documents for
//! scripting — run through the user's login shell like the agent itself,
//! which is most of a second behind the rc files. A hint that stops
//! matching (Claude moves its job dirs) degrades to
//! `Daemon::respawn_failed_resume`, which probes any Claude resume that
//! died at once with its transcript intact.

use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How much of a session id makes its background short id.
const SHORT_ID_LEN: usize = 8;
/// The listing's `kind` for a session Claude's daemon runs.
const BACKGROUND_KIND: &str = "background";
/// How long the probe may take. It runs under the spawn gate, so a login
/// shell that hangs has to cost seconds, not a wedged daemon.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_POLL: Duration = Duration::from_millis(10);

/// Whether `config_dir` (a Claude config dir, `~/.claude`) holds a
/// background job for `session_id` — running or long stopped, the hint
/// can't tell, which is what the probe is for.
pub fn job_hint(config_dir: &Path, session_id: &str) -> bool {
    let Some(short) = session_id.get(..SHORT_ID_LEN) else {
        return false;
    };
    // An id that isn't plain hex-and-letters names no job dir.
    short.bytes().all(|b| b.is_ascii_alphanumeric()) && config_dir.join("jobs").join(short).is_dir()
}

/// `claude attach <id>`: no model, effort or system prompt — the worker
/// keeps the flags it was backgrounded with — and never a resume to watch.
pub fn attach_args(id: &str) -> Vec<String> {
    vec!["attach".to_string(), id.to_string()]
}

/// One row of `claude agents --json`. Every field optional and every other
/// one ignored: a listing that grows or drops a field must not read as "no
/// background sessions".
#[derive(Deserialize)]
struct Listed {
    id: Option<String>,
    kind: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

/// The id `claude attach` takes for `session_id`, when `listing` (the
/// stdout of `claude agents --json`, behind whatever the rc files printed)
/// has it as an active background session.
pub fn listed_background_id(listing: &str, session_id: &str) -> Option<String> {
    let rows = listing_rows(listing)?;
    rows.into_iter()
        .filter(|row| row.kind.as_deref() == Some(BACKGROUND_KIND))
        .filter(|row| row.session_id.as_deref() == Some(session_id))
        .find_map(|row| row.id)
        // It becomes a command-line word: nothing that could read as a flag.
        .filter(|id| {
            !id.is_empty()
                && !id.starts_with('-')
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// The JSON array in `listing`: the first line-leading `[` that parses, so
/// a banner an interactive shell's rc files echo first is stepped over.
fn listing_rows(listing: &str) -> Option<Vec<Listed>> {
    let mut offset = 0;
    for line in listing.split_inclusive('\n') {
        if line.starts_with('[') {
            let mut stream =
                serde_json::Deserializer::from_str(&listing[offset..]).into_iter::<Vec<Listed>>();
            if let Some(Ok(rows)) = stream.next() {
                return Some(rows);
            }
        }
        offset += line.len();
    }
    None
}

/// Run the probe — `program args…` is `claude agents --json` behind the
/// login shell — and read `session_id`'s attach id off it. `None` for every
/// failure: no CLI, a timeout, an older Claude with no `agents` command.
pub fn probe(program: &str, args: &[String], session_id: &str) -> Option<String> {
    let listing = run_captured(program, args, PROBE_TIMEOUT)?;
    listed_background_id(&listing, session_id)
}

fn run_captured(program: &str, args: &[String], timeout: Duration) -> Option<String> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Own session, as `Daemon::probe_cli`: an interactive shell must not
    // reach the daemon's controlling terminal. It also makes the shell a
    // group leader, so a timeout can sweep whatever it started.
    unsafe {
        command.pre_exec(crate::registry::own_session);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Drained beside the wait: a listing past the pipe buffer would
    // otherwise block the CLI until the timeout.
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(PROBE_POLL),
            _ => {
                let group = nix::unistd::Pid::from_raw(child.id() as i32);
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
                let _ = child.wait();
                // The reader ends with the pipe; nothing waits on it.
                return None;
            }
        }
    }
    let out = reader.join().ok()?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "1a3138b9-8caa-4839-9ced-173a0cdff89b";

    /// The listing as Claude Code 2.1.274 prints it: a parked background
    /// session with no pid, the running one, and an interactive session.
    const LISTING: &str = r#"[
  {
    "id": "0652b103",
    "cwd": "/w/nebula",
    "kind": "background",
    "startedAt": 1787881644172,
    "sessionId": "0652b103-c008-49d6-8b86-63c4ff86a58f",
    "name": "Workspace name",
    "state": "blocked"
  },
  {
    "pid": 21146,
    "id": "1a3138b9",
    "cwd": "/w/nebula",
    "kind": "background",
    "startedAt": 1789614803320,
    "sessionId": "1a3138b9-8caa-4839-9ced-173a0cdff89b",
    "name": "Optimistic PR Worktree Nesting",
    "status": "idle",
    "state": "done"
  },
  {
    "pid": 62891,
    "cwd": "/w/nebula",
    "kind": "interactive",
    "startedAt": 1789618363505,
    "sessionId": "71c6fa6f-62ac-4aaa-bd23-39b5719d4a8f",
    "name": "nebula-da",
    "status": "idle"
  }
]
"#;

    #[test]
    fn a_listed_background_session_gives_its_attach_id() {
        assert_eq!(
            listed_background_id(LISTING, SID).as_deref(),
            Some("1a3138b9")
        );
        // Parked, no pid: still Claude's daemon's session, still refused a
        // resume, still attachable.
        assert_eq!(
            listed_background_id(LISTING, "0652b103-c008-49d6-8b86-63c4ff86a58f").as_deref(),
            Some("0652b103")
        );
    }

    #[test]
    fn interactive_and_unlisted_sessions_give_none() {
        assert_eq!(
            listed_background_id(LISTING, "71c6fa6f-62ac-4aaa-bd23-39b5719d4a8f"),
            None
        );
        assert_eq!(listed_background_id(LISTING, "nope"), None);
        assert_eq!(listed_background_id("[]", SID), None);
        assert_eq!(listed_background_id("", SID), None);
        assert_eq!(
            listed_background_id("error: unknown command 'agents'\n", SID),
            None
        );
    }

    #[test]
    fn rc_file_noise_before_the_listing_is_stepped_over() {
        let noisy = format!("Welcome back!\n[oh-my-zsh] update available\n{LISTING}");
        assert_eq!(
            listed_background_id(&noisy, SID).as_deref(),
            Some("1a3138b9")
        );
    }

    #[test]
    fn an_id_that_could_read_as_a_flag_is_refused() {
        let listing = format!(r#"[{{"id":"--help","kind":"background","sessionId":"{SID}"}}]"#);
        assert_eq!(listed_background_id(&listing, SID), None);
        let listing = format!(r#"[{{"id":"a b","kind":"background","sessionId":"{SID}"}}]"#);
        assert_eq!(listed_background_id(&listing, SID), None);
    }

    #[test]
    fn the_hint_is_the_job_dir_under_the_short_id() {
        let config = tempfile::tempdir().unwrap();
        assert!(!job_hint(config.path(), SID));
        std::fs::create_dir_all(config.path().join("jobs/1a3138b9")).unwrap();
        assert!(job_hint(config.path(), SID));
        assert!(!job_hint(config.path(), "2b3138b9-8caa"));
        // Too short, or not one plain path component: no job dir.
        assert!(!job_hint(config.path(), "1a31"));
        std::fs::create_dir_all(config.path().join("jobs/..")).unwrap();
        assert!(!job_hint(config.path(), "../../..-8caa"));
    }

    #[test]
    fn the_probe_reads_a_real_commands_stdout() {
        let script = format!("cat <<'EOF'\n{LISTING}EOF");
        let args = vec!["-c".to_string(), script];
        assert_eq!(probe("/bin/sh", &args, SID).as_deref(), Some("1a3138b9"));
        assert_eq!(
            probe("/bin/sh", &["-c".to_string(), "exit 3".to_string()], SID),
            None
        );
        assert_eq!(probe("/nebula-test/no-such-shell", &[], SID), None);
    }

    #[test]
    fn a_hung_probe_is_killed_at_the_timeout() {
        let started = Instant::now();
        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        assert_eq!(
            run_captured("/bin/sh", &args, Duration::from_millis(150)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
