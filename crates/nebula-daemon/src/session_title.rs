//! CLAUDE TITLE SYNC — keeping a Claude Code session's own name and its
//! NEBULA row's name tied (issue #25).
//!
//! Claude Code names a session with `/rename <name>` (also `--name`, and
//! shown in its prompt box, `/resume` picker, `/rc` list and window title).
//! It persists the name beside the transcript as
//! `<transcript dir>/<session id>/custom-title.json` (`{"customTitle": …}`)
//! and as `{"type":"custom-title",…}` lines in the transcript itself —
//! verified on 2.1.261 — but fires no hook for it. So:
//!
//! - **Claude → nebula.** Every hook payload names the transcript
//!   (`transcript_path` + `session_id`); the daemon remembers it and reads
//!   the title on every hook, and at once when the PTY's window title
//!   changes (`pty::title`). A title Claude did not hold before is adopted
//!   as a user RENAME: the row takes it and AUTO-TITLE is retired. The
//!   comparison is against the title *last seen from Claude*, not the
//!   row's name, so a name the user set in nebula meanwhile survives
//!   re-reading Claude's older one.
//! - **nebula → Claude.** The `UserPromptSubmit` hook reply carries
//!   `hookSpecificOutput.sessionTitle` whenever the row's name (user- or
//!   AUTO-TITLE-set) differs from Claude's; Claude renames itself and
//!   persists it, which the first direction then reads back. Only that
//!   event's reply persists the name (a `SessionStart` reply changes the
//!   display alone), and Codex has no such field.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nebula_core::{AgentId, AgentKind};

use crate::registry::{sanitize_title, Daemon};

/// The sidecar Claude writes beside the transcript on `/rename`.
pub const SIDECAR: &str = "custom-title.json";
/// How much of the transcript's tail to search when no sidecar exists
/// (older Claude versions): the title line is re-appended around every
/// prompt, so it is never far from the end for long.
const TAIL_BYTES: u64 = 64 * 1024;
/// When the window title changes, when to (re)read the persisted title.
/// The PTY bytes and the sidecar write are not ordered, so the first read
/// can land before the file does.
const TITLE_READ_DELAYS: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_millis(300),
    Duration::from_secs(1),
    Duration::from_secs(3),
];

/// Where Claude keeps a session's title on disk, from what every hook
/// payload reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRef {
    pub transcript_path: PathBuf,
    pub session_id: String,
}

impl TranscriptRef {
    pub fn from_payload(transcript_path: Option<&str>, session_id: Option<&str>) -> Option<Self> {
        let transcript_path = transcript_path.filter(|p| !p.is_empty())?;
        let session_id = session_id.filter(|s| !s.is_empty())?;
        Some(Self {
            transcript_path: PathBuf::from(transcript_path),
            session_id: session_id.to_string(),
        })
    }

    fn sidecar(&self) -> Option<PathBuf> {
        Some(
            self.transcript_path
                .parent()?
                .join(&self.session_id)
                .join(SIDECAR),
        )
    }

    /// The title Claude currently holds for the session, or `None` when it
    /// was never named (or nothing is readable).
    pub fn read_title(&self) -> Option<String> {
        self.sidecar()
            .and_then(|p| read_sidecar(&p))
            .or_else(|| read_transcript_tail(&self.transcript_path))
    }
}

fn custom_title(value: &serde_json::Value) -> Option<String> {
    value
        .get("customTitle")?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn read_sidecar(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    custom_title(&serde_json::from_str(&text).ok()?)
}

/// The last `{"type":"custom-title"}` line within the transcript's tail.
fn read_transcript_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;
    tail.lines()
        .rev()
        .filter(|line| line.contains(r#""type":"custom-title""#))
        .find_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            (value.get("type")?.as_str()? == "custom-title")
                .then(|| custom_title(&value))
                .flatten()
        })
}

