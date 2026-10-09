use super::*;

impl Daemon {
    // ---- agents ----

    /// The refusals [`Self::create_agent`] would give a cold launch on a
    /// STARTING PROMPT — the prompt, the harness, a missing CLI — asked
    /// ahead of it, for a caller about to do something costly first
    /// (`nebula spawn --worktree` cuts a checkout) that the create could
    /// then refuse. A miss is re-probed, so the create's own check is the
    /// cached hit this leaves behind.
    pub(crate) async fn check_cold_launch(
        &self,
        kind: AgentKind,
        custom_harness: Option<&str>,
        starting_prompt: &str,
    ) -> Result<()> {
        validate_starting_prompt(starting_prompt)?;
        let harness = resolve_harness(kind, custom_harness)?;
        let program = harness.program.trim();
        if !self.cli_available_for_create(program).await {
            bail!("{}", cli_missing_message(program));
        }
        Ok(())
    }

    pub(crate) async fn create_agent(self: &Arc<Self>, spec: CreateAgentSpec) -> Result<EntityId> {
        let CreateAgentSpec {
            worktree: worktree_id,
            name,
            kind,
            custom_harness,
            model,
            effort,
            auto_title,
            cloud_prompt,
            starting_prompt,
            pr_url,
            issue_url,
        } = spec;
        let cloud_prompt = match cloud_prompt {
            Some(_) if kind != AgentKind::Claude => {
                bail!("cloud launch is only supported for Claude")
            }
            Some(prompt) => {
                let prompt = validate_cloud_text(&prompt, "task")?;
                Some(prompt)
            }
            None => None,
        };
        let starting_prompt = match starting_prompt {
            Some(_) if cloud_prompt.is_some() => {
                bail!("a starting prompt is not supported for Claude Cloud")
            }
            Some(prompt) => Some(validate_starting_prompt(&prompt)?),
            None => None,
        };
        let pr_url = match pr_url {
            Some(_) if cloud_prompt.is_some() => {
                bail!("PR launch context is not supported for Claude Cloud")
            }
            Some(url) => Some(crate::pr_scope::validate_pr_url(&url)?),
            None => None,
        };
        let issue_url = match issue_url {
            Some(_) if cloud_prompt.is_some() => {
                bail!("issue launch context is not supported for Claude Cloud")
            }
            Some(url) => Some(crate::pr_scope::validate_issue_url(&url)?),
            None => None,
        };
        // Every harness resolves against the current registry before
        // anything spawns: a missing or broken entry refuses the create
        // with its reason, and a deleted entry breaks respawns the same
        // way at boot.
        let harness = resolve_harness(kind, custom_harness.as_deref())?;
        let program = harness.program.trim().to_string();
        // A launch that hands the CLI a first prompt is working from the
        // moment it spawns, so the row says so now instead of staying gray
        // until the CLI has booted and its first hook has landed — seconds
        // in which the session the user just started looked idle and sorted
        // under every session mid-turn.
        let optimistic_run = Self::launch_submits_first_prompt(
            &harness,
            starting_prompt.as_deref(),
            pr_url.is_some() || issue_url.is_some(),
        ) && cloud_prompt.is_none();
        let worktree_lookup = worktree_id.clone();
        let worktree = self
            .store_blocking(move |store| {
                store
                    .get_worktree(&worktree_lookup)?
                    .context("worktree not found")
            })
            .await?;
        // A warm session for this (worktree, kind) hands over its PTY and
        // its pre-generated id — the CLI booted while the user typed the
        // name, so the create feels instant. A starting prompt rides the
        // CLI's argv, and a spare already booted bare cannot be handed one;
        // neither can it be handed a PR or issue rule.
        let adopted = (cloud_prompt.is_none()
            && pr_url.is_none()
            && issue_url.is_none()
            && starting_prompt.is_none())
        .then(|| self.take_prewarmed(&worktree_id, kind, model.as_deref(), effort.as_deref()))
        .flatten();
        // Only the cold path needs asking: an adopted warm session is proof
        // the CLI runs. Without this, a missing CLI still "succeeds" — the
        // login shell prints `command not found` into a PTY that dies at
        // once, leaving a dead row that looks identical to a fresh one.
        if adopted.is_none() && !self.cli_available_for_create(&program).await {
            bail!("{}", cli_missing_message(&program));
        }
        let agent = Agent {
            id: adopted
                .as_ref()
                .map(|e| e.agent_id.clone())
                .unwrap_or_else(AgentId::generate),
            worktree_id,
            name: if name.trim().is_empty() {
                "agent".into()
            } else {
                name.trim().to_string()
            },
            status: if optimistic_run {
                AgentStatus::Running
            } else {
                AgentStatus::Fresh
            },
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: custom_harness
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            model,
            effort,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: nebula_core::clock::now_ms(),
            alive: false,
            issue_url: issue_url.clone(),
            recent_prompts: Vec::new(),
        };
        // A cloud task is the session's first prompt, and the only one
        // nebula will ever see typed for it: the agent runs in the sandbox,
        // where no `UserPromptSubmit` hook reaches the daemon. Recorded
        // here, the card says what the session was asked to do like any
        // other card's does (issue #92).
        let mut agent = agent;
        let prompt_entry = cloud_prompt
            .as_deref()
            .and_then(crate::prompt_history::condense)
            .map(|text| nebula_core::PromptEntry {
                text,
                submitted_at: nebula_core::clock::now_ms(),
            });
        let agent_to_insert = agent.clone();
        let pr_to_insert = pr_url.clone();
        let issue_to_insert = issue_url.clone();
        let prompt_to_insert = prompt_entry.clone();
        self.store_blocking(move |store| {
            store.insert_agent_with_launch_context(
                &agent_to_insert,
                auto_title,
                pr_to_insert.as_deref(),
                issue_to_insert.as_deref(),
            )?;
            if let Some(entry) = &prompt_to_insert {
                store.push_prompt(&agent_to_insert.id, entry)?;
            }
            Ok(())
        })
        .await?;
        if let Some(entry) = prompt_entry {
            agent.recent_prompts.push(entry);
        }
        let agent = agent;
        if optimistic_run {
            // Seeded by hand, ahead of the spawn, so the CLI's own startup
            // progress-clear cannot green the row out before its turn has
            // begun — and so nothing seeds a plain `running` machine from
            // the row first.
            self.status_machines
                .lock()
                .insert(agent.id.clone(), AgentStatusMachine::launching());
        }
        if adopted.is_none() {
            // Cold path: boot the CLI right away.
            let spawned = self.spawn_agent_session_with(
                &agent,
                &worktree,
                DEFAULT_COLS,
                DEFAULT_ROWS,
                cloud_prompt.as_deref(),
                starting_prompt.as_deref(),
            );
            self.rollback_agent_on_spawn_error(&agent.id, spawned)?;
        }
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        if let Some(entry) = adopted {
            // Now that the row exists, replay the hooks the warm CLI fired
            // before adoption (SessionStart stores the resume session id).
            for (event, sid) in entry.buffered_hooks {
                self.apply_hook_event(&agent.id, event, sid);
            }
        }
        Ok(EntityId::Agent(agent.id))
    }

