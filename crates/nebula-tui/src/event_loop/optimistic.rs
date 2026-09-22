//! OPTIMISTIC UPDATES for the verbs that change a row in place — rename,
//! archive, unarchive, delete a session, close a terminal.
//!
//! Each of them used to send its request and wait for the DAEMON to say the
//! row had changed. That answer is quick but not free — the INPUT LATENCY
//! PROBE put a delete's and an archive's at 13 ms, since the DAEMON sweeps
//! the process tree it is about to kill before it answers — and it is only
//! as quick as the DAEMON is idle: behind a `git worktree add`, a prewarm
//! sweep or a busy disk, the row the user just deleted sat there, still
//! selectable, until the answer came.
//!
//! Here the row changes on the keypress. The change is made by handing
//! [`handle_server_event`] the very event the DAEMON is about to broadcast
//! — the same upsert, the same removal — so cursors, the pane, the PALETTE
//! and every count move exactly as they will when the real one lands, which
//! then changes nothing. The request rides a `PendingIntent::Undo` holding
//! the row as it was: an Error puts it back (and flashes why, like every
//! other refusal), an Ack drops it.

use super::{handle_server_event, send_with};
use crate::app::{App, PendingIntent, Undo};
use nebula_core::{AgentId, ClientRequest, Entity, EntityId, ProjectId, ServerEvent, TerminalId};

/// Show `entity` as the DAEMON will have it, and send `make`'s request with
/// `before` — the row as it is now — to put back if it is refused.
fn upsert(
    app: &mut App,
    before: Entity,
    entity: Entity,
    out: &mut Vec<ClientRequest>,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    handle_server_event(app, ServerEvent::EntityUpserted { entity }, out);
    let undo = PendingIntent::Undo(Undo::Restore(Box::new(before)));
    send_with(app, out, undo, make);
}

/// Take the row `id` down, and send `make`'s request with the row and where
/// it sat to put back if it is refused. Until the DAEMON answers, an upsert
/// of that row — one already on its way when the delete was asked for — is
/// ignored (`App::deleting`), so the row cannot flicker back.
fn remove(
    app: &mut App,
    id: EntityId,
    index: usize,
    entity: Entity,
    out: &mut Vec<ClientRequest>,
    make: impl FnOnce(u64) -> ClientRequest,
) {
    app.deleting.insert(id.clone());
    handle_server_event(app, ServerEvent::EntityRemoved { id }, out);
    let undo = PendingIntent::Undo(Undo::Reinsert {
        index,
        entity: Box::new(entity),
    });
    send_with(app, out, undo, make);
}

pub(super) fn rename_agent(app: &mut App, id: AgentId, name: String, out: &mut Vec<ClientRequest>) {
    let make = |req_id| ClientRequest::RenameAgent {
        req_id,
        id: id.clone(),
        name: name.clone(),
    };
    // An empty name is the DAEMON's to refuse, in its own words.
    match app.tree.agents.iter().find(|a| a.id == id) {
        Some(before) if !name.trim().is_empty() => {
            let mut after = before.clone();
            after.name = name.trim().to_string();
            upsert(
                app,
                Entity::Agent(before.clone()),
                Entity::Agent(after),
                out,
                make,
            );
        }
        _ => send_with(app, out, PendingIntent::None, make),
    }
}

pub(super) fn rename_terminal(
    app: &mut App,
    id: TerminalId,
    name: String,
    out: &mut Vec<ClientRequest>,
) {
    let make = |req_id| ClientRequest::RenameTerminal {
        req_id,
        id: id.clone(),
        name: name.clone(),
    };
    match app.tree.terminals.iter().find(|t| t.id == id) {
        Some(before) if !name.trim().is_empty() => {
            let mut after = before.clone();
            after.name = name.trim().to_string();
            upsert(
                app,
                Entity::Terminal(before.clone()),
                Entity::Terminal(after),
                out,
                make,
            );
        }
        _ => send_with(app, out, PendingIntent::None, make),
    }
}

/// An empty name puts the row back on its folder's — the DAEMON's rule,
/// applied here the same way.
pub(super) fn rename_project(
    app: &mut App,
    id: ProjectId,
    name: String,
    out: &mut Vec<ClientRequest>,
) {
    let make = |req_id| ClientRequest::RenameProject {
        req_id,
        id: id.clone(),
        name: name.clone(),
    };
    match app.tree.projects.iter().find(|p| p.id == id) {
        Some(before) => {
            let mut after = before.clone();
            after.name = match name.trim() {
                "" => nebula_core::Project::folder_name(&before.repo_path),
                typed => typed.to_string(),
            };
            upsert(
                app,
                Entity::Project(before.clone()),
                Entity::Project(after),
                out,
                make,
            );
        }
        None => send_with(app, out, PendingIntent::None, make),
    }
}

