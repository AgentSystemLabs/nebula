use super::*;

impl Daemon {
    // ---- prewarm pool ----

    /// Pre-spawn an agent CLI for (worktree, kind) so the next create adopts
    /// an already-booted session. Fail-soft by design: a disabled config,
    /// missing CLI, or spawn error just means the create stays cold.
    pub async fn prewarm_agent(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<()> {
        if !crate::config::Config::load().prewarm_agents {
            return Ok(());
        }
        if kind == AgentKind::Custom {
            // No warm spares for custom harnesses: the pool is keyed by
            // kind alone and a spare booted for one entry must never be
            // adopted by another. Custom creates stay cold.
            return Ok(());
        }
        let worktree_lookup = worktree_id.clone();
        let Some(worktree) = self
            .store_blocking(move |store| store.get_worktree(&worktree_lookup))
            .await?
        else {
            return Ok(());
        };
        let stale = {
            // One warm slot per key; keep a live, young one with the same
            // spec, replace a dead, wrong-spec, or aging one (recycling
            // before the reaper hits keeps a re-requested slot gap-free).
            let mut pool = self.prewarmed.lock();
            if let Some(entry) = pool.get(&(worktree_id.clone(), kind)) {
                if self.is_alive(&SessionRef::Agent(entry.agent_id.clone()))
                    && entry.model == model
                    && entry.effort == effort
                    && entry.spawned_at.elapsed() < PREWARM_RECYCLE_AGE
                {
                    return Ok(());
                }
                pool.remove(&(worktree_id.clone(), kind))
            } else {
                None
            }
        };
        if let Some(old) = stale {
            self.kill_session(&SessionRef::Agent(old.agent_id));
        }
        if !self.cli_available(kind.cli_program()).await {
            tracing::debug!(kind = kind.as_str(), "prewarm skipped: CLI not installed");
            return Ok(());
        }
        let agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree_id.clone(),
            name: "prewarm".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: None,
            model: model.clone(),
            effort: effort.clone(),
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            issue_url: None,
            recent_prompts: Vec::new(),
        };
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        tracing::info!(agent = %agent.id, kind = kind.as_str(), worktree = %worktree.branch, "prewarmed agent session");
        let replaced = self.prewarmed.lock().insert(
            (worktree_id.clone(), kind),
            PrewarmEntry {
                agent_id: agent.id,
                spawned_at: Instant::now(),
                model,
                effort,
                buffered_hooks: Vec::new(),
            },
        );
        // Two racing prewarms for the same key: the loser's session would
        // otherwise leak as an orphan CLI process.
        if let Some(old) = replaced {
            self.kill_session(&SessionRef::Agent(old.agent_id));
        }
        Ok(())
    }