    /// Does a cold spawn of this launch hand the CLI a first prompt — the
    /// turn that makes a created row `running` before a single hook has
    /// fired? Asked of the two places that decide it, so it cannot drift
    /// from them: [`crate::pr_scope::launch_prompts`], which folds a launch
    /// rule (a PR SESSION's, an ISSUE SESSION's) into the first prompt on a
    /// harness with no system-prompt flag, and the argv builder's prepend
    /// shape, which puts nebula's guidance there
    /// ([`agent_spawn_command_with`]). The rule's text is built per spawn
    /// from the checkout; only whether there *is* one matters here, so a
    /// stand-in stands in for it.
    pub(super) fn launch_submits_first_prompt(
        harness: &nebula_core::harness::HarnessDescriptor,
        starting_prompt: Option<&str>,
        rule: bool,
    ) -> bool {
        harness.system.prepend_to_first_prompt
            || crate::pr_scope::launch_prompts(
                harness.system.append_flag.is_some(),
                // A row being created has no session to resume.
                false,
                rule.then_some("<rule>"),
                starting_prompt,
            )
            .initial
            .is_some()
    }

    pub(super) fn rollback_agent_on_spawn_error<T>(
        &self,
        id: &AgentId,
        result: Result<T>,
    ) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(spawn_error) => {
                self.status_machines.lock().remove(id);
                if let Err(rollback_error) = self.store.delete_agent(id) {
                    return Err(spawn_error.context(format!(
                        "agent spawn failed and its database rollback also failed: {rollback_error:#}"
                    )));
                }
                Err(spawn_error)
            }
        }
    }

    // ---- attach / spawn ----

    /// Get the live session for an entity, lazily (re)spawning its PTY when
    /// none is running (restored agents, closed shells).
    pub fn ensure_session(
        self: &Arc<Self>,
        sref: &SessionRef,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        if let Some(s) = self.session(sref) {
            return Ok(s);
        }
        // Hold the gate across the whole check-and-install: an Attach and the
        // prewarm sweep racing the same dead session must produce one CLI,
        // not two. Re-check under it — the winner installed while we waited.
        let _gate = self.spawn_gate.lock();
        if let Some(s) = self.session(sref) {
            return Ok(s);
        }
        match sref {
            SessionRef::Agent(id) => {
                let agent = self.store.get_agent(id)?.context("agent not found")?;
                if agent.archived {
                    bail!("agent is archived — unarchive it first");
                }
                // A Cloud row's only PTY is the `claude --cloud <task>`
                // create, gone seconds after it prints the session id. There
                // is nothing to bring back: the agent runs in the cloud, and
                // a spawn here would be a bare local CLI wearing its name.
                if agent.cloud_session_id.is_some() {
                    bail!("{CLOUD_ROW_NO_LOCAL_SESSION}");
                }
                let worktree = self
                    .store
                    .get_worktree(&agent.worktree_id)?
                    .context("worktree not found")?;
                let session = self.spawn_agent_session(&agent, &worktree, cols, rows)?;
                let mut broadcast_agent = agent;
                broadcast_agent.alive = true;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Agent(broadcast_agent),
                });
                Ok(session)
            }
            SessionRef::Terminal(id) => {
                let term = self.store.get_terminal(id)?.context("terminal not found")?;
                // A RUN TERMINAL is only ever started by `r`: an attach, or
                // the prewarm sweep walking past, must never run a command
                // that exited again. What it can show is how the run ended,
                // while the DAEMON still holds that PTY.
                if term.run_command.is_some() {
                    if let Some((session, _)) = self.finished_runs.lock().get(id) {
                        return Ok(session.clone());
                    }
                    bail!("{RUN_NOT_RUNNING}");
                }
                let worktree = self
                    .store
                    .get_worktree(&term.worktree_id)?
                    .context("worktree not found")?;
                let session = self.spawn_terminal_session(&term, &worktree, cols, rows)?;
                let mut broadcast_term = term;
                broadcast_term.alive = true;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Terminal(broadcast_term),
                });
                Ok(session)
            }
        }
    }

    pub(super) fn spawn_agent_session(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        self.spawn_agent_session_with(agent, worktree, cols, rows, None, None)
    }

    /// The general spawn: `cloud_task` makes it a Claude Cloud dispatch
    /// (`claude --cloud <task>`, which creates the session, prints its id
    /// and exits), `initial_prompt` a first turn the CLI submits on its own
    /// (the relocation notice a `nebula worktree` respawn opens with, or the
    /// prefix + task + postfix an AGENT PRESET launch composes). Both
    /// are intentionally transient: later restarts/resumes follow the
    /// persisted Agent fields — a Cloud row's `cloud_session_id` makes
    /// `restart_agent` and `ensure_session` refuse to boot a local CLI for
    /// it, everything else takes the plain local-session path.
    pub(super) fn spawn_agent_session_with(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        cloud_task: Option<&str>,
        initial_prompt: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        // A session the user sent to Claude's background (`/background`)
        // can't be resumed, only attached to — see `claude_bg`. The probe
        // costs a login shell, so it hides behind the one-`stat` hint.
        let attach = if cloud_task.is_none() && self.claude_job_hint(agent) {
            self.claude_background_id(agent)
        } else {
            None
        };
        self.spawn_agent_pty(
            agent,
            worktree,
            cols,
            rows,
            cloud_task,
            initial_prompt,
            attach.as_deref(),
        )
    }

    /// [`Self::spawn_agent_session_with`] past its look for a backgrounded
    /// Claude session: `attach` is the id `claude attach` takes, and wins
    /// over a resume of the stored session id — and over `initial_prompt`,
    /// which `attach` has no way to submit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_agent_pty(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        cloud_task: Option<&str>,
        initial_prompt: Option<&str>,
        attach: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        // Before anything touches the path: the hook install below creates
        // its directories, so a spawn into a deleted checkout would recreate
        // it as an empty folder and resume the agent there.
        require_checkout(worktree)?;
        // Whatever spawns this agent, it runs in `worktree` from here: a
        // relocation still pending for it has been overtaken.
        self.pending_moves.lock().remove(&agent.id);
        // Every row resolves its registry descriptor once, up front: a row
        // whose entry was deleted or broken since refuses the boot with
        // its reason rather than launching the wrong CLI, and the same
        // descriptor picks the hook dialect below.
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref())?;
        // Managed status hooks; a failure here degrades to "no status
        // updates", never blocks the spawn. The dialect is data: a custom
        // harness naming one reports status, prompts and permission waits
        // exactly like that harness, and a harness with none runs
        // hookless (process-based status until a dialect is mapped).
        let install_result = match harness.hook_dialect() {
            Some(AgentKind::Claude) => hooks::installer::install_claude_hooks(&worktree.path),
            // Codex's hooks live in its home, not the worktree, so one
            // trust approval covers every worktree (see installer docs);
            // any per-worktree copy an older nebula left is pruned.
            Some(AgentKind::Codex) => {
                hooks::installer::install_codex_hooks(&hooks::installer::codex_home())
                    .and_then(|()| hooks::installer::prune_codex_worktree_hooks(&worktree.path))
            }
            // Cursor also gets the managed auto-title project rule — its
            // hook dialect has no context-injection channel.
            Some(AgentKind::Cursor) => hooks::installer::install_cursor_hooks(&worktree.path)
                .and_then(|()| hooks::installer::install_cursor_title_rule(&worktree.path)),
            // Pi runs TypeScript extensions, not shell hooks: one managed
            // extension in its global agent dir (loaded without the trust
            // prompt a worktree-local `.pi/extensions/` would raise) serves
            // every worktree.
            Some(AgentKind::Pi) => {
                hooks::pi_extension::install(&hooks::pi_extension::pi_agent_dir())
            }
            // OpenCode runs TypeScript plugins, likewise: one managed
            // plugin in its global config dir (globbed at startup, no
            // trust prompt) serves every worktree.
            Some(AgentKind::OpenCode) => {
                hooks::opencode_plugin::install(&hooks::opencode_plugin::opencode_config_dir())
            }
            _ => Ok(()),
        };
        if let Err(e) = install_result {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }

        // NEBULA_AGENT_CMD overrides for tests; default is the kind's CLI.
        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        // A Claude session id with no transcript behind it — a CLI nobody
        // sent a prompt, or a session Claude's cleanup has deleted — resumes
        // into "No conversation found" and a dead pane: boot fresh instead.
        // An override (tests) never resumes, so it skips the look.
        let unresumable;
        let agent = match agent.session_id.as_deref() {
            Some(sid)
                if agent.kind == AgentKind::Claude
                    && cloud_task.is_none()
                    && cmd_override.is_none()
                    && attach.is_none()
                    && claude_transcript_exists(&self.claude_projects_dirs(), sid)
                        == Some(false) =>
            {
                tracing::info!(agent = %agent.id, session = %sid, "no Claude transcript for the session — spawning fresh");
                if let Err(e) = self.store.set_agent_session_id(&agent.id, None) {
                    tracing::warn!(agent = %agent.id, error = %e, "clear session id failed");
                }
                let mut fresh = agent.clone();
                fresh.session_id = None;
                unresumable = fresh;
                &unresumable
            }
            _ => agent,
        };
        // A PR SESSION's rule — or an ISSUE SESSION's — rides Claude's
        // system prompt, or opens a Codex / Cursor cold spawn as its first
        // prompt (see `pr_scope`). Rebuilt from the row's *current*
        // worktree on every spawn, so a relocated session is told where it
        // now works.
        let (pr_url, issue_url) = if cloud_task.is_none() {
            (
                self.store.agent_pr_url(&agent.id)?,
                self.store.agent_issue_url(&agent.id)?,
            )
        } else {
            (None, None)
        };
        let root = match &pr_url {
            Some(_) if !worktree.is_main => self
                .store
                .get_project(&worktree.project_id)?
                .map(|p| p.repo_path),
            _ => None,
        };
        let scope = pr_url.as_deref().map(|url| crate::pr_scope::PrScope {
            url,
            worktree: &worktree.path,
            branch: &worktree.branch,
            root: root.as_deref(),
        });
        let issue_scope = issue_url.as_deref().map(|url| crate::pr_scope::IssueScope {
            url,
            worktree: &worktree.path,
            branch: &worktree.branch,
        });
        let rule = crate::pr_scope::combined_rule(scope.as_ref(), issue_scope.as_ref());
        let prompts = crate::pr_scope::launch_prompts(
            harness.system.append_flag.is_some(),
            agent.session_id.is_some(),
            rule.as_deref(),
            initial_prompt,
        );
        let (program, args, resumed) = match (cloud_task, attach) {
            (Some(task), _) => claude_cloud_spawn_command(
                &harness,
                task,
                agent.model.as_deref(),
                agent.effort.as_deref(),
                cmd_override.as_deref(),
            ),
            // Never a watched resume: an attach that dies at once keeps the
            // row's session id, and the pane keeps the CLI's reason.
            (None, Some(id)) => {
                if prompts.initial.is_some() {
                    tracing::warn!(agent = %agent.id, "backgrounded Claude session — `claude attach` takes no prompt, dropping the initial one");
                }
                (
                    harness.program.trim().to_string(),
                    claude_bg::attach_args(id),
                    false,
                )
            }
            (None, None) => agent_spawn_command_with(
                &harness,
                agent.session_id.as_deref(),
                Some(&worktree.path),
                agent.model.as_deref(),
                agent.effort.as_deref(),
                cmd_override.as_deref(),
                prompts.initial.as_deref(),
                prompts.system.as_deref(),
                true,
            ),
        };
        // Run the agent through the user's login+interactive shell so it sees
        // the same env as a Terminal.app tab (~/.zprofile, ~/.zshrc,
        // path_helper) instead of the daemon's inherited-at-boot env, and
        // resolves the CLI the way a typed command would — an alias or
        // function in those files wins over the binary on PATH.
        // Overrides (tests) stay verbatim.
        let (program, args) = if cmd_override.is_some() {
            (program, args)
        } else {
            login_shell_wrap(&nebula_core::shell::user_shell(), &program, &args)
        };

        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![
                (env::AGENT_ID.into(), agent.id.to_string()),
                (
                    env::API_URL.into(),
                    format!("http://127.0.0.1:{}", self.hook_env.port),
                ),
                (env::API_TOKEN.into(), self.hook_env.token.clone()),
            ],
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
        };
        let sref = SessionRef::Agent(agent.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        // Recorded before the install, so a CLI that dies at once still
        // finds its watch when `watch_for_exit` sees it go.
        {
            let mut resumes = self.resumes.lock();
            if resumed {
                resumes.insert(
                    agent.id.clone(),
                    ResumeWatch {
                        session: Arc::downgrade(&session),
                        spawned_at: Instant::now(),
                        cols,
                        rows,
                    },
                );
            } else {
                resumes.remove(&agent.id);
            }
        }
        self.install_session(session.clone());
        // The create prints the session id and exits at once: capture it
        // off the output (`watch_for_exit` persists it and re-broadcasts the
        // row), which is what turns the row's pane into the link panel.
        if cloud_task.is_some() {
            session.arm_cloud_scan();
        }
        if agent.kind == AgentKind::Cursor {
            session.arm_question_scan();
        }
        Ok(session)
    }

    /// A resumed session (`claude --resume` / `codex resume` /
    /// `cursor-agent --resume`) that died inside [`RESUME_FAIL_WINDOW`]
    /// could not find its session: clear the id and boot fresh instead of
    /// leaving a dead pane. Reached from `watch_for_exit` on a natural death
    /// only, so a restart or relocation that killed the PTY on purpose (and
    /// has respawned it already) never gets a second CLI. (`pi --session-id`
    /// creates a missing id instead of dying, so pi never lands here.)
    pub(super) fn respawn_failed_resume(self: &Arc<Self>, id: &AgentId, cols: u16, rows: u16) {
        // `ensure_session`'s gate: an Attach reaching for the dead session
        // right now must not fork a CLI beside this one.
        let _gate = self.spawn_gate.lock();
        if self.is_alive(&SessionRef::Agent(id.clone())) {
            return; // respawned already, and that spawn is watched itself
        }
        // Archived or deleted inside the window: never resurrect those.
        let Ok(Some(mut agent)) = self.store.get_agent(id) else {
            return;
        };
        if agent.archived || agent.cloud_session_id.is_some() {
            return;
        }
        let Some(sid) = agent.session_id.take() else {
            return;
        };
        // A Claude transcript still on disk says the id is good and the CLI
        // quit over something else — a bad flag, a login: keep the id for
        // the next attach, and the pane keeps the CLI's reason.
        if agent.kind == AgentKind::Claude
            && claude_transcript_exists(&self.claude_projects_dirs(), &sid) == Some(true)
        {
            // One reason Claude refuses a good id: the session runs in its
            // background daemon now, and only `claude attach` opens it. The
            // spawn's own look missed it — its job-dir hint is Claude's
            // private layout, free to move — so this one asks outright.
            agent.session_id = Some(sid);
            if let Some(attach) = self.claude_background_id(&agent) {
                let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
                    return;
                };
                tracing::info!(agent = %id, attach = %attach, "resume refused for a backgrounded session — attaching");
                if self
                    .spawn_agent_pty(&agent, &worktree, cols, rows, None, None, Some(&attach))
                    .is_ok()
                {
                    agent.alive = true;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Agent(agent),
                    });
                }
                return;
            }
            tracing::info!(agent = %id, "resume failed fast with its transcript intact — keeping the session id");
            return;
        }
        let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
            return;
        };
        tracing::info!(agent = %id, "resume failed fast — respawning fresh");
        if let Err(e) = self.store.set_agent_session_id(id, None) {
            tracing::warn!(agent = %id, error = %e, "clear session id failed");
        }
        if self
            .spawn_agent_session(&agent, &worktree, cols, rows)
            .is_ok()
        {
            agent.alive = true;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            });
        }
    }

    /// The resume watch `session` was spawned under, while it is still the
    /// agent's latest — taken, so each is judged once.
    pub(super) fn take_resume_watch(
        &self,
        id: &AgentId,
        session: &Arc<PtySession>,
    ) -> Option<ResumeWatch> {
        let mut resumes = self.resumes.lock();
        match resumes.get(id) {
            Some(watch) if std::ptr::eq(watch.session.as_ptr(), Arc::as_ptr(session)) => {
                resumes.remove(id)
            }
            _ => None,
        }
    }

    /// Whether Claude keeps a background job under `agent`'s session id —
    /// the cheap look that earns [`Self::claude_background_id`] its probe.
    /// The job dirs sit beside the projects dirs, in the same config dir.
    pub(super) fn claude_job_hint(&self, agent: &Agent) -> bool {
        let Some(sid) = claude_resumable_session(agent) else {
            return false;
        };
        self.claude_projects_dirs()
            .iter()
            .filter_map(|projects| projects.parent())
            .any(|config| claude_bg::job_hint(config, sid))
    }

    /// The id `claude attach` takes for `agent`'s session, when `claude
    /// agents --json` lists it as a background session. The listing runs
    /// through the login shell the agent itself would: the same `claude`,
    /// the same `CLAUDE_CONFIG_DIR`.
    pub(super) fn claude_background_id(&self, agent: &Agent) -> Option<String> {
        let sid = claude_resumable_session(agent)?;
        let listing = ["agents".to_string(), "--json".to_string()];
        let (program, args) = login_shell_wrap(
            &nebula_core::shell::user_shell(),
            agent.kind.cli_program(),
            &listing,
        );
        let id = claude_bg::probe(&program, &args, sid)?;
        tracing::info!(agent = %agent.id, session = %sid, attach = %id, "Claude session runs in the background");
        Some(id)
    }

    /// Every Claude projects dir a transcript may sit in: the ones this
    /// daemon's Claude hooks reported (wherever the agent's shell pointed
    /// `CLAUDE_CONFIG_DIR`), then the default.
    pub(super) fn claude_projects_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .transcripts
            .lock()
            .values()
            .filter_map(|t| Some(t.transcript_path.parent()?.parent()?.to_path_buf()))
            .collect();
        dirs.extend(nebula_core::paths::claude_config_dir().map(|dir| dir.join("projects")));
        dirs.sort();
        dirs.dedup();
        dirs
    }

    pub(super) fn spawn_terminal_session(
        self: &Arc<Self>,
        terminal: &TerminalTab,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        require_checkout(worktree)?;
        let (program, args) = match &terminal.run_command {
            // A RUN TERMINAL runs its command line through the login +
            // interactive shell an agent launch uses, so `npm` or `bun`
            // resolve the way they do typed. The PTY lives exactly as long
            // as the command, which is what makes it the RUNNING state.
            Some(command) => login_shell_line(&nebula_core::shell::user_shell(), command),
            // `-l` makes it a login shell, matching Terminal.app: zsh then
            // sources /etc/zprofile (path_helper), ~/.zprofile, and ~/.zshrc.
            None => (nebula_core::shell::user_shell(), vec!["-l".into()]),
        };
        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![],
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
        };
        let sref = SessionRef::Terminal(terminal.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        Ok(session)
    }
}