/// Archive (`archived`) or unarchive an agent. An archived agent's process
/// is killed by the DAEMON, so its row goes down as not alive.
pub(super) fn set_archived(
    app: &mut App,
    id: AgentId,
    archived: bool,
    out: &mut Vec<ClientRequest>,
) {
    let make = |req_id| match archived {
        true => ClientRequest::ArchiveAgent {
            req_id,
            id: id.clone(),
        },
        false => ClientRequest::UnarchiveAgent {
            req_id,
            id: id.clone(),
        },
    };
    match app.tree.agents.iter().find(|a| a.id == id) {
        Some(before) if before.archived != archived => {
            let mut after = before.clone();
            after.archived = archived;
            if archived {
                after.alive = false;
                after.archived_at = crate::app::now_ms();
            }
            upsert(
                app,
                Entity::Agent(before.clone()),
                Entity::Agent(after),
                out,
                make,
            );
        }
        _ => send_with(app, out, PendingIntent::None, make),
    }
}

pub(super) fn delete_agent(app: &mut App, id: AgentId, out: &mut Vec<ClientRequest>) {
    let make = |req_id| ClientRequest::DeleteAgent {
        req_id,
        id: id.clone(),
    };
    match app.tree.agents.iter().position(|a| a.id == id) {
        Some(index) => {
            let row = Entity::Agent(app.tree.agents[index].clone());
            remove(app, EntityId::Agent(id.clone()), index, row, out, make);
        }
        None => send_with(app, out, PendingIntent::None, make),
    }
}

pub(super) fn close_terminal(app: &mut App, id: TerminalId, out: &mut Vec<ClientRequest>) {
    let make = |req_id| ClientRequest::CloseTerminal {
        req_id,
        id: id.clone(),
    };
    match app.tree.terminals.iter().position(|t| t.id == id) {
        Some(index) => {
            let row = Entity::Terminal(app.tree.terminals[index].clone());
            remove(app, EntityId::Terminal(id.clone()), index, row, out, make);
        }
        None => send_with(app, out, PendingIntent::None, make),
    }
}

fn id_of(entity: &Entity) -> EntityId {
    match entity {
        Entity::Project(p) => EntityId::Project(p.id.clone()),
        Entity::Worktree(w) => EntityId::Worktree(w.id.clone()),
        Entity::Agent(a) => EntityId::Agent(a.id.clone()),
        Entity::Terminal(t) => EntityId::Terminal(t.id.clone()),
        Entity::Link(l) => EntityId::Link(l.id.clone()),
    }
}

/// Is this upsert for a row that was deleted here and whose delete the
/// DAEMON has not answered yet? Then it was on its way before the delete
/// was, and showing it would bring the row back for a frame.
pub(super) fn is_deleting(app: &App, entity: &Entity) -> bool {
    !app.deleting.is_empty() && app.deleting.contains(&id_of(entity))
}

/// The DAEMON did it: the row as it was is no longer needed.
pub(super) fn settled(app: &mut App, undo: Undo) {
    if let Undo::Reinsert { entity, .. } = undo {
        app.deleting.remove(&id_of(&entity));
    }
}

