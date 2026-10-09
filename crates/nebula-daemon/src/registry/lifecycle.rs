use super::*;

impl Daemon {
    pub fn rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_agent(id, name.trim())?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// Agent-initiated one-shot title (`nebula rename` inside the session's
    /// CLI). Applies only while the auto-title is still pending; afterwards
    /// it reports the standing title as an error so the CLI (and the model
    /// reading its output) knows nothing changed.
    pub fn auto_rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        let title = sanitize_title(name);
        if title.is_empty() {
            bail!("title is empty");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if !self.store.rename_agent_if_auto_pending(id, &title)? {
            bail!(
                "session already has a title ({:?}); leaving it unchanged — a user-set \
                 title is only replaced with `nebula rename --force`",
                agent.name
            );
        }
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// `nebula worktree <branch>`, run by the agent inside its own session.
    /// The row moves under `branch`'s worktree of the same project now —
    /// created when the project has no checkout for that branch yet — and
    /// a live PTY follows once its turn ends (`complete_pending_move`),
    /// because the CLI running this command *is* that PTY's foreground
    /// tool call: killing it here would cut the turn off mid-answer.
    pub async fn enter_worktree(
        self: &Arc<Self>,
        id: &AgentId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<(Worktree, EnterOutcome)> {
        let branch = branch.trim();
        if branch.is_empty() {
            bail!("branch name is empty");
        }
        let agent_lookup = id.clone();
        let agent = self
            .store_blocking(move |store| store.get_agent(&agent_lookup))
            .await?
            .context("agent not found")?;
        if agent.archived {
            bail!("agent is archived");
        }
        let current_lookup = agent.worktree_id.clone();
        let current = self
            .store_blocking(move |store| store.get_worktree(&current_lookup))
            .await?
            .context("worktree not found")?;
        let target = self
            .worktree_on_branch(&current.project_id, branch, base)
            .await?;
        // A row whose checkout is gone (removed outside nebula, kept because
        // sessions hang off it) is no place to move to, and not one this
        // command can recreate: refuse before the row moves, so the agent is
        // never killed at turn end for a respawn that refuses.
        require_checkout(&target)?;
        if target.id == current.id {
            return Ok((target, EnterOutcome::AlreadyThere));
        }
        let alive = self.session(&SessionRef::Agent(id.clone())).is_some();
        // Same invalidation as `relocate_into`: every cwd this process
        // reports until it respawns is the old checkout's.
        self.last_cwd.lock().remove(id);
        if alive {
            self.pending_moves.lock().insert(
                id.clone(),
                PendingMove {
                    target: target.clone(),
                    notice: true,
                },
            );
        }
        let move_id = id.clone();
        let move_target = target.id.clone();
        self.store_blocking(move |store| store.set_agent_worktree(&move_id, &move_target))
            .await?;
        self.broadcast_agent(id)?;
        let outcome = if alive {
            EnterOutcome::Relocating
        } else {
            EnterOutcome::NextLaunch
        };
        Ok((target, outcome))
    }

    /// The user moved this session's card onto another checkout, of its
    /// own project or another one (the card menu's **Move to…**, or a drag
    /// onto another band or project tab). The row moves now. A live session mid-turn follows at that turn's
    /// end, as `nebula worktree` does but resumed silent: that turn never
    /// asked to move. An idle one is killed and resumed in the target at
    /// once, since no turn end is coming to wait for. A dead one boots
    /// there on its next launch.
    pub fn move_agent(self: &Arc<Self>, id: &AgentId, worktree: &WorktreeId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived");
        }
        let target = self
            .store
            .get_worktree(worktree)?
            .context("worktree not found")?;
        if target.id == agent.worktree_id {
            return Ok(());
        }
        let alive = self.session(&SessionRef::Agent(id.clone())).is_some();
        let mid_turn = matches!(
            agent.status,
            AgentStatus::Running | AgentStatus::NeedsFeedback
        );
        // Same invalidation as `enter_worktree`: every cwd this process
        // reports until it respawns is the old checkout's.
        self.last_cwd.lock().remove(id);
        if alive && mid_turn {
            self.pending_moves.lock().insert(
                id.clone(),
                PendingMove {
                    target: target.clone(),
                    notice: false,
                },
            );
        }
        self.store.set_agent_worktree(id, &target.id)?;
        self.broadcast_agent(id)?;
        if alive && !mid_turn {
            self.relocate_into(id, &target, false);
        }
        Ok(())
    }

    /// The turn an agent ran `nebula worktree` in has ended: make the
    /// process match its row. Kill it and respawn it resumed in the target,
    /// with a prompt naming the checkout it now runs in so the conversation
    /// carries straight on (Claude, codex and pi take that prompt as an
    /// argument; cursor resumes silent and waits for the user — see
    /// `relocation_prompt`). Gated on the turn-end signals — Stop, the idle
    /// notification a Stop-less end still fires, and the progress clear
    /// that is the only word of a cancelled turn — so a Bash hook from the
    /// same turn never triggers it.
    ///
    /// The status machine held that turn end at `running` for the
    /// relocation (`AgentStatusMachine::set_relocating`), so the card never
    /// drops to the bottom of the grid for the seconds until the respawned
    /// CLI's first hook: a respawn that opens on the notice is seeded as a
    /// launch, the way a create with a task is, and any other outcome — a
    /// silent respawn, a failed one, nothing left to respawn — lets the
    /// held end finish the turn after all.
    pub fn complete_pending_move(self: &Arc<Self>, id: &AgentId, event: &HookEvent) {
        let turn_over = match event {
            HookEvent::Stop => true,
            HookEvent::Notification { notification_type } => {
                notification_type.as_deref() == Some("idle_prompt")
            }
            HookEvent::Progress { busy } => !busy,
            _ => false,
        };
        if !turn_over {
            return;
        }
        let Some(PendingMove { target, notice }) = self.pending_moves.lock().remove(id) else {
            return;
        };
        if self.relocate_into(id, &target, notice) {
            // Working from the moment it boots, on the notice: seeded with
            // the launch reprieve so its startup progress-clear cannot
            // green it out before that turn begins (see `create_agent`).
            self.status_machines
                .lock()
                .insert(id.clone(), AgentStatusMachine::launching());
        } else {
            self.release_relocation_hold(id);
        }
    }

    /// The kill-and-respawn of [`Self::complete_pending_move`] and of an
    /// idle [`Self::move_agent`]. `notice` asks for the relocation notice.
    /// True when the respawn opened on it: the one outcome that carries on
    /// the turn the status machine held.
    pub(super) fn relocate_into(
        self: &Arc<Self>,
        id: &AgentId,
        target: &Worktree,
        notice: bool,
    ) -> bool {
        let agent = match self.store.get_agent(id) {
            Ok(Some(agent)) if !agent.archived && agent.worktree_id == target.id => agent,
            // Archived, deleted, or moved elsewhere by hand since: the
            // row's current home wins, nothing to relocate into.
            _ => return false,
        };
        let sref = SessionRef::Agent(id.clone());
        if self.session(&sref).is_none() {
            // Died since (or the user closed it): the next launch boots in
            // the target on its own, only without the relocation notice.
            return false;
        }
        // The target can vanish between `enter_worktree` and the turn end:
        // keep the running session rather than kill it for a boot that refuses.
        if let Err(e) = require_checkout(target) {
            tracing::warn!(agent = %id, error = %e, "relocation target gone; session left running");
            return false;
        }
        tracing::info!(agent = %id, to = %target.branch, "relocating session into its worktree");
        self.kill_session(&sref);
        self.last_cwd.lock().remove(id);
        // A row whose entry went missing since still relocates; the boot
        // itself refuses with the entry's reason, so the notice degrades
        // to none rather than failing the move.
        let prompt = resolve_harness(agent.kind, agent.custom_harness.as_deref())
            .map(|harness| relocation_prompt(notice && harness.relocation_prompt, target))
            .unwrap_or(None);
        let spawned = self.spawn_agent_session_with(
            &agent,
            target,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            None,
            prompt.as_deref(),
        );
        let continued = match spawned {
            Ok(_) => prompt.is_some(),
            Err(e) => {
                tracing::warn!(agent = %id, error = %e, "respawn after worktree relocation failed");
                false
            }
        };
        self.try_broadcast_agent(id);
        continued
    }

    /// No respawn is carrying the held turn on: the end the status machine
    /// held for the relocation lands now, as the Stop it was. A machine not
    /// mid-turn held nothing — the row finished or died the ordinary way —
    /// and is left alone.
    pub(super) fn release_relocation_hold(&self, id: &AgentId) {
        let effects = {
            let mut machines = self.status_machines.lock();
            let Some(machine) = machines.get_mut(id) else {
                return;
            };
            if !matches!(
                machine.status(),
                AgentStatus::Running | AgentStatus::NeedsFeedback
            ) {
                return;
            }
            machine.set_relocating(false);
            machine.handle(HookEvent::Stop, None, Instant::now())
        };
        self.apply_status_effects(id, effects);
    }

    /// Whether `id` is between `enter_worktree` and its respawn.
    #[cfg(test)]
    pub(super) fn relocation_pending(&self, id: &AgentId) -> bool {
        self.pending_moves.lock().contains_key(id)
    }

    /// Row-only re-home: store update plus broadcast, never the PTY. The
    /// hook-cwd reparent uses this — there the process already runs in the
    /// target checkout and only the row is stale, so killing it would
    /// interrupt a live conversation for nothing.
    pub(super) fn move_agent_row(
        self: &Arc<Self>,
        id: &AgentId,
        worktree_id: &WorktreeId,
    ) -> Result<()> {
        self.store.set_agent_worktree(id, worktree_id)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// A hook payload reported the agent CLI's working directory. When that
    /// directory sits inside a *different* worktree of the same project (the
    /// session entered a worktree it created mid-conversation), re-home the
    /// agent row so the tree reflects where the work actually happens.
    /// Fail-soft: any error leaves the row where it is.
    pub fn reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) {
        if let Err(e) =
            self.try_reparent_agent_by_cwd(agent_id, cwd, payload_session_id, captures_session)
        {
            tracing::warn!(agent = %agent_id, error = %e, "cwd reparent failed");
        }
    }

    pub(super) fn try_reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) -> Result<()> {
        let Some(agent) = self.store.get_agent(agent_id)? else {
            self.last_cwd.lock().remove(agent_id);
            return Ok(());
        };
        if agent.archived {
            self.last_cwd.lock().remove(agent_id);
            return Ok(());
        }
        // Mid-relocation the row already sits under the target while the
        // process still reports the old checkout — ignore it until the
        // respawn lands there.
        if self.pending_moves.lock().contains_key(agent_id) {
            return Ok(());
        }
        // Same foreign-session rule as the status machine: a payload from a
        // different CLI session only counts when the event (re)establishes
        // session ownership (UserPromptSubmit / SessionStart).
        if !captures_session {
            if let (Some(mine), Some(theirs)) = (agent.session_id.as_deref(), payload_session_id) {
                if mine != theirs {
                    return Ok(());
                }
            }
        }
        let cwd = canonical_or_raw(Path::new(cwd));
        // Remembered even when it resolves to nothing: an agent that just ran
        // `git worktree add` and stepped into the result reports a cwd nebula
        // has no row for yet, and the worktree sync replays this to finish the
        // re-home the moment that row is adopted.
        self.last_cwd.lock().insert(agent_id.clone(), cwd.clone());
        self.reparent_agent_to_cwd(&agent, &cwd)
    }

