use super::*;

impl Daemon {
    // ---- status machine plumbing ----

    /// Feed one hook (or synthetic) event through the agent's status machine
    /// and apply the resulting effects (persist + broadcast).
    pub fn apply_hook_event(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
    ) {
        enum Outcome {
            Effects(Vec<Effect>),
            UnknownAgent(HookEvent, Option<String>),
        }
        // A `nebula worktree` relocation waiting on this agent's turn end
        // holds that end at `running` (`AgentStatusMachine::set_relocating`;
        // `complete_pending_move` drains it). Read off the daemon's own
        // record before every event, so the machine's copy cannot go stale
        // — and before the machines lock, so the two are never nested.
        let relocating = self.pending_moves.lock().contains_key(agent_id);
        let outcome = {
            let mut machines = self.status_machines.lock();
            match machines.entry(agent_id.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    let machine = e.into_mut();
                    machine.set_relocating(relocating);
                    Outcome::Effects(machine.handle(event, session_id.as_deref(), Instant::now()))
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Lazily seed from the persisted row.
                    match self.store.get_agent(agent_id) {
                        Ok(Some(agent)) => {
                            let machine = slot
                                .insert(AgentStatusMachine::new(agent.status, agent.session_id));
                            machine.set_relocating(relocating);
                            Outcome::Effects(machine.handle(
                                event,
                                session_id.as_deref(),
                                Instant::now(),
                            ))
                        }
                        _ => Outcome::UnknownAgent(event, session_id),
                    }
                }
            }
        };
        match outcome {
            Outcome::Effects(effects) => self.apply_status_effects(agent_id, effects),
            // Ids with no row are prewarmed sessions (buffer for replay at
            // adoption) or stale env / deleted agents (dropped, as before).
            Outcome::UnknownAgent(event, session_id) => {
                self.buffer_prewarm_hook(agent_id, event, session_id)
            }
        }
    }

    pub(super) fn buffer_prewarm_hook(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
    ) {
        let mut pool = self.prewarmed.lock();
        if let Some(entry) = pool.values_mut().find(|e| &e.agent_id == agent_id) {
            if entry.buffered_hooks.len() >= PREWARM_HOOK_BUFFER_CAP {
                entry.buffered_hooks.remove(0);
            }
            entry.buffered_hooks.push((event, session_id));
        }
    }

    /// Deferred-finish recheck across all machines (runs on a timer).
    pub fn tick_status_machines(&self) {
        let now = Instant::now();
        let ticked: Vec<(AgentId, Vec<Effect>)> = {
            let mut machines = self.status_machines.lock();
            machines
                .iter_mut()
                .map(|(id, m)| (id.clone(), m.tick(now)))
                .collect()
        };
        for (id, effects) in ticked {
            self.apply_status_effects(&id, effects);
        }
    }

    pub(super) fn apply_status_effects(&self, agent_id: &AgentId, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetStatus(status) => {
                    let (changed_at, unseen) = match self.store.set_agent_status(agent_id, status) {
                        Ok(stamped) => stamped,
                        Err(e) => {
                            tracing::warn!(error = %e, "persist status failed");
                            (nebula_core::clock::now_ms(), false)
                        }
                    };
                    self.broadcast(ServerEvent::StatusChanged {
                        agent: agent_id.clone(),
                        status,
                        changed_at,
                        unseen,
                    });
                    self.maybe_notify_status_change(agent_id, status);
                }
                Effect::SaveSessionId(sid) => {
                    if let Err(e) = self.store.set_agent_session_id(agent_id, Some(&sid)) {
                        tracing::warn!(error = %e, "persist session id failed");
                    }
                }
            }
        }
    }

    fn maybe_notify_status_change(&self, agent_id: &AgentId, status: AgentStatus) {
        let Some(kind) = crate::notifications::NotificationKind::of(status) else {
            return;
        };
        if !crate::config::Config::load().desktop_notifications {
            return;
        }
        if self.session_is_focused(&SessionRef::Agent(agent_id.clone())) {
            return;
        }
        let Ok(Some(agent)) = self.store.get_agent(agent_id) else {
            return;
        };
        let (project, branch) = self
            .store
            .get_worktree(&agent.worktree_id)
            .ok()
            .flatten()
            .map(|worktree| {
                let project = self
                    .store
                    .get_project(&worktree.project_id)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
                    .unwrap_or_default();
                (project, worktree.branch)
            })
            .unwrap_or_default();
        let place = match (project.is_empty(), branch.is_empty()) {
            (false, false) => format!("{project} - {branch}"),
            (true, false) => branch,
            _ => String::new(),
        };
        self.notifications.notify(
            agent_id,
            crate::notifications::Notification::new(kind, agent.name, place),
        );
    }
}