    /// Pop the warm entry for (worktree, kind) if its PTY is still running
    /// and it booted with the requested model/effort. A dead entry (CLI
    /// missing/crashed while warm) is dropped, a wrong-spec one is killed;
    /// either way the caller falls back to a cold spawn.
    pub(super) fn take_prewarmed(
        &self,
        worktree_id: &WorktreeId,
        kind: AgentKind,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Option<PrewarmEntry> {
        let entry = self.prewarmed.lock().remove(&(worktree_id.clone(), kind))?;
        if !self.is_alive(&SessionRef::Agent(entry.agent_id.clone())) {
            return None;
        }
        if entry.model.as_deref() != model || entry.effort.as_deref() != effort {
            self.kill_session(&SessionRef::Agent(entry.agent_id));
            return None;
        }
        Some(entry)
    }

    /// Drop warm sessions that died or sat unclaimed past the max age —
    /// and, once `prewarm_agents` is switched off, every one of them: a
    /// spare is a real CLI process the user can see (Claude's `/list-agents`
    /// names it beside their own sessions), so the toggle takes it away on
    /// the next sweep rather than leaving it to age out over 15 minutes.
    /// Runs on the daemon's periodic tick.
    pub fn reap_prewarmed(&self) {
        self.reap_prewarmed_with(&crate::config::Config::load());
    }

    pub(super) fn reap_prewarmed_with(&self, config: &crate::config::Config) {
        let doomed: Vec<AgentId> = {
            let mut pool = self.prewarmed.lock();
            let expired: Vec<_> = pool
                .iter()
                .filter(|(_, e)| {
                    !config.prewarm_agents
                        || e.spawned_at.elapsed() > PREWARM_MAX_AGE
                        || !self.is_alive(&SessionRef::Agent(e.agent_id.clone()))
                })
                .map(|(k, _)| k.clone())
                .collect();
            expired
                .into_iter()
                .filter_map(|k| pool.remove(&k))
                .map(|e| e.agent_id)
                .collect()
        };
        for id in doomed {
            tracing::debug!(agent = %id, "reaping prewarmed session");
            self.kill_session(&SessionRef::Agent(id));
        }
    }

    /// Kill every live agent and terminal PTY homed in these worktrees, and
    /// the warm spares with them — the prelude to dropping their rows
    /// (worktree delete, project remove).
    pub(super) fn kill_sessions_in(
        &self,
        worktree_ids: &[WorktreeId],
        agents: &[Agent],
        terminals: &[TerminalTab],
    ) {
        for a in agents
            .iter()
            .filter(|a| worktree_ids.contains(&a.worktree_id))
        {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals
            .iter()
            .filter(|t| worktree_ids.contains(&t.worktree_id))
        {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
            self.finished_runs.lock().remove(&t.id);
        }
        self.kill_prewarmed_in(worktree_ids);
    }

    /// Kill warm sessions homed in any of these worktrees (worktree delete,
    /// project remove — their store rows are gone or going).
    pub(super) fn kill_prewarmed_in(&self, worktree_ids: &[WorktreeId]) {
        let doomed: Vec<AgentId> = {
            let mut pool = self.prewarmed.lock();
            let keys: Vec<_> = pool
                .keys()
                .filter(|(w, _)| worktree_ids.contains(w))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|k| pool.remove(&k))
                .map(|e| e.agent_id)
                .collect()
        };
        for id in doomed {
            self.kill_session(&SessionRef::Agent(id));
        }
    }

    /// Is the harness's CLI on the user's PATH (as their login shell sees
    /// it)? Cached by program: hits for an hour, misses for a minute so a
    /// just-installed CLI gets picked up quickly. Probe trouble (timeout,
    /// spawn error) fails open — a doomed warm spawn is still graceful.
    /// Custom harnesses pass their entry's program; built-ins their CLI.
    pub(super) async fn cli_available(&self, program: &str) -> bool {
        if std::env::var(env::AGENT_CMD).is_ok() {
            return true; // test override is spawned verbatim
        }
        const OK_TTL: Duration = Duration::from_secs(3600);
        const FAIL_TTL: Duration = Duration::from_secs(60);
        {
            let probes = self.cli_probes.lock();
            if let Some((ok, at)) = probes.get(program) {
                if at.elapsed() < if *ok { OK_TTL } else { FAIL_TTL } {
                    return *ok;
                }
            }
        }
        self.probe_cli(program).await
    }

    /// Fill the availability cache for every harness at boot, off the
    /// request loop. Without it the first CreateAgent of a session pays a
    /// full login-shell probe (~1s with a heavy ~/.zshrc) before it can
    /// answer. Custom entries resolve against the current config; a bare
    /// `Custom` kind never reaches the probe — it has no program.
    pub async fn warm_cli_probes(self: &Arc<Self>) {
        // One probe per launchable program in the effective registry, so
        // a repointed program warms the binary that actually launches.
        let mut programs = Vec::new();
        for entry in harness_registry() {
            if entry.problem().is_none() && !programs.contains(&entry.program) {
                programs.push(entry.program.clone());
            }
        }
        for program in programs {
            self.cli_available(program.trim()).await;
        }
    }

    /// Same question, asked on behalf of a create the user just triggered.
    /// A cached *hit* is trusted; a cached *miss* is re-probed, so someone who
    /// installs the CLI and immediately retries isn't told for another minute
    /// that it's missing. Misses are rare, so this costs nothing in practice.
    pub(super) async fn cli_available_for_create(&self, program: &str) -> bool {
        self.cli_available(program).await || self.probe_cli(program).await
    }

    /// Uncached `command -v` through the user's login shell; caches the answer.
    pub(super) async fn probe_cli(&self, program: &str) -> bool {
        let check = cli_probe_line(program);
        let mut probe = tokio::process::Command::new(nebula_core::shell::user_shell());
        probe
            .args(LOGIN_SHELL_ARGS)
            .arg(&check)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // A timed-out probe must die with the dropped future, not linger.
            .kill_on_drop(true);
        // Own session: the interactive shell must not reach the daemon's
        // controlling terminal (--foreground runs have one). zsh's job-control
        // init opens /dev/tty and makes itself the foreground process group,
        // SIGTTIN-stopping whatever TUI owns that terminal.
        unsafe {
            probe.pre_exec(own_session);
        }
        let status = tokio::time::timeout(CLI_PROBE_TIMEOUT, probe.status()).await;
        match status {
            Ok(Ok(status)) => {
                let ok = status.success();
                self.cli_probes
                    .lock()
                    .insert(program.to_string(), (ok, Instant::now()));
                ok
            }
            _ => true,
        }
    }

    /// Boot every dead, non-archived session under `worktree_id` (agents and
    /// terminals) so a later Attach replays an already-running screen.
    /// Already-alive sessions pass through ensure_session untouched; one
    /// session failing to spawn (missing CLI, deleted checkout) is logged
    /// and doesn't stop the rest.
    pub fn prewarm_worktree_sessions(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        if !crate::config::Config::load().prewarm_sessions {
            return;
        }
        let daemon = self.clone();
        let worktree_id = worktree_id.clone();
        let handle = tokio::spawn(async move {
            daemon.run_worktree_prewarm(&worktree_id, cols, rows).await;
        });
        // Supersede whatever sweep was still warming the worktree the user
        // has now left; its remaining boots are wasted work.
        if let Some(old) = self.prewarm_sweep.lock().replace(handle) {
            old.abort();
        }
    }

    /// The sweep itself: boot the worktree's dead sessions one at a time,
    /// [`PREWARM_STAGGER`] apart. Deliberately off the connection's request
    /// loop — it used to run inline, which stalled that client's Input and
    /// Attach frames for as long as the whole burst of forks took.
    pub(super) async fn run_worktree_prewarm(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        let tree = self.store_blocking(|store| store.load_tree()).await;
        let Ok((_, _, agents, terminals)) = tree else {
            return;
        };
        let srefs: Vec<SessionRef> = agents
            .iter()
            .filter(|a| &a.worktree_id == worktree_id && !a.archived)
            .map(|a| SessionRef::Agent(a.id.clone()))
            .chain(
                terminals
                    .iter()
                    // A RUN TERMINAL never boots from a sweep (see
                    // `ensure_session`).
                    .filter(|t| &t.worktree_id == worktree_id && t.run_command.is_none())
                    .map(|t| SessionRef::Terminal(t.id.clone())),
            )
            .collect();
        for sref in srefs {
            // The prewarm doubles as a "user is looking here" signal for
            // the idle reaper, for alive sessions as much as fresh spawns.
            self.touch_session(&sref);
            // Already warm — most importantly the one the user just
            // attached to, which Attach spawned a moment ago.
            if self.is_alive(&sref) {
                continue;
            }
            let daemon = self.clone();
            let target = sref.clone();
            // fork/exec blocks; keep it off the async worker threads.
            let spawned = tokio::task::spawn_blocking(move || {
                daemon.ensure_session(&target, cols, rows).map(|_| ())
            })
            .await;
            match spawned {
                Ok(Err(e)) => {
                    tracing::debug!(session = ?sref, error = %e, "session prewarm failed")
                }
                Err(e) => tracing::debug!(session = ?sref, error = %e, "session prewarm panicked"),
                Ok(Ok(())) => {}
            }
            tokio::time::sleep(PREWARM_STAGGER).await;
        }
    }
}
