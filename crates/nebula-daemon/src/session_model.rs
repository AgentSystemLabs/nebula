//! CLAUDE MODEL SYNC — a session card names the model Claude is on now,
//! not only the one the session was launched with.
//!
//! Claude Code's `/model` switches a live session's model and fires no
//! hook for it. It does write the switch into the transcript at once, as a
//! local-command line (verified on 2.1.270):
//!
//! ```text
//! {"type":"user","message":{"content":"<local-command-stdout>Set model to `Opus 5 (1M context)` and saved as your default for new sessions</local-command-stdout>"},…}
//! ```
//!
//! — the name in backticks, or between ANSI bold codes on some builds —
//! and with the next prompt a `model` attachment naming the exact id:
//!
//! ```text
//! {"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-opus-5[1m]",…}},…}
//! ```
//!
//! That attachment also rides every session's first prompt, so a session
//! launched on the default model learns its name after one turn. Assistant
//! lines carry `message.model` too, but it drops the `[1m]` context suffix,
//! so they are not read.
//!
//! The daemon sweeps the transcripts the Claude hooks named (see
//! `Daemon::note_transcript`) every second and reads only what was
//! appended since the last pass — a transcript runs to megabytes, and a
//! switch is one line somewhere in the middle of it. The newest marker is
//! mapped onto the harness's model list (`opus`, `opus[1m]`, …) and written
//! to the row's `model`, which is also what a resume passes back as
//! `--model`, so the session comes back on the model it was switched to.
//!
//! What a transcript held before the sweep first saw it is history: a
//! resume passes the row's own `--model`, so an old switch says nothing
//! about the process now running. Only a row with no model at all (launched
//! on the default) takes its name from there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nebula_core::{AgentId, AgentKind, SessionRef};

use crate::registry::Daemon;

/// What the sweep remembers per transcript: where it is and how far it
/// has been read, so an unchanged file costs one `stat`.
pub type TranscriptOffsets = HashMap<AgentId, (PathBuf, u64)>;

/// The model the newest switch in the transcript's tail names, as the row
/// should store it; `None` when the tail records no switch.
fn read_model(path: &Path) -> Option<String> {
    let tail = crate::session_title::transcript_tail(path)?;
    tail.lines().rev().find_map(marker_model)
}

/// The newest switch among the complete lines written from byte `from` on,
/// and the offset just past the last of them — where the next read starts,
/// so a line Claude is still writing is read whole next time.
fn read_appended(path: &Path, from: u64) -> Option<(Option<String>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let complete = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let seen = String::from_utf8_lossy(&bytes[..complete])
        .lines()
        .rev()
        .find_map(marker_model);
    Some((seen, from + complete as u64))
}

/// The model one transcript line switches to, if it records a switch.
fn marker_model(line: &str) -> Option<String> {
    // Nearly every line is neither marker: skip the parse for those.
    if !line.contains("Set model to ") && !line.contains(r#""type":"model""#) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    match value.get("type")?.as_str()? {
        "user" => {
            let stdout = value.get("message")?.get("content")?.as_str()?;
            let named = stdout
                .strip_prefix("<local-command-stdout>")?
                .strip_prefix("Set model to ")?;
            alias(&display_name(named))
        }
        "attachment" => {
            let attachment = value.get("attachment")?;
            if attachment.get("type")?.as_str()? != "model" {
                return None;
            }
            let id = attachment.get("identity")?.get("modelId")?.as_str()?.trim();
            // An id outside the known families (a gateway's own name) is
            // still one Claude took, so it is what `--model` should get.
            (!id.is_empty()).then(|| alias(id).unwrap_or_else(|| id.to_string()))
        }
        _ => None,
    }
}