    /// Move `agent`'s row under the worktree owning `cwd` when that is a
    /// different worktree of the same project. `cwd` must already be
    /// canonicalized.
    pub(super) fn reparent_agent_to_cwd(self: &Arc<Self>, agent: &Agent, cwd: &Path) -> Result<()> {
        let Some(current) = self.store.get_worktree(&agent.worktree_id)? else {
            return Ok(());
        };
        let (_, worktrees, _, _) = self.store.load_tree()?;
        // Deepest worktree of the same project containing cwd — nested
        // layouts (checkouts under the repo root) must not resolve to the
        // root row just because the root path is also a prefix.
        let target = worktrees
            .into_iter()
            .filter(|w| w.project_id == current.project_id)
            .map(|w| {
                let canonical = canonical_or_raw(&w.path);
                (w, canonical)
            })
            .filter(|(_, canonical)| cwd.starts_with(canonical))
            .max_by_key(|(_, canonical)| canonical.components().count());
        if let Some((worktree, _)) = target {
            if worktree.id != agent.worktree_id {
                tracing::info!(
                    agent = %agent.id,
                    from = %current.branch,
                    to = %worktree.branch,
                    "agent re-homed by hook cwd"
                );
                self.move_agent_row(&agent.id, &worktree.id)?;
            }
        }
        Ok(())
    }