/// The name inside a Claude window title: `✳ Fix Login Redirect` →
/// `Fix Login Redirect`. Only a hint that a read is worth doing, so the
/// glyph rule is loose on purpose.
pub fn title_text(osc_title: &str) -> &str {
    osc_title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
}

/// The generated `agent-N` default a fresh row wears until AUTO-TITLE or the
/// user names it — never worth pushing into Claude.
pub fn is_default_agent_name(name: &str) -> bool {
    name.strip_prefix("agent-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The row's naming state, as the store reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleState {
    pub name: String,
    pub claude_title: Option<String>,
    pub auto_title_pending: bool,
    pub kind: AgentKind,
}

impl TitleState {
    /// The title the `UserPromptSubmit` reply should hand Claude: the
    /// row's name when it is user- or AUTO-TITLE-set and not what Claude
    /// already holds. `None` while the AUTO-TITLE is still pending (the
    /// instruction rides that reply instead), for a default name, when the
    /// two agree, and for every non-Claude harness.
    pub fn to_push(&self) -> Option<&str> {
        if self.kind != AgentKind::Claude
            || self.auto_title_pending
            || is_default_agent_name(&self.name)
            || self.claude_title.as_deref() == Some(self.name.as_str())
        {
            return None;
        }
        Some(&self.name)
    }
}

impl Daemon {
    /// Every hook payload names the transcript; remember it so a window
    /// title change can find the persisted title without a hook.
    pub fn note_transcript(&self, id: &AgentId, transcript: TranscriptRef) {
        self.transcripts
            .lock()
            .unwrap()
            .insert(id.clone(), transcript);
    }

    /// Read the title Claude holds for `id` and adopt it if Claude changed
    /// it since the last sync. Returns whether the row changed.
    pub fn sync_claude_title(self: &Arc<Self>, id: &AgentId) -> bool {
        let transcript = self.transcripts.lock().unwrap().get(id).cloned();
        let Some(transcript) = transcript else {
            return false;
        };
        let Some(title) = transcript.read_title() else {
            return false;
        };
        let title = sanitize_title(&title);
        if title.is_empty() {
            return false;
        }
        match self.store.adopt_claude_title(id, &title) {
            Ok(true) => {
                tracing::info!(agent = %id, title = %title, "adopted the session title Claude set");
                self.try_broadcast_agent(id);
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!(agent = %id, error = %e, "session title not adopted");
                false
            }
        }
    }

    /// The PTY's window title changed. Titles Claude already holds (the
    /// glyph flip of a permission prompt, a frame re-set) cost nothing; a
    /// new name is chased with a few delayed reads, since the sidecar may
    /// land after the bytes.
    pub fn on_pty_title(self: &Arc<Self>, id: &AgentId, osc_title: &str) {
        let text = title_text(osc_title);
        if text.is_empty() {
            return;
        }
        let known = self.store.agent_claude_title(id).ok().flatten();
        if known.as_deref() == Some(text) {
            return;
        }
        if !self.transcripts.lock().unwrap().contains_key(id) {
            return;
        }
        let daemon = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            for delay in TITLE_READ_DELAYS {
                tokio::time::sleep(delay).await;
                if daemon.sync_claude_title(&id) {
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn title_text_drops_the_status_glyph() {
        assert_eq!(title_text("✳ Fix Login Redirect"), "Fix Login Redirect");
        assert_eq!(title_text("Fix Login Redirect"), "Fix Login Redirect");
        assert_eq!(title_text("  ✳  "), "");
        assert_eq!(title_text(""), "");
    }

    #[test]
    fn default_names_are_agent_n_only() {
        assert!(is_default_agent_name("agent-1"));
        assert!(is_default_agent_name("agent-42"));
        assert!(!is_default_agent_name("agent-"));
        assert!(!is_default_agent_name("agent-x"));
        assert!(!is_default_agent_name("Fix Login Redirect"));
    }

    #[test]
    fn transcript_ref_needs_both_halves() {
        assert!(TranscriptRef::from_payload(Some("/t/s.jsonl"), None).is_none());
        assert!(TranscriptRef::from_payload(None, Some("s")).is_none());
        assert!(TranscriptRef::from_payload(Some(""), Some("s")).is_none());
        let t = TranscriptRef::from_payload(Some("/t/s.jsonl"), Some("s")).unwrap();
        assert_eq!(
            t.sidecar().unwrap(),
            PathBuf::from("/t/s/custom-title.json")
        );
    }

    #[test]
    fn sidecar_wins_then_the_transcript_tail_then_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("sid.jsonl");
        let t = TranscriptRef::from_payload(transcript.to_str(), Some("sid")).unwrap();
        assert_eq!(t.read_title(), None, "no files at all");

        // Transcript only (older Claude): the *last* custom-title line
        // wins, and unrelated lines mentioning the type are ignored.
        write(
            &transcript,
            concat!(
                r#"{"type":"custom-title","customTitle":"First Name","sessionId":"sid"}"#,
                "\n",
                r#"{"type":"user","message":{"content":"note: \"type\":\"custom-title\" is a thing"}}"#,
                "\n",
                r#"{"type":"custom-title","customTitle":"Second Name","sessionId":"sid"}"#,
                "\n",
                r#"{"type":"agent-name","agentName":"Second Name"}"#,
                "\n",
            ),
        );
        assert_eq!(t.read_title().as_deref(), Some("Second Name"));

        // The sidecar, once present, is the answer.
        let sidecar = dir.path().join("sid").join(SIDECAR);
        write(&sidecar, r#"{"customTitle":"Side Name"}"#);
        assert_eq!(t.read_title().as_deref(), Some("Side Name"));

        // An empty or malformed sidecar falls through to the transcript.
        write(&sidecar, r#"{"customTitle":"  "}"#);
        assert_eq!(t.read_title().as_deref(), Some("Second Name"));
        write(&sidecar, "not json");
        assert_eq!(t.read_title().as_deref(), Some("Second Name"));
    }

    #[test]
    fn tail_read_finds_a_title_behind_a_large_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("sid.jsonl");
        let t = TranscriptRef::from_payload(transcript.to_str(), Some("sid")).unwrap();
        let mut text = String::new();
        text.push_str(r#"{"type":"custom-title","customTitle":"Early","sessionId":"sid"}"#);
        text.push('\n');
        // Far older than the tail window: only what's inside it counts.
        text.push_str(&format!(
            "{{\"type\":\"user\",\"message\":\"{}\"}}\n",
            "x".repeat(2 * TAIL_BYTES as usize)
        ));
        text.push_str(r#"{"type":"custom-title","customTitle":"Late","sessionId":"sid"}"#);
        text.push('\n');
        text.push_str(&format!(
            "{{\"type\":\"user\",\"message\":\"{}\"}}\n",
            "y".repeat(TAIL_BYTES as usize / 2)
        ));
        write(&transcript, &text);
        assert_eq!(t.read_title().as_deref(), Some("Late"));
    }

    fn seeded_daemon() -> (Arc<Daemon>, AgentId) {
        use crate::hooks::HookEnv;
        use crate::store::Store;
        use nebula_core::{Agent, AgentStatus, Project, ProjectId, Worktree, WorktreeId};
        let store = Arc::new(Store::open_in_memory().unwrap());
        store
            .insert_project(&Project {
                workspace_id: Default::default(),
                id: ProjectId("p1".into()),
                name: "p".into(),
                repo_path: "/tmp/p".into(),
                sort_order: 0,
            })
            .unwrap();
        store
            .insert_worktree(&Worktree {
                id: WorktreeId("w1".into()),
                project_id: ProjectId("p1".into()),
                path: "/tmp/p".into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            })
            .unwrap();
        let id = AgentId("a1".into());
        store
            .insert_agent_with_auto_title(
                &Agent {
                    id: id.clone(),
                    worktree_id: WorktreeId("w1".into()),
                    name: "agent-1".into(),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    model: None,
                    effort: None,
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: false,
                    cloud_mirroring: false,
                },
                true,
            )
            .unwrap();
        let daemon = Daemon::new(
            store,
            HookEnv {
                port: 0,
                token: String::new(),
            },
        );
        (daemon, id)
    }

    /// The whole Claude → nebula direction against a real store: a title
    /// Claude persisted is adopted (and broadcast) once, a nebula rename
    /// made afterwards survives re-reads, and the next Claude change wins
    /// again.
    #[tokio::test]
    async fn sync_adopts_claudes_title_once_and_keeps_a_later_nebula_rename() {
        use nebula_core::{Entity, ServerEvent};
        let (daemon, id) = seeded_daemon();
        let name = |daemon: &Daemon| daemon.store.get_agent(&id).unwrap().unwrap().name;
        let mut events = daemon.events.subscribe();

        // No transcript known yet: nothing to read.
        assert!(!daemon.sync_claude_title(&id));

        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("sid.jsonl");
        daemon.note_transcript(
            &id,
            TranscriptRef::from_payload(transcript.to_str(), Some("sid")).unwrap(),
        );
        assert!(!daemon.sync_claude_title(&id), "never named in Claude");
        assert_eq!(name(&daemon), "agent-1");

        // `/rename From Claude` in the CLI.
        let sidecar = dir.path().join("sid").join(SIDECAR);
        write(&sidecar, r#"{"customTitle":"  From   Claude "}"#);
        assert!(daemon.sync_claude_title(&id));
        assert_eq!(name(&daemon), "From Claude", "sanitized like any title");
        assert!(!daemon.store.agent_auto_title_pending(&id).unwrap());
        match events.try_recv() {
            Ok(ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            }) => assert_eq!(a.name, "From Claude"),
            other => panic!("expected the row's upsert, got {other:?}"),
        }
        assert!(!daemon.sync_claude_title(&id), "same title again is silent");

        // `r` in nebula afterwards: a re-read of Claude's older title
        // (every hook triggers one) must not undo it.
        daemon.rename_agent(&id, "From Nebula").unwrap();
        assert!(!daemon.sync_claude_title(&id));
        assert_eq!(name(&daemon), "From Nebula");

        // The next `/rename` in Claude wins again.
        write(&sidecar, r#"{"customTitle":"Claude Again"}"#);
        assert!(daemon.sync_claude_title(&id));
        assert_eq!(name(&daemon), "Claude Again");

        // A window title Claude already holds is not worth a read; one it
        // doesn't schedules reads that find the new sidecar.
        daemon.on_pty_title(&id, "✳ Claude Again");
        write(&sidecar, r#"{"customTitle":"Via Title"}"#);
        daemon.on_pty_title(&id, "✳ Via Title");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while name(&daemon) != "Via Title" {
            assert!(
                std::time::Instant::now() < deadline,
                "title read never landed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn push_only_a_settled_non_default_name_claude_does_not_hold() {
        let state = |name: &str, claude: Option<&str>, pending: bool, kind: AgentKind| TitleState {
            name: name.into(),
            claude_title: claude.map(str::to_string),
            auto_title_pending: pending,
            kind,
        };
        let claude = AgentKind::Claude;
        assert_eq!(
            state("Fix Login", None, false, claude).to_push(),
            Some("Fix Login")
        );
        assert_eq!(
            state("Fix Login", Some("Old"), false, claude).to_push(),
            Some("Fix Login")
        );
        assert_eq!(
            state("Fix Login", Some("Fix Login"), false, claude).to_push(),
            None
        );
        assert_eq!(state("agent-1", None, true, claude).to_push(), None);
        assert_eq!(state("agent-1", None, false, claude).to_push(), None);
        assert_eq!(
            state("Fix Login", None, false, AgentKind::Codex).to_push(),
            None
        );
    }
}
