use super::*;

impl Daemon {
    pub fn broadcast(&self, ev: ServerEvent) {
        let _ = self.events.send(ev);
    }

    pub fn session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.sessions.lock().get(sref).cloned()
    }

    pub fn is_alive(&self, sref: &SessionRef) -> bool {
        self.sessions.lock().contains_key(sref)
    }

    /// Whether `session` is still the PTY registered for `sref` — not one
    /// a kill (restart, move, relocation) has since replaced.
    pub(super) fn owns_session(&self, sref: &SessionRef, session: &Arc<PtySession>) -> bool {
        self.sessions
            .lock()
            .get(sref)
            .is_some_and(|current| Arc::ptr_eq(current, session))
    }

    /// (session, child pid, prewarm-pool home) for every live PTY — the
    /// metrics reading's input. A pool spare has no agent row, so the only
    /// way a client can name or place it is the home reported here.
    pub fn session_pids(&self) -> Vec<(SessionRef, u32, Option<PrewarmInfo>)> {
        // Snapshot the pool first and drop its lock: `prewarm_agent` holds
        // the pool lock while it asks the sessions map, so the two are
        // never held together here in the other order.
        let prewarmed: HashMap<AgentId, PrewarmInfo> = self
            .prewarmed
            .lock()
            .iter()
            .map(|((worktree, kind), e)| {
                (
                    e.agent_id.clone(),
                    PrewarmInfo {
                        worktree: worktree.clone(),
                        kind: *kind,
                        model: e.model.clone(),
                    },
                )
            })
            .collect();
        self.sessions
            .lock()
            .iter()
            .filter_map(|(sref, s)| {
                let pid = s.child_pid?;
                let prewarm = match sref {
                    SessionRef::Agent(id) => prewarmed.get(id).cloned(),
                    SessionRef::Terminal(_) => None,
                };
                Some((sref.clone(), pid, prewarm))
            })
            .collect()
    }

    pub fn remove_session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.session_interest.lock().remove(sref);
        self.sessions.lock().remove(sref)
    }

    pub fn kill_session(&self, sref: &SessionRef) {
        if let Some(s) = self.remove_session(sref) {
            s.kill();
        }
    }

    pub fn kill_all(&self) {
        for (_, s) in self.sessions.lock().drain() {
            s.kill();
        }
    }

    // ---- in-place restart ----

    /// Every live session, written down for the image an IN-PLACE RESTART
    /// execs into, with its master fd left open across the exec. A warm
    /// spare from the PREWARM POOL goes too, marked, so the new image reaps
    /// it rather than leaving an unreaped child behind.
    pub fn carry_sessions(&self) -> Result<Vec<crate::handoff::CarriedSession>> {
        let spares: std::collections::HashSet<AgentId> = self
            .prewarmed
            .lock()
            .values()
            .map(|e| e.agent_id.clone())
            .collect();
        let launching: std::collections::HashSet<AgentId> = self
            .status_machines
            .lock()
            .iter()
            .filter(|(_, m)| m.is_launching())
            .map(|(id, _)| id.clone())
            .collect();
        let sessions: Vec<Arc<PtySession>> = self.sessions.lock().values().cloned().collect();
        let mut carried = Vec::with_capacity(sessions.len());
        for session in sessions {
            let (spare, launching) = match &session.sref {
                SessionRef::Agent(id) => (spares.contains(id), launching.contains(id)),
                SessionRef::Terminal(_) => (false, false),
            };
            match session.carry() {
                Ok(Some(pty)) => carried.push(crate::handoff::CarriedSession {
                    pty,
                    spare,
                    launching,
                }),
                Ok(None) => {}
                Err(e) => {
                    self.uncarry_sessions();
                    return Err(e.context(format!("carry {:?}", session.sref)));
                }
            }
        }
        Ok(carried)
    }

    /// Keep [`Self::ensure_session`] from spawning while the guard lives.
    pub fn hold_spawns(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.spawn_gate.lock()
    }

    /// The exec [`Self::carry_sessions`] prepared for failed: close the
    /// master fds on exec again.
    pub fn uncarry_sessions(&self) {
        for session in self.sessions.lock().values() {
            session.uncarry();
        }
    }

    /// Take over what the previous image carried. Spares are reaped at
    /// once: the pool that would hand them out did not come along. Returns
    /// how many sessions are live here now.
    pub fn adopt_sessions(self: &Arc<Self>, carried: Vec<crate::handoff::CarriedSession>) -> usize {
        let mut adopted = 0;
        for crate::handoff::CarriedSession {
            pty,
            spare,
            launching,
        } in carried
        {
            let sref = pty.sref.clone();
            match PtySession::adopt(pty) {
                Ok(session) if spare => session.kill(),
                Ok(session) => {
                    if let (true, SessionRef::Agent(id)) = (launching, &sref) {
                        self.status_machines
                            .lock()
                            .insert(id.clone(), AgentStatusMachine::launching());
                    }
                    self.install_session(session);
                    adopted += 1;
                }
                Err(e) => tracing::warn!(session = ?sref, error = %e, "could not adopt session"),
            }
        }
        adopted
    }

    /// Relocations still waiting on their turn to end, for an IN-PLACE
    /// RESTART to carry. The boolean is whether the resumed session should
    /// open on the relocation notice (`nebula worktree`) or silently (a
    /// user-initiated card move).
    pub fn pending_moves(&self) -> Vec<(AgentId, Worktree, bool)> {
        self.pending_moves
            .lock()
            .iter()
            .map(|(id, pending)| (id.clone(), pending.target.clone(), pending.notice))
            .collect()
    }

    pub fn restore_pending_moves(&self, moves: Vec<(AgentId, Worktree, bool)>) {
        self.pending_moves.lock().extend(
            moves
                .into_iter()
                .map(|(id, target, notice)| (id, PendingMove { target, notice })),
        );
    }

    // ---- attach tracking & idle reaping ----

    /// A client attached to `sref` (the server dedupes re-attaches per
    /// connection). While any attachment exists, the session — and its
    /// whole worktree — counts as "in view".
    pub fn note_attached(&self, sref: &SessionRef) {
        *self.attach_counts.lock().entry(sref.clone()).or_insert(0) += 1;
        self.touch_session(sref);
    }

    /// A client detached from `sref` (or its connection dropped). Restamps
    /// the session so the idle clock starts at "stopped looking", not at
    /// spawn time.
    pub fn note_detached(&self, sref: &SessionRef) {
        let mut counts = self.attach_counts.lock();
        if let Some(n) = counts.get_mut(sref) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(sref);
            }
        }
        drop(counts);
        self.touch_session(sref);
    }

    /// Stamp `sref` as just-looked-at for the idle reaper.
    pub(super) fn touch_session(&self, sref: &SessionRef) {
        self.session_interest
            .lock()
            .insert(sref.clone(), Instant::now());
    }

    /// Kill idle sessions in worktrees no client is looking at, per
    /// `session_idle_timeout` — this bounds what prewarming and
    /// walked-away-from sessions cost. "In view" = the worktree holding any
    /// attached session; in-view sessions get their stamps refreshed
    /// instead, so the full timeout starts only when the user leaves.
    /// Spared regardless of age: agents that are running or waiting on
    /// feedback, agents with a backgrounded tool call still running (a job
    /// detached from their terminal — see `pty::detached_job_under`),
    /// terminals with a command running, and prewarm-pool sessions
    /// (`reap_prewarmed` owns those). A reaped session revives on the next
    /// attach or prewarm; agents resume their conversation.
    pub fn reap_idle_sessions(self: &Arc<Self>) {
        let Some(timeout) = crate::config::Config::load().session_idle_timeout() else {
            return;
        };
        let sessions: Vec<(SessionRef, Arc<PtySession>)> = self
            .sessions
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let attached: std::collections::HashSet<SessionRef> =
            self.attach_counts.lock().keys().cloned().collect();
        let viewed_worktrees: std::collections::HashSet<WorktreeId> = attached
            .iter()
            .filter_map(|sref| self.session_worktree(sref))
            .collect();
        let now = Instant::now();
        for (sref, session) in sessions {
            // No store row = prewarm-pool session (or deleted mid-sweep).
            let Some(worktree_id) = self.session_worktree(&sref) else {
                continue;
            };
            if attached.contains(&sref) || viewed_worktrees.contains(&worktree_id) {
                self.touch_session(&sref);
                continue;
            }
            let age = {
                let mut interest = self.session_interest.lock();
                // A missing stamp (session predating the map) starts aging now.
                now.duration_since(*interest.entry(sref.clone()).or_insert(now))
            };
            if age < timeout {
                continue;
            }
            let spared = match &sref {
                SessionRef::Agent(id) => match self.store.get_agent(id).ok().flatten() {
                    Some(agent)
                        if matches!(
                            agent.status,
                            AgentStatus::Running | AgentStatus::NeedsFeedback
                        ) =>
                    {
                        true
                    }
                    // A backgrounded tool call — Claude's `run_in_background`
                    // Bash, its Monitor watch, a Codex shell command —
                    // outlives the turn that started it, and the hook-fed
                    // status machine read that turn's Stop as Finished. The
                    // process tree still knows (#78). Restamped rather than
                    // just skipped: the job ending is the moment the agent
                    // wakes to read its result, so the clock starts there.
                    Some(_) => {
                        let busy = agent_has_detached_job(&session);
                        if busy {
                            self.touch_session(&sref);
                        }
                        busy
                    }
                    // Row vanished mid-sweep: its delete kills the PTY anyway.
                    None => true,
                },
                // A RUN TERMINAL is the server `r` started: sitting quiet
                // is what it is for, and only `r` again stops it.
                SessionRef::Terminal(id) => {
                    self.is_run_terminal(id) || shell_has_children(&session)
                }
            };
            if spared {
                continue;
            }
            tracing::info!(session = ?sref, idle_secs = age.as_secs(), "reaping idle session");
            self.kill_session(&sref);
            let upsert = match &sref {
                SessionRef::Agent(id) => self.agent_entity(id).map(Entity::Agent),
                SessionRef::Terminal(id) => self.terminal_entity(id).map(Entity::Terminal),
            };
            if let Ok(entity) = upsert {
                self.broadcast(ServerEvent::EntityUpserted { entity });
            }
        }
    }

    /// The worktree a session's row lives under; None when the row is gone
    /// or never existed (prewarm pool).
    pub(super) fn session_worktree(&self, sref: &SessionRef) -> Option<WorktreeId> {
        match sref {
            SessionRef::Agent(id) => self
                .store
                .get_agent(id)
                .ok()
                .flatten()
                .map(|a| a.worktree_id),
            SessionRef::Terminal(id) => self
                .store
                .get_terminal(id)
                .ok()
                .flatten()
                .map(|t| t.worktree_id),
        }
    }

    // ---- snapshot ----

    pub fn snapshot(&self) -> Result<ServerEvent> {
        let (projects, worktrees, mut agents, mut terminals) = self.store.load_tree()?;
        {
            let sessions = self.sessions.lock();
            for a in &mut agents {
                a.alive = sessions.contains_key(&SessionRef::Agent(a.id.clone()));
            }
            for t in &mut terminals {
                t.alive = sessions.contains_key(&SessionRef::Terminal(t.id.clone()));
            }
        }
        Ok(ServerEvent::Snapshot {
            projects,
            worktrees,
            agents,
            terminals,
            links: self.store.load_links()?,
            pr_seen: self.store.load_pr_seen()?,
            ui_state: self.store.load_ui_state()?,
        })
    }

    pub(super) fn agent_entity(&self, id: &AgentId) -> Result<Agent> {
        let mut agent = self.store.get_agent(id)?.context("agent not found")?;
        agent.alive = self.is_alive(&SessionRef::Agent(id.clone()));
        Ok(agent)
    }

    /// Push the agent's current row — liveness included —
    /// to every subscriber. The tail of every mutation that changes how
    /// the row renders; fails only when the row is gone.
    pub(super) fn broadcast_agent(&self, id: &AgentId) -> Result<()> {
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    /// [`Self::broadcast_agent`] for the best-effort sites — background
    /// tasks and post-respawn refreshes — where a row deleted meanwhile is
    /// not an error: nothing to show, so nothing to say.
    pub(crate) fn try_broadcast_agent(&self, id: &AgentId) {
        let _ = self.broadcast_agent(id);
    }

    pub(super) fn terminal_entity(&self, id: &TerminalId) -> Result<TerminalTab> {
        let mut term = self.store.get_terminal(id)?.context("terminal not found")?;
        term.alive = self.is_alive(&SessionRef::Terminal(id.clone()));
        Ok(term)
    }
}