    /// Replay remembered hook cwds for `project`'s agents. Runs after the
    /// worktree sync adopts checkouts: a session that creates a worktree and
    /// enters it reports the new cwd (often on the very next `Stop`) before
    /// the row exists, and without this replay its row would sit under the
    /// old checkout until the user's next prompt.
    pub(super) fn reparent_agents_by_last_cwd(self: &Arc<Self>, project: &Project) {
        let known: Vec<(AgentId, PathBuf)> = {
            let map = self.last_cwd.lock();
            map.iter().map(|(id, p)| (id.clone(), p.clone())).collect()
        };
        for (agent_id, cwd) in known {
            let agent = match self.store.get_agent(&agent_id) {
                Ok(Some(agent)) => agent,
                Ok(None) => {
                    self.last_cwd.lock().remove(&agent_id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!(agent = %agent_id, error = %e, "cwd replay lookup failed");
                    continue;
                }
            };
            if agent.archived {
                continue;
            }
            let in_project = matches!(
                self.store.get_worktree(&agent.worktree_id),
                Ok(Some(w)) if w.project_id == project.id
            );
            if !in_project {
                continue;
            }
            if let Err(e) = self.reparent_agent_to_cwd(&agent, &cwd) {
                tracing::warn!(agent = %agent_id, error = %e, "cwd replay reparent failed");
            }
        }
    }

    pub fn archive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.set_agent_archived(id, true)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    pub fn unarchive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.store.set_agent_archived(id, false)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// A client put this agent's session on screen: its unseen-finish flag
    /// (`Agent::unseen`) is cleared, and every subscriber gets the row so
    /// their counts drop together. Nothing is sent when the flag was
    /// already clear — re-attaching to a session you've read is free.
    pub fn mark_agent_seen(&self, id: &AgentId) -> Result<()> {
        if self.store.mark_agent_seen(id)? {
            self.broadcast_agent(id)?;
        }
        Ok(())
    }

    pub fn delete_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.last_cwd.lock().remove(id);
        self.pending_moves.lock().remove(id);
        self.store.delete_agent(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Agent(id.clone()),
        });
        Ok(())
    }

    pub async fn restart_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        // A Cloud row has no local session to restart: the agent runs in
        // the cloud sandbox, and a plain restart would boot a bare CLI with
        // no link to the work. The row's pane says where the session is.
        if agent.cloud_session_id.is_some() {
            bail!("{CLOUD_ROW_NO_LOCAL_SESSION}");
        }
        let worktree_lookup = agent.worktree_id.clone();
        let worktree = self
            .store_blocking(move |store| store.get_worktree(&worktree_lookup))
            .await?
            .context("worktree not found")?;
        // Before the kill: a session still running in a deleted checkout
        // would otherwise be stopped for a boot that refuses.
        require_checkout(&worktree)?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(())
    }

    /// Queue a message on a Cloud session without leaving nebula.
    /// `claude -p <msg> --cloud <id>` is fire-and-forget — the CLI prints
    /// "Sent to cloud session." and returns, and the reply only ever shows
    /// up on the session's page in the browser, which the row's pane links
    /// to. Runs in the row's own checkout like every other CLI call; the
    /// cloud sandbox is where the work happens, so nothing here switches a
    /// branch or touches the tree.
    pub async fn send_cloud_message(self: &Arc<Self>, id: &AgentId, message: &str) -> Result<()> {
        let agent_lookup = id.clone();
        let agent = self
            .store_blocking(move |store| store.get_agent(&agent_lookup))
            .await?
            .context("agent not found")?;
        let Some(cloud_id) = agent.cloud_session_id.clone() else {
            bail!("session was not launched in Claude Cloud");
        };
        let message = validate_cloud_text(message, "message")?;
        let worktree_lookup = agent.worktree_id.clone();
        let worktree = self
            .store_blocking(move |store| store.get_worktree(&worktree_lookup))
            .await?
            .context("worktree not found")?;

        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        let (program, args) = match cmd_override.as_deref() {
            Some(over) => (over.to_string(), Vec::new()),
            None => login_shell_wrap(
                &nebula_core::shell::user_shell(),
                "claude",
                &[
                    "-p".to_string(),
                    message.clone(),
                    format!("--cloud={cloud_id}"),
                ],
            ),
        };
        let output = tokio::process::Command::new(&program)
            .args(&args)
            .current_dir(&worktree.path)
            .output()
            .await
            .context("run claude -p --cloud")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim().lines().last().unwrap_or("").to_string();
            bail!(
                "claude could not reach the cloud session{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        tracing::info!(agent = %id, cloud_session = %cloud_id, bytes = message.len(), "message sent to cloud session");
        // Its next prompt, for the card: the sandbox reports no hook for
        // it, so this is where nebula learns it was asked.
        if let Some(text) = crate::prompt_history::condense(&message) {
            self.record_prompt(id, text);
        }
        Ok(())
    }
}
