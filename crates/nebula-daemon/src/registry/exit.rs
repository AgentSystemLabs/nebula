use super::*;

impl Daemon {
    pub(super) fn install_session(self: &Arc<Self>, session: Arc<PtySession>) {
        self.touch_session(&session.sref);
        self.sessions
            .lock()
            .insert(session.sref.clone(), session.clone());
        // After the insert: a forward task woken by this looks the ref up.
        let _ = self.session_installs.send(session.sref.clone());
        self.watch_for_exit(session);
    }

    /// Once the child dies: drop it from the registry, feed the status
    /// machine (agents), and tell subscribers the entity is no longer alive.
    pub(super) fn watch_for_exit(self: &Arc<Self>, session: Arc<PtySession>) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        let sref = session.sref.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { exit_code }) => {
                        // Deliberate kills (archive/restart/delete) remove the
                        // entry first — only a *natural* death of the still-
                        // registered session drives status, so a restart never
                        // flags the fresh PTY's agent as terminated.
                        let was_registered = {
                            let mut sessions = daemon.sessions.lock();
                            match sessions.get(&sref) {
                                Some(current) if Arc::ptr_eq(current, &session) => {
                                    sessions.remove(&sref);
                                    true
                                }
                                _ => false,
                            }
                        };
                        if was_registered {
                            daemon.session_interest.lock().remove(&sref);
                        }
                        // Taken on any exit, so a killed resume leaves no
                        // record behind; acted on for a natural death only.
                        let resume = match &sref {
                            SessionRef::Agent(id) => daemon.take_resume_watch(id, &session),
                            SessionRef::Terminal(_) => None,
                        };
                        if !was_registered {
                            break;
                        }
                        tracing::info!(session = ?sref, exit_code, "session exited");
                        // A run that ended on its own keeps its PTY for the
                        // attach that wants to read how it ended.
                        if let SessionRef::Terminal(id) = &sref {
                            if daemon.is_run_terminal(id) {
                                daemon
                                    .finished_runs
                                    .lock()
                                    .insert(id.clone(), (session.clone(), exit_code));
                            }
                        }
                        if let SessionRef::Agent(id) = &sref {
                            daemon.apply_hook_event(
                                id,
                                HookEvent::SessionEnded { exit_code },
                                None,
                            );
                        }
                        let upsert = match &sref {
                            SessionRef::Agent(id) => daemon.agent_entity(id).map(Entity::Agent),
                            SessionRef::Terminal(id) => {
                                daemon.terminal_entity(id).map(Entity::Terminal)
                            }
                        };
                        if let Ok(entity) = upsert {
                            daemon.broadcast(ServerEvent::EntityUpserted { entity });
                        }
                        if let (SessionRef::Agent(id), Some(watch)) = (&sref, resume) {
                            if exit_code.unwrap_or(1) != 0
                                && watch.spawned_at.elapsed() < RESUME_FAIL_WINDOW
                            {
                                daemon.respawn_failed_resume(
                                    id,
                                    watch.cols,
                                    watch.rows,
                                    watch.fresh_prompt,
                                );
                            }
                        }
                        break;
                    }
                    // The CLI's own busy/idle bit, read off its output. It is
                    // the only end-of-turn news after a user cancel: Claude
                    // Code fires no Stop for an interrupted turn, and
                    // suppresses the idle notification because the user just
                    // pressed a key. See `pty::progress`. Only while this PTY
                    // is still the session's: a killed CLI clears its bar as
                    // it dies, and that must not speak for the replacement a
                    // restart or relocation has installed by then.
                    Ok(PtyEvent::Progress { busy }) => {
                        if let SessionRef::Agent(id) = &sref {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                                // A cancelled turn's only word: a relocation
                                // waiting on this turn's end goes now, not
                                // at the end of the next one.
                                daemon.complete_pending_move(id, &HookEvent::Progress { busy });
                            }
                        }
                    }
                    // The window title carries Claude's session name, and
                    // `/rename` fires no hook — this is the cue to read the
                    // title it persisted (see `session_title`).
                    Ok(PtyEvent::Title { title }) => {
                        if let SessionRef::Agent(id) = &sref {
                            daemon.on_pty_title(id, &title);
                        }
                    }
                    // Cursor's ask-question dialog, which no hook reports
                    // (see `pty::question`). Owned PTYs only, as above.
                    Ok(PtyEvent::Question { open }) => {
                        if let SessionRef::Agent(id) = &sref {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Question { open }, None);
                            }
                        }
                    }
                    // The Cloud session this row launched, read off the
                    // `claude --cloud` output. Persisted at once — the child
                    // is typically gone within milliseconds of printing it —
                    // and re-broadcast so the row grows its `cloud` badge and
                    // its pane becomes the panel linking to the session.
                    Ok(PtyEvent::CloudTitle { title }) => {
                        // Claude Cloud's own name for the session names the
                        // row, when nothing else has: the agent runs where
                        // no hook reaches nebula, so the AUTO-TITLE a local
                        // session gives itself never comes (issue #92). A
                        // name the user typed stands.
                        if let SessionRef::Agent(id) = &sref {
                            let title = sanitize_title(&title);
                            if title.is_empty() {
                                continue;
                            }
                            let id_to_update = id.clone();
                            let title_to_update = title.clone();
                            match daemon
                                .store_blocking(move |store| {
                                    store.rename_agent_if_auto_pending(
                                        &id_to_update,
                                        &title_to_update,
                                    )
                                })
                                .await
                            {
                                Ok(true) => {
                                    tracing::info!(agent = %id, %title, "cloud session title adopted");
                                    daemon.try_broadcast_agent(id);
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    tracing::warn!(agent = %id, error = %e, "cloud session title not persisted")
                                }
                            }
                        }
                    }
                    Ok(PtyEvent::CloudSession { id: cloud_id }) => {
                        if let SessionRef::Agent(id) = &sref {
                            let id_to_update = id.clone();
                            let cloud_id_to_store = cloud_id.clone();
                            match daemon
                                .store_blocking(move |store| {
                                    store.set_agent_cloud_session_id(
                                        &id_to_update,
                                        Some(&cloud_id_to_store),
                                    )
                                })
                                .await
                            {
                                Ok(()) => {
                                    tracing::info!(agent = %id, cloud_session = %cloud_id, "cloud session id captured");
                                    daemon.try_broadcast_agent(id);
                                }
                                Err(e) => {
                                    tracing::warn!(agent = %id, error = %e, "cloud session id not persisted")
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // A fire-hosing child can push progress edges off the
                        // broadcast queue. The scanner itself never lags, so
                        // reconcile from its current reading rather than
                        // leaving the status stuck on a dropped edge.
                        if let (SessionRef::Agent(id), Some(busy)) =
                            (&sref, session.progress_busy())
                        {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                                daemon.complete_pending_move(id, &HookEvent::Progress { busy });
                            }
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
