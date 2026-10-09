use super::*;

impl Daemon {
    // ---- terminals ----

    pub fn create_terminal(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: Option<String>,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let n = self.store.count_terminals(worktree_id).unwrap_or(0);
            format!("term-{}", n + 1)
        });
        let terminal = TerminalTab {
            id: TerminalId::generate(),
            worktree_id: worktree_id.clone(),
            name,
            sort_order: 0,
            alive: false,
            run_command: None,
        };
        self.store.insert_terminal(&terminal)?;
        self.spawn_terminal_session(&terminal, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_term = terminal.clone();
        broadcast_term.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(broadcast_term),
        });
        Ok(EntityId::Terminal(terminal.id))
    }

    pub fn rename_terminal(self: &Arc<Self>, id: &TerminalId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_terminal(id, name.trim())?;
        let term = self.terminal_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(term),
        });
        Ok(())
    }

    pub fn close_terminal(self: &Arc<Self>, id: &TerminalId) -> Result<()> {
        self.kill_session(&SessionRef::Terminal(id.clone()));
        self.finished_runs.lock().remove(id);
        self.store.delete_terminal(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Terminal(id.clone()),
        });
        Ok(())
    }

    // ---- run terminals ----

    /// `r` on a worktree: start its RUN COMMAND — the project's
    /// `run_command` setting (Settings → Project), else `.nebula.json`'s
    /// `run`, read fresh from the worktree's checkout, else the main
    /// checkout's — in the worktree's RUN TERMINAL. A run that already
    /// exited lends its row; one still going is the answer as it stands,
    /// so a second client's `r` never starts a second server.
    pub fn start_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<EntityId> {
        self.start_run_with(worktree_id, &crate::config::Config::load())
    }

    /// [`Daemon::start_run`] against a given config, for the tests that
    /// can't pin the settings file.
    pub fn start_run_with(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        config: &crate::config::Config,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        // Held across the check and the spawn, like `ensure_session`: two
        // presses racing must produce one run, not two.
        let _gate = self.spawn_gate.lock();
        let existing = self.store.run_terminals_in(worktree_id)?.into_iter().next();
        if let Some(term) = &existing {
            if self.is_alive(&SessionRef::Terminal(term.id.clone())) {
                return Ok(EntityId::Terminal(term.id.clone()));
            }
        }
        let main = self
            .store
            .get_project(&worktree.project_id)?
            .map_or_else(|| worktree.path.clone(), |p| p.repo_path);
        let command = match config.run_command(&main) {
            Some(command) => command.to_string(),
            None => project_file::lookup(&worktree.path, &main, ProjectCommand::Run)
                .map_err(anyhow::Error::msg)?
                .context(NO_RUN_COMMAND)?,
        };
        let mut term = match existing {
            Some(mut term) => {
                self.store.set_terminal_run_command(&term.id, &command)?;
                term.run_command = Some(command.clone());
                term
            }
            None => {
                let term = TerminalTab {
                    id: TerminalId::generate(),
                    worktree_id: worktree_id.clone(),
                    name: RUN_TERMINAL_NAME.into(),
                    sort_order: 0,
                    alive: false,
                    run_command: Some(command.clone()),
                };
                self.store.insert_terminal(&term)?;
                term
            }
        };
        self.finished_runs.lock().remove(&term.id);
        let spawned = self.spawn_terminal_session(&term, &worktree, DEFAULT_COLS, DEFAULT_ROWS);
        tracing::info!(worktree = %worktree_id, %command, ok = spawned.is_ok(), "run started");
        let id = term.id.clone();
        {
            // Stamped and sent under the sessions lock: a command that exits
            // at once is dropped from the map, and its not-alive upsert sent,
            // only after this one — never the other way round, which would
            // leave every client showing a finished run as still going.
            let sessions = self.sessions.lock();
            term.alive = sessions.contains_key(&SessionRef::Terminal(id.clone()));
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Terminal(term),
            });
        }
        spawned?;
        Ok(EntityId::Terminal(id))
    }

    /// `r` on a running worktree: kill its RUN TERMINAL and drop the row, so
    /// a stopped run leaves nothing behind. Nothing running is not an
    /// error — two clients may both have pressed it.
    pub fn stop_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<()> {
        for term in self.store.run_terminals_in(worktree_id)? {
            tracing::info!(worktree = %worktree_id, "run stopped");
            self.close_terminal(&term.id)?;
        }
        Ok(())
    }

    /// Whether `id` is a RUN TERMINAL's row.
    pub(super) fn is_run_terminal(&self, id: &TerminalId) -> bool {
        self.store
            .get_terminal(id)
            .ok()
            .flatten()
            .is_some_and(|t| t.run_command.is_some())
    }

    /// A RUN TERMINAL that exited on its own: its exit code (None when the
    /// OS reported none), for the attach replaying it. None for a live
    /// session or anything that is not a finished run.
    pub fn finished_run_exit(&self, sref: &SessionRef) -> Option<Option<i32>> {
        let SessionRef::Terminal(id) = sref else {
            return None;
        };
        self.finished_runs.lock().get(id).map(|(_, code)| *code)
    }

    // ---- links ----

    pub fn update_link(self: &Arc<Self>, id: &LinkId, url: &str) -> Result<()> {
        let url = normalize_url(url)?;
        self.store.set_link_url(id, &url)?;
        let link = self.store.get_link(id)?.context("link not found")?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Link(link),
        });
        Ok(())
    }

    pub fn delete_link(self: &Arc<Self>, id: &LinkId) -> Result<()> {
        self.store.delete_link(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Link(id.clone()),
        });
        Ok(())
    }
}