/// The DAEMON refused: the row goes back to what — and where — it was.
pub(super) fn undo(app: &mut App, undo: Undo, out: &mut Vec<ClientRequest>) {
    let entity = match undo {
        Undo::Restore(entity) => *entity,
        Undo::Reinsert { index, entity } => {
            app.deleting.remove(&id_of(&entity));
            // In its old place, so the list reads as it did; the upsert
            // below then finds the row and only refreshes it.
            match &*entity {
                Entity::Agent(a) => {
                    let at = index.min(app.tree.agents.len());
                    app.tree.agents.insert(at, a.clone());
                }
                Entity::Terminal(t) => {
                    let at = index.min(app.tree.terminals.len());
                    app.tree.terminals.insert(at, t.clone());
                }
                _ => {}
            }
            *entity
        }
    };
    handle_server_event(app, ServerEvent::EntityUpserted { entity }, out);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{hse, seed_tree};
    use super::*;
    use crate::app::Focus;

    fn req_id_of(out: &[ClientRequest]) -> u64 {
        out.iter()
            .find_map(|r| match r {
                ClientRequest::RenameAgent { req_id, .. }
                | ClientRequest::ArchiveAgent { req_id, .. }
                | ClientRequest::UnarchiveAgent { req_id, .. }
                | ClientRequest::DeleteAgent { req_id, .. } => Some(*req_id),
                _ => None,
            })
            .expect("the request went out")
    }

    fn agent_name(app: &App, id: &str) -> Option<String> {
        app.tree
            .agents
            .iter()
            .find(|a| a.id.0 == id)
            .map(|a| a.name.clone())
    }

    /// A rename shows on the keypress, the DAEMON's refusal puts the old
    /// name back, and its say-so is what the footer reads.
    #[test]
    fn a_rename_shows_at_once_and_a_refusal_puts_the_name_back() {
        let mut app = App::new();
        seed_tree(&mut app); // p1 / w1(main) / a1
        let was = agent_name(&app, "a1").expect("a1 is seeded");
        let mut out = Vec::new();

        rename_agent(
            &mut app,
            AgentId("a1".into()),
            "  fix login  ".into(),
            &mut out,
        );
        assert_eq!(
            agent_name(&app, "a1").as_deref(),
            Some("fix login"),
            "the new name, trimmed as the DAEMON will trim it, before any answer"
        );

        hse(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id_of(&out)),
                message: "name is taken".into(),
            },
        );
        assert_eq!(agent_name(&app, "a1"), Some(was), "the refusal undid it");
        assert_eq!(app.flash.as_deref(), Some("name is taken"));
    }

    /// An empty name is refused by the DAEMON, in its words: nothing is
    /// shown for it, and the request still goes out to be refused.
    #[test]
    fn an_empty_rename_changes_nothing_here() {
        let mut app = App::new();
        seed_tree(&mut app);
        let was = agent_name(&app, "a1");
        let mut out = Vec::new();
        rename_agent(&mut app, AgentId("a1".into()), "   ".into(), &mut out);
        assert_eq!(agent_name(&app, "a1"), was);
        assert!(matches!(
            out.as_slice(),
            [ClientRequest::RenameAgent { .. }]
        ));
    }

    /// A deleted session's row is gone on the keypress; an upsert of it that
    /// was already on its way does not bring it back; the Ack ends that.
    #[test]
    fn a_deleted_row_stays_down_until_the_daemon_answers() {
        let mut app = App::new();
        seed_tree(&mut app);
        app.focus = Focus::Sessions;
        let straggler = Entity::Agent(app.tree.agents[0].clone());
        let mut out = Vec::new();

        delete_agent(&mut app, AgentId("a1".into()), &mut out);
        assert!(agent_name(&app, "a1").is_none(), "gone before any answer");

        hse(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: straggler.clone(),
            },
        );
        assert!(
            agent_name(&app, "a1").is_none(),
            "an upsert sent before the delete was handled is not a resurrection"
        );

        hse(
            &mut app,
            ServerEvent::Ack {
                req_id: req_id_of(&out),
                created: None,
            },
        );
        assert!(app.deleting.is_empty(), "the Ack ends the wait");
    }

    /// A delete the DAEMON refuses puts the row back where it was.
    #[test]
    fn a_refused_delete_puts_the_row_back_in_its_place() {
        let mut app = App::new();
        seed_tree(&mut app);
        let mut second = app.tree.agents[0].clone();
        second.id = AgentId("a2".into());
        second.name = "agent-2".into();
        app.tree.agents.push(second);
        let order =
            |app: &App| -> Vec<String> { app.tree.agents.iter().map(|a| a.id.0.clone()).collect() };
        let before = order(&app);
        let mut out = Vec::new();

        delete_agent(&mut app, AgentId("a1".into()), &mut out);
        assert_eq!(order(&app), vec!["a2".to_string()]);

        hse(
            &mut app,
            ServerEvent::Error {
                req_id: Some(req_id_of(&out)),
                message: "database is locked".into(),
            },
        );
        assert_eq!(order(&app), before, "back, and first again");
        assert!(app.deleting.is_empty());
    }

    /// Unarchive is the archive's mirror: the row is back among the live
    /// sessions on the keypress.
    #[test]
    fn archive_and_unarchive_flip_the_row_at_once() {
        let mut app = App::new();
        seed_tree(&mut app);
        let archived = |app: &App| app.tree.agents[0].archived;
        let mut out = Vec::new();

        set_archived(&mut app, AgentId("a1".into()), true, &mut out);
        assert!(archived(&app) && !app.tree.agents[0].alive);
        set_archived(&mut app, AgentId("a1".into()), false, &mut out);
        assert!(!archived(&app));
        let sent: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::ArchiveAgent { .. } => Some("archive"),
                ClientRequest::UnarchiveAgent { .. } => Some("unarchive"),
                _ => None,
            })
            .collect();
        assert_eq!(
            sent,
            ["archive", "unarchive"],
            "both still asked of the DAEMON"
        );
    }
}