/// `/model`'s name for the model, out of the rest of its line:
/// `` `Opus 5 (1M context)` and saved as your default… `` →
/// `Opus 5 (1M context)`.
fn display_name(stdout: &str) -> String {
    let name = stdout.split(" and saved").next().unwrap_or(stdout);
    let name = name.split("</local-command-stdout>").next().unwrap_or(name);
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        match c {
            // A CSI sequence (`ESC [ 1 m`): drop through its final letter.
            '\u{1b}' => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            '`' => {}
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// The Claude harness's name for a model: the family its list holds that
/// the text names first, with `[1m]` for the 1M context window — so
/// `claude-opus-5[1m]` and `Opus 5 (1M context)` both read `opus[1m]`.
fn alias(model: &str) -> Option<String> {
    let lower = model.to_ascii_lowercase();
    let claude = nebula_core::harness::builtin("claude")?;
    let family = claude
        .model
        .models
        .into_iter()
        .map(|entry| entry.id)
        .filter(|id| id != "default")
        .filter_map(|id| Some((lower.find(&id)?, id)))
        .min_by_key(|(at, _)| *at)?
        .1;
    Some(if lower.contains("[1m]") || lower.contains("1m context") {
        format!("{family}[1m]")
    } else {
        family
    })
}

/// What the row's model becomes now that Claude is on `seen`, or `None`
/// when the row already says so. Rows compare by alias, so one launched on
/// an id the aliases don't reach (a Bedrock id from `claude_models`) keeps
/// that id while Claude stays in its family.
fn adopt(current: Option<&str>, seen: &str) -> Option<String> {
    let same = current
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .is_some_and(|current| {
            current == seen || alias(current).is_some_and(|a| alias(seen) == Some(a))
        });
    (!same).then(|| seen.to_string())
}

impl Daemon {
    /// One pass of the model sweep over every live session's transcript.
    pub fn sweep_claude_models(&self, offsets: &mut TranscriptOffsets) {
        let known: Vec<(AgentId, PathBuf)> = self
            .transcripts
            .lock()
            .unwrap()
            .iter()
            .map(|(id, t)| (id.clone(), t.transcript_path.clone()))
            .collect();
        let live = known
            .into_iter()
            .filter(|(id, _)| self.is_alive(&SessionRef::Agent(id.clone())))
            .collect();
        self.sync_transcript_models(live, offsets);
    }

    fn sync_transcript_models(
        &self,
        transcripts: Vec<(AgentId, PathBuf)>,
        offsets: &mut TranscriptOffsets,
    ) {
        offsets.retain(|id, _| transcripts.iter().any(|(t, _)| t == id));
        for (id, path) in transcripts {
            let len = std::fs::metadata(&path).map(|m| m.len()).ok();
            let read_to = offsets
                .get(&id)
                .filter(|(seen, _)| *seen == path)
                .map(|(_, at)| *at);
            match (read_to, len) {
                // Claude writes no transcript before the first prompt:
                // everything that appears will be new.
                (None, None) => {
                    offsets.insert(id, (path, 0));
                }
                (Some(_), None) => {}
                // Already written when first seen: history (see the
                // module docs), read only for a row that names no model.
                (None, Some(len)) => {
                    offsets.insert(id.clone(), (path.clone(), len));
                    let unset =
                        matches!(self.store.get_agent(&id), Ok(Some(a)) if a.model.is_none());
                    if let Some(seen) = read_model(&path).filter(|_| unset) {
                        self.adopt_claude_model(&id, &seen);
                    }
                }
                (Some(at), Some(len)) if len == at => {}
                // Rewritten shorter: nothing in it is news.
                (Some(at), Some(len)) if len < at => {
                    offsets.insert(id, (path, len));
                }
                (Some(at), Some(_)) => {
                    let Some((seen, next)) = read_appended(&path, at) else {
                        continue;
                    };
                    offsets.insert(id.clone(), (path, next));
                    if let Some(seen) = seen {
                        self.adopt_claude_model(&id, &seen);
                    }
                }
            }
        }
    }

    /// Store `seen` (a switch just read from the transcript) as the row's
    /// model when it names another. Built-in Claude rows only: a custom
    /// harness's model list is its own, and `--model opus` would mean
    /// nothing to it. Returns whether the row changed.
    fn adopt_claude_model(&self, id: &AgentId, seen: &str) -> bool {
        let Ok(Some(agent)) = self.store.get_agent(id) else {
            return false;
        };
        if agent.kind != AgentKind::Claude || agent.cloud_session_id.is_some() {
            return false;
        }
        let Some(model) = adopt(agent.model.as_deref(), seen) else {
            return false;
        };
        if let Err(e) = self.store.set_agent_model(id, Some(&model)) {
            tracing::warn!(agent = %id, error = %e, "model switch not adopted");
            return false;
        }
        tracing::info!(agent = %id, model = %model, "adopted the model Claude switched to");
        self.try_broadcast_agent(id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const SWITCH_BACKTICKS: &str = r#"{"parentUuid":"a","isSidechain":false,"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to `Opus 5 (1M context)` and saved as your default for new sessions</local-command-stdout>"},"uuid":"b"}"#;
    const SWITCH_ANSI: &str = r#"{"isSidechain":false,"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to \u001b[1mSonnet 5\u001b[22m and saved as your default for new sessions</local-command-stdout>"}}"#;
    const ATTACH_FABLE: &str = r#"{"isSidechain":false,"attachment":{"type":"model","identity":{"modelId":"claude-fable-5-1","marketingName":"Fable 5.1"},"text":"You are powered by…"},"type":"attachment","uuid":"c"}"#;
    const ASSISTANT_HAIKU: &str = r#"{"isSidechain":false,"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","role":"assistant","content":[]}}"#;

    fn write(path: &Path, lines: &[&str]) {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn alias_maps_ids_and_display_names_onto_the_model_list() {
        assert_eq!(alias("claude-opus-5").as_deref(), Some("opus"));
        assert_eq!(alias("claude-opus-5[1m]").as_deref(), Some("opus[1m]"));
        assert_eq!(alias("Opus 5 (1M context)").as_deref(), Some("opus[1m]"));
        assert_eq!(alias("claude-fable-5-1").as_deref(), Some("fable"));
        assert_eq!(alias("Sonnet 5").as_deref(), Some("sonnet"));
        assert_eq!(alias("claude-haiku-4-5-20251001").as_deref(), Some("haiku"));
        assert_eq!(
            alias("us.anthropic.claude-opus-5-v1:0").as_deref(),
            Some("opus")
        );
        // The family named first wins, whatever the list's order.
        assert_eq!(alias("Sonnet 5 (was Opus 5)").as_deref(), Some("sonnet"));
        assert_eq!(alias("Default (recommended)"), None);
        assert_eq!(alias("gpt-5.5"), None);
    }

    #[test]
    fn display_name_drops_the_quoting_and_the_rest_of_the_line() {
        assert_eq!(
            display_name("`Opus 5 (1M context)` and saved as your default for new sessions</local-command-stdout>"),
            "Opus 5 (1M context)"
        );
        assert_eq!(
            display_name("\u{1b}[1mFable 5\u{1b}[22m and saved as your default for new sessions</local-command-stdout>"),
            "Fable 5"
        );
        assert_eq!(display_name("opus</local-command-stdout>"), "opus");
    }

    #[test]
    fn markers_are_the_switch_line_and_the_model_attachment_only() {
        assert_eq!(marker_model(SWITCH_BACKTICKS).as_deref(), Some("opus[1m]"));
        assert_eq!(marker_model(SWITCH_ANSI).as_deref(), Some("sonnet"));
        assert_eq!(marker_model(ATTACH_FABLE).as_deref(), Some("fable"));
        // Assistant lines lose the context suffix: never a marker.
        assert_eq!(marker_model(ASSISTANT_HAIKU), None);
        // A prompt that merely quotes the switch line is not one.
        assert_eq!(
            marker_model(
                r#"{"type":"user","message":{"content":"why does it say Set model to `Haiku` there?"}}"#
            ),
            None
        );
        // Nor is a subagent's own switch.
        assert_eq!(
            marker_model(&SWITCH_ANSI.replace(r#""isSidechain":false"#, r#""isSidechain":true"#)),
            None
        );
        // An attachment id outside the families is kept whole.
        assert_eq!(
            marker_model(r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"gateway-big-1"}}}"#)
                .as_deref(),
            Some("gateway-big-1")
        );
    }

    #[test]
    fn read_model_takes_the_newest_marker_in_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.jsonl");
        assert_eq!(read_model(&path), None, "no transcript yet");
        write(&path, &[ASSISTANT_HAIKU]);
        assert_eq!(read_model(&path), None, "no switch recorded");
        write(&path, &[ATTACH_FABLE, SWITCH_BACKTICKS, ASSISTANT_HAIKU]);
        assert_eq!(read_model(&path).as_deref(), Some("opus[1m]"));
        write(&path, &[SWITCH_BACKTICKS, ATTACH_FABLE]);
        assert_eq!(read_model(&path).as_deref(), Some("fable"));
    }

    #[test]
    fn adopt_keeps_a_row_already_on_that_model() {
        assert_eq!(adopt(None, "opus").as_deref(), Some("opus"));
        assert_eq!(adopt(Some(""), "opus").as_deref(), Some("opus"));
        assert_eq!(adopt(Some("default"), "opus").as_deref(), Some("opus"));
        assert_eq!(adopt(Some("opus"), "opus"), None);
        assert_eq!(adopt(Some("sonnet"), "opus").as_deref(), Some("opus"));
        assert_eq!(adopt(Some("opus"), "opus[1m]").as_deref(), Some("opus[1m]"));
        assert_eq!(adopt(Some("opus[1m]"), "opus[1m]"), None);
        // An exact id the row launched on survives while the family holds.
        assert_eq!(adopt(Some("us.anthropic.claude-opus-5-v1:0"), "opus"), None);
        assert_eq!(
            adopt(Some("us.anthropic.claude-opus-5-v1:0"), "sonnet").as_deref(),
            Some("sonnet")
        );
        assert_eq!(adopt(Some("gateway-big-1"), "gateway-big-1"), None);
    }

    fn seeded_daemon(kind: AgentKind) -> (Arc<Daemon>, AgentId) {
        use crate::hooks::HookEnv;
        use crate::store::Store;
        use nebula_core::{Agent, AgentStatus, Project, ProjectId, Worktree, WorktreeId};
        let store = Arc::new(Store::open_in_memory().unwrap());
        store
            .insert_project(&Project {
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
                    status: AgentStatus::Running,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind,
                    custom_harness: None,
                    model: Some("sonnet".into()),
                    effort: Some("high".into()),
                    session_id: None,
                    cloud_session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: false,
                    recent_prompts: Vec::new(),
                },
                false,
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

    /// `/model` in a live session, end to end against a real store: the
    /// row takes each new switch once (and broadcasts it), nothing already
    /// read is read again, and a half-written line waits until it is whole.
    #[test]
    fn a_model_switch_lands_on_the_row_once_per_change() {
        use nebula_core::{Entity, ServerEvent};
        let (daemon, id) = seeded_daemon(AgentKind::Claude);
        let model = |daemon: &Daemon| daemon.store.get_agent(&id).unwrap().unwrap().model;
        let mut events = daemon.events.subscribe();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.jsonl");
        let mut offsets = TranscriptOffsets::new();
        let mut sweep =
            || daemon.sync_transcript_models(vec![(id.clone(), path.clone())], &mut offsets);

        // Before the first prompt there is no file: all of it will be new.
        sweep();
        write(&path, &[ASSISTANT_HAIKU, SWITCH_BACKTICKS]);
        sweep();
        assert_eq!(model(&daemon).as_deref(), Some("opus[1m]"));
        match events.try_recv() {
            Ok(ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            }) => assert_eq!(a.model.as_deref(), Some("opus[1m]")),
            other => panic!("expected the row's upsert, got {other:?}"),
        }
        assert_eq!(
            daemon
                .store
                .get_agent(&id)
                .unwrap()
                .unwrap()
                .effort
                .as_deref(),
            Some("high"),
            "the effort is not the switch's to change"
        );

        // Nothing appended: nothing re-read, so a row changed meanwhile
        // stays, and so does it past lines that switch nothing.
        daemon.store.set_agent_model(&id, Some("haiku")).unwrap();
        sweep();
        write(&path, &[ASSISTANT_HAIKU, SWITCH_BACKTICKS, ASSISTANT_HAIKU]);
        sweep();
        assert_eq!(model(&daemon).as_deref(), Some("haiku"));

        // A switch caught mid-write lands once its line is whole.
        let lines = [ASSISTANT_HAIKU, SWITCH_BACKTICKS, ASSISTANT_HAIKU];
        let mut text = lines.join("\n") + "\n";
        let half = ATTACH_FABLE.len() / 2;
        std::fs::write(&path, format!("{text}{}", &ATTACH_FABLE[..half])).unwrap();
        sweep();
        assert_eq!(model(&daemon).as_deref(), Some("haiku"));
        text.push_str(ATTACH_FABLE);
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        sweep();
        assert_eq!(model(&daemon).as_deref(), Some("fable"));

        // A session that went away is forgotten.
        daemon.sync_transcript_models(Vec::new(), &mut offsets);
        assert!(offsets.is_empty());
    }

    /// A transcript already written when the sweep first sees it — a
    /// resumed conversation — names models from before this process: the
    /// row's own `--model` outranks them. Only a row with no model at all
    /// takes the newest one as its name.
    #[test]
    fn history_only_names_a_row_launched_on_the_default() {
        let (daemon, id) = seeded_daemon(AgentKind::Claude);
        let model = |daemon: &Daemon| daemon.store.get_agent(&id).unwrap().unwrap().model;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.jsonl");
        write(&path, &[SWITCH_BACKTICKS, ATTACH_FABLE, ASSISTANT_HAIKU]);
        let first_sight = || {
            daemon.sync_transcript_models(
                vec![(id.clone(), path.clone())],
                &mut TranscriptOffsets::new(),
            )
        };

        first_sight();
        assert_eq!(model(&daemon).as_deref(), Some("sonnet"));

        daemon.store.set_agent_model(&id, None).unwrap();
        first_sight();
        assert_eq!(model(&daemon).as_deref(), Some("fable"));
    }

    #[test]
    fn only_builtin_claude_rows_follow_the_transcript() {
        let (daemon, id) = seeded_daemon(AgentKind::Custom);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sid.jsonl");
        let mut offsets = TranscriptOffsets::new();
        daemon.sync_transcript_models(vec![(id.clone(), path.clone())], &mut offsets);
        write(&path, &[SWITCH_BACKTICKS]);
        daemon.sync_transcript_models(vec![(id.clone(), path.clone())], &mut offsets);
        assert_eq!(
            daemon
                .store
                .get_agent(&id)
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("sonnet")
        );
        assert!(!daemon.adopt_claude_model(&id, "opus"));
    }
}
