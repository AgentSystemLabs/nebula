//! Extracted event-loop helper section.

use super::*;

macro_rules! main_loop_body {
    ($terminal:ident, $channels:ident) => {{

    let mut app = App::new();
    app.chrome.conn = ConnState::Connected;
    // The repo nebula was started in, for the first run's "open this
    // folder" — looked up once, off the tree the snapshot has not sent yet.
    app.launcher.launch_repo = crate::app::launch_repo();
    let cfg = crate::config::Config::load();
    apply_config(&mut app, &cfg);
    app.chrome.keymap = cfg.keymap();
    // Every pull request the last run knew about, painted before the
    // daemon's snapshot even lands; the lookups below refresh them all in
    // the background (`pr_cache`).
    app.github.pr_cache = Some(crate::pr_cache::PrCache::default_location());
    crate::pr_cache::hydrate(&mut app);
    // A screenshot dropped onto a prompt box is copied here the moment it
    // lands, before macOS deletes the file behind its thumbnail.
    app.modals.attachments_dir = Some(crate::dropped_files::default_dir());
    // The Cursor MODEL / EFFORT lists: cached `cursor-agent --list-models`
    // now, a background refresh when the cache is a day old.
    crate::cursor_catalogue::bootstrap(cfg.cursor_enabled);
    // The Claude MODEL list: Claude Code's own `availableModels` allowlist
    // when its settings carry one, so an org-restricted machine offers the
    // ids the CLI will accept instead of aliases it refuses.
    crate::claude_catalogue::bootstrap(cfg.claude_enabled);
    let mut input = crossterm::event::EventStream::new();
    let mut out: Vec<ClientRequest> = Vec::new();
    // Pointer shape last sent to the $terminal (OSC 22), so hover over a
    // splitter swaps the cursor once instead of on every motion event.
    let mut pointer_sent = PointerShape::default();
    let mut directory_sent = None;
    let mut next_draw = tokio::time::Instant::now();
    // When the loop may paint again (FRAME PACING): back to back for a key
    // and its answer, 60 fps under sustained output.
    let mut pacer = pacing::FramePacer::new(next_draw);
    let mut next_git_poll = tokio::time::Instant::now();
    // The changed-file badge's `git status`, run off the loop; the count
    // lands here and in `app.jobs.git_changes`.
    let (git_tx, mut git_rx) = tokio::sync::mpsc::unbounded_channel::<ChangedFiles>();
    // The cards' sweep of every other checkout the grid lists, one per
    // tick; its counts land in `app.jobs.worktree_changes` (and its line counts
    // in `app.jobs.worktree_lines`) and nowhere else.
    let (sweep_git_tx, mut sweep_git_rx) = tokio::sync::mpsc::unbounded_channel::<SweptChanges>();
    // Pull-request lookups run off the loop (they hit the network); answers
    // come back here and land in `app.github.pull_requests`.
    let (pr_tx, mut pr_rx) = tokio::sync::mpsc::unbounded_channel::<(WorktreeId, Lookup)>();
    // The selected project's open-pull-request list, on the same off-loop
    // footing. `None` is "couldn't ask", which keeps the last good list.
    let (prs_tx, mut prs_rx) = tokio::sync::mpsc::unbounded_channel::<(
        nebula_core::ProjectId,
        Option<Vec<crate::pull_request::OpenPr>>,
    )>();
    // One pull request's body and conversation, for the preview pane.
    let (detail_tx, mut detail_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Option<crate::pull_request::PrDetail>)>();
    // A whole `gh pr diff`, which opens the diff modal when it lands — or
    // refreshes the one already open on the cached copy.
    let (prdiff_tx, mut prdiff_rx) = tokio::sync::mpsc::unbounded_channel::<PrDiffAnswer>();
    app.github.pr_diff_tx = Some(prdiff_tx);
    // A finished `gh pr comment`, which flashes its outcome when it lands
    // — and re-reads the pull request so the pane shows what was said.
    let (prcomment_tx, mut prcomment_rx) =
        tokio::sync::mpsc::unbounded_channel::<PrCommentAnswer>();
    app.github.pr_comment_tx = Some(prcomment_tx);
    // The ISSUES MODAL's `gh issue list` / `gh issue view` answers, on the
    // same footing: the modal's own handlers start the fetch, the loop
    // lands it.
    let (issues_tx, mut issues_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::issues::IssuesAnswer>();
    app.github.issues_tx = Some(issues_tx);
    // The BRANCH SWITCHER's git — the listing, the changed-file count,
    // the background fetch and the switch itself — lands here too.
    let (branch_tx, mut branch_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::branch_switch::Answer>();
    app.jobs.branch_switch.tx = Some(branch_tx);
    // BACKGROUND READS for the worktree views: the git and the disk behind
    // `g`, `f`, `F` and `b` run on the blocking pool and land here.
    let (views_tx, mut views_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::view_jobs::Answer>();
    app.jobs.view_jobs = Some(crate::view_jobs::Jobs::new(views_tx));
    // A newer nebula published on GitHub, probed off the loop at start and
    // then on a slow beat (`update_check::interval`; the e2e tests turn it
    // off). Only a newer version ever arrives, so the footer's indicator,
    // once lit, survives a check that later can't ask.
    let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let update_interval = crate::update_check::interval();
    let mut next_update_check = tokio::time::Instant::now();
    let mut next_metrics_poll = tokio::time::Instant::now();
    let mut next_tail_poll = tokio::time::Instant::now();
    let mut next_splash_frame = tokio::time::Instant::now();
    let mut next_sweep_frame = tokio::time::Instant::now();
    // The last sweep tick still had something to animate (see the tick).
    let mut sweep_was_ticking = false;
    let mut next_ago_refresh = tokio::time::Instant::now() + AGO_REFRESH;
    // The host's runtime modes, re-asked on a slow beat (see host_$terminal).
    let mut next_mode_reassert = tokio::time::Instant::now() + MODE_REASSERT;
    // Editor-modal PTY output; the channel outlives individual editor
    // spawns (VimEvent generations keep them apart).
    let (vim_tx, mut vim_rx) = tokio::sync::mpsc::unbounded_channel::<VimEvent>();
    app.pane.vim_tx = Some(vim_tx);
    // The INPUT LATENCY PROBE (`NEBULA_PERF_LOG`); None outside a
    // measurement run.
    let mut perf = crate::perf::Perf::from_env();

    loop {
        if app.chrome.dirty && tokio::time::Instant::now() >= next_draw {
            // A selection change must never paint another checkout's badge:
            // the badge stays off until this checkout's count lands, and
            // between selections the slow poll keeps it fresh.
            if app.git_changes_stale() {
                request_git_changes(&mut app, &git_tx);
            }
            let began = std::time::Instant::now();
            draw_frame($terminal, &mut app)?;
            if let Some(perf) = &mut perf {
                perf.frame(began, &app);
            }
            app.chrome.dirty = false;
            next_draw = pacer.drew(tokio::time::Instant::now(), began.elapsed());
            sync_pty_size(&mut app, &mut out);
            sync_vim_size(&mut app);
        }

        let focus_before = app.nav.focus;
        let preview_before = app.reading_url();
        // When the KEY COMBO DISPLAY's last press comes down (its arm below).
        let key_combo_deadline = app
            .chrome.key_combo
            .as_ref()
            .map_or_else(tokio::time::Instant::now, |combo| {
                tokio::time::Instant::from_std(combo.deadline())
            });
        // When the EDGE AUTO-SCROLL next steps (its arm below).
        let drag_autoscroll_deadline = app
            .pane.next_drag_autoscroll
            .map_or_else(tokio::time::Instant::now, tokio::time::Instant::from_std);
        tokio::select! {
            // Pending redraw: wake at the frame boundary even if no new
            // events arrive.
            _ = tokio::time::sleep_until(next_draw), if app.chrome.dirty => {}
            // Fixed deadline (not a fresh sleep per iteration) so heavy PTY
            // traffic can't starve the badge refresh.
            _ = tokio::time::sleep_until(next_git_poll) => {
                request_git_changes(&mut app, &git_tx);
                sweep_git_changes(&mut app, &sweep_git_tx);
                // Rides the git tick rather than the repaint, so walking the
                // worktree list with j/k can't spawn a `gh` per row passed —
                // only whatever the selection is resting on when it fires,
                // plus one of the project's other checkouts if its turn has
                // come.
                lookup_pull_request(&mut app, &pr_tx);
                sweep_pull_request(&mut app, &pr_tx);
                lookup_open_prs(&mut app, &prs_tx, &mut out);
                sweep_open_prs(&mut app, &prs_tx, &mut out);
                // The selected project's open issues, on the same beat, so
                // `i` paints rows that are at most a couple of minutes old.
                crate::issues::refresh_selected(&mut app);
                // And one other project's — the pass that keeps every
                // row's count warm, `sweep_open_prs` for issues.
                crate::issues::sweep_others(&mut app);
                // Whatever the answers above changed since the last tick
                // goes to disk, off the loop; the next launch paints from it.
                if let Some((cache, store, live)) = crate::pr_cache::take_flush(&mut app) {
                    tokio::task::spawn_blocking(move || {
                        crate::pr_cache::write_all(&cache, &store, &live)
                    });
                }
                next_git_poll = tokio::time::Instant::now() + GIT_POLL;
            }
            // `Shift+R` asked for the pull requests now: the same lookups
            // the git tick runs, on this turn instead of up to `GIT_POLL`
            // later.
            _ = std::future::ready(()), if app.github.pr_refresh_requested => {
                app.github.pr_refresh_requested = false;
                lookup_pull_request(&mut app, &pr_tx);
                sweep_pull_request(&mut app, &pr_tx);
                lookup_open_prs(&mut app, &prs_tx, &mut out);
            }
            // Metrics poll: always on for the footer's memory/session
            // readout, tightened while the metrics modal is open (its
            // initial reading is requested by the M keypress itself).
            _ = tokio::time::sleep_until(next_metrics_poll) => {
                request_metrics(&mut app, &mut out);
                let period = if matches!(app.modals.overlay, Some(Overlay::Metrics(_))) {
                    METRICS_POLL
                } else {
                    FOOTER_METRICS_POLL
                };
                next_metrics_poll = tokio::time::Instant::now() + period;
            }
            // The grid's TERMINAL cards: what each shell last printed, asked
            // on a beat of its own for as long as any is on screen — with
            // the grid folded away or no $terminal drawn there is nothing
            // to ask after, and the beat sleeps with it.
            _ = tokio::time::sleep_until(next_tail_poll), if !app.pane.tail_cards.is_empty() => {
                request_terminal_tails(&mut app, &mut out);
                next_tail_poll = tokio::time::Instant::now() + TAIL_POLL;
            }
            // First-run splash, or the empty grid's welcome drawn over the
            // same sky: while either is on screen nothing else repaints an
            // idle app, so tick the animation on a fixed cadence.
            _ = tokio::time::sleep_until(next_splash_frame), if app.splash_active() || app.welcome_active() => {
                app.chrome.dirty = true;
                next_splash_frame = tokio::time::Instant::now() + SPLASH_FRAME;
            }
            // Status sweep: running / needs-feedback rows shimmer, so keep
            // repainting while any are visible (same pure-function-of-time
            // model as the splash — a missed tick skips ahead cleanly). A
            // ONE-SHOT SWEEP ends on the clock, with no event to repaint
            // the row it leaves mid-band, so the tick after the last sweep
            // stops still fires: that frame is the row settling.
            _ = tokio::time::sleep_until(next_sweep_frame), if app.status_anim_active() || sweep_was_ticking => {
                app.chrome.dirty = true;
                sweep_was_ticking = app.status_anim_active();
                next_sweep_frame = tokio::time::Instant::now() + SWEEP_FRAME;
            }
            // "23m ago" labels age on their own with nothing else to
            // repaint an idle app. Only worth a frame once some row carries
            // one — a project or worktree row reads any session under it,
            // so the whole tree is the test, not just the visible sessions.
            _ = tokio::time::sleep_until(next_ago_refresh) => {
                if app.tree.agents.iter().any(|a| a.status_changed_at > 0) {
                    app.chrome.dirty = true;
                }
                next_ago_refresh = tokio::time::Instant::now() + AGO_REFRESH;
            }
            // The KEY COMBO DISPLAY clears itself a moment after the press
            // (`key_combo::LINGER`) — nothing else repaints an idle app, so
            // the deadline takes a wake of its own.
            _ = tokio::time::sleep_until(key_combo_deadline), if app.chrome.key_combo.is_some() => {
                app.chrome.key_combo = None;
                app.chrome.dirty = true;
            }
            // A host that reset itself (iTerm2's ⌘R) drops mouse reporting
            // without a word, and a dead mouse can't report that it is
            // dead — so re-ask on a beat rather than on a signal.
            // …but never while the left button is down. The re-ask is the
            // one thing nebula writes to the host on a clock, and it is a
            // mode change under a gesture in progress: a host that takes
            // it as a fresh start drops the button, and the selection
            // stops growing a couple of seconds into every long drag. The
            // beat resumes on the release.
            _ = tokio::time::sleep_until(next_mode_reassert), if !app.mouse_held() => {
                // A RELEASE WATCH nothing has touched for a while goes
                // first, so the beat re-asks the resting flags, not its.
                if release_watch::expire(&mut app.chrome.release_watch, std::time::Instant::now()) {
                    let _ = watch_held_key($terminal.backend_mut(), false);
                }
                let _ = reassert_modes($terminal.backend_mut());
                next_mode_reassert = tokio::time::Instant::now() + MODE_REASSERT;
            }
            // The EDGE AUTO-SCROLL beat: a drag-selection resting past the
            // pane's top or bottom edge scrolls the history under it, with
            // no further mouse report to prompt it.
            _ = tokio::time::sleep_until(drag_autoscroll_deadline), if app.pane.next_drag_autoscroll.is_some() => {
                drag_autoscroll_tick(&mut app, &mut out);
            }
            // The selection rested past the debounce: tell the daemon what
            // the pane has been showing since the cursor landed here.
            _ = tokio::time::sleep(app.attach_delay().unwrap_or_default()),
                if app.pane.pending_attach.is_some() =>
            {
                fire_pending_attach(&mut app, &mut out);
            }
            // The worktree selection rested past the debounce: ask the
            // daemon to boot its dead sessions in the background so
            // attaching one replays a live screen instead of a cold boot.
            _ = tokio::time::sleep(app.prewarm_delay().unwrap_or_default()),
                if app.requests.pending_prewarm.is_some() =>
            {
                fire_pending_prewarm(&mut app, &mut out);
            }
            // The project selection rested past the debounce: ask GitHub
            // for its open issues in the background, so `i` paints the
            // rows at once instead of an empty modal.
            _ = tokio::time::sleep(app.issues_prefetch_delay().unwrap_or_default()),
                if app.github.pending_issues_prefetch.is_some() =>
            {
                crate::issues::fire_prefetch(&mut app);
            }
            // Standing keep-warm: periodically re-assert the selected
            // worktree's warm default-spec Claude session so the daemon's
            // reaper never leaves the next create cold.
            _ = tokio::time::sleep(app.keepwarm_delay().unwrap_or_default()),
                if app.requests.next_keepwarm.is_some() =>
            {
                fire_keepwarm(&mut app, &mut out);
            }
            ev = input.next() => match ev {
                Some(Ok(event)) => {
                    tracing::debug!(?event, "$terminal event");
                    if matches!(event, Event::Resize(..)) {
                        on_host_resize($terminal)?;
                    }
                    let probe = perf
                        .as_ref()
                        .and_then(|_| crate::perf::label(&event))
                        .map(|label| (label, std::time::Instant::now()));
                    let watching = app.chrome.release_watch.is_some();
                    handle_terminal_event(&mut app, event, &mut out);
                    // A RELEASE WATCH armed by this key flips the host's
                    // keyboard flags now, ahead of the key's first repeat;
                    // one ended by it flips them back (release_watch.rs).
                    if watching != app.chrome.release_watch.is_some() {
                        let _ = watch_held_key($terminal.backend_mut(), !watching);
                    }
                    if let (Some(perf), Some((label, arrived))) = (&mut perf, probe) {
                        perf.input(label, arrived, &app);
                    }
                }
                Some(Err(_)) | None => app.chrome.should_quit = true,
            },
            ev = $channels.rx.recv() => match ev {
                Some(server_event) => {
                    log_server_event(&server_event);
                    if let Some(perf) = &mut perf {
                        perf.server(crate::perf::server_name(&server_event));
                    }
                    handle_server_event(&mut app, server_event, &mut out);
                }
                None => {
                    app.chrome.conn = ConnState::Disconnected;
                    app.chrome.flash = Some("daemon connection lost".into());
                    app.chrome.dirty = true;
                }
            },
            ev = vim_rx.recv() => {
                // Never None: app.pane.vim_tx keeps a sender alive.
                if let Some(ev) = ev {
                    handle_vim_event(&mut app, ev);
                }
            }
            answer = pr_rx.recv() => {
                // Never None: `pr_tx` lives as long as the loop.
                if let Some((worktree, answer)) = answer {
                    let found = matches!(answer, Lookup::Found(_));
                    land_pull_request(&mut app, worktree.clone(), answer);
                    if found {
                        maybe_auto_cleanup_merged(&mut app, &worktree, &mut out);
                    }
                }
            }
            answer = git_rx.recv() => {
                // Never None: `git_tx` lives as long as the loop.
                if let Some((worktree, files, lines)) = answer {
                    let count = files.as_ref().map(Vec::len);
                    note_worktree_lines(&mut app, &worktree, lines);
                    keep_changed_files(&mut app, &worktree, files);
                    land_git_changes(&mut app, worktree, count);
                    // The selection moved on while this one was being read:
                    // ask for where it is now, rather than wait a poll.
                    if app.git_changes_stale() {
                        request_git_changes(&mut app, &git_tx);
                    }
                }
            }
            answer = sweep_git_rx.recv() => {
                // Never None: `sweep_git_tx` lives as long as the loop.
                if let Some((worktree, count, lines)) = answer {
                    note_worktree_lines(&mut app, &worktree, lines);
                    land_swept_changes(&mut app, worktree, count);
                }
            }
            answer = prs_rx.recv() => {
                // Never None: `prs_tx` lives as long as the loop.
                if let Some((project, list)) = answer {
                    note_open_prs_answer(&mut app, project, list, &mut out);
                    refresh_palette(&mut app);
                }
            }
            _ = tokio::time::sleep_until(next_update_check), if update_interval.is_some() => {
                crate::update_check::spawn(update_tx.clone());
                next_update_check = tokio::time::Instant::now()
                    + update_interval.unwrap_or(crate::update_check::DEFAULT_INTERVAL);
            }
            answer = update_rx.recv() => {
                // Never None: `update_tx` lives as long as the loop.
                if let Some(version) = answer {
                    app.chrome.dirty |= app.chrome.update_available.as_deref() != Some(version.as_str());
                    app.chrome.update_available = Some(version);
                }
            }
            // The hover debounce: the cursor has rested on a pull request
            // long enough to mean it.
            _ = tokio::time::sleep(app.pr_detail_delay().unwrap_or(Duration::MAX)),
                if app.pr_detail_delay().is_some() => {
                lookup_pr_detail(&mut app, &detail_tx);
            }
            answer = detail_rx.recv() => {
                if let Some((url, detail)) = answer {
                    land_pr_detail(&mut app, url, detail, &mut out);
                }
            }
            answer = prdiff_rx.recv() => {
                if let Some(answer) = answer {
                    land_pr_diff(&mut app, answer);
                }
            }
            answer = prcomment_rx.recv() => {
                if let Some(answer) = answer {
                    land_pr_comment(&mut app, answer);
                }
            }
            // The ISSUES MODAL's hover debounce: the cursor has rested on
            // an issue long enough to fetch its comments.
            _ = tokio::time::sleep(app.issue_detail_delay().unwrap_or(Duration::MAX)),
                if app.issue_detail_delay().is_some() => {
                crate::issues::lookup_detail(&mut app);
            }
            answer = issues_rx.recv() => {
                if let Some(answer) = answer {
                    crate::issues::land_answer(&mut app, answer);
                }
            }
            answer = branch_rx.recv() => {
                if let Some(answer) = answer {
                    crate::branch_switch::land_answer(&mut app, answer);
                }
            }
            answer = views_rx.recv() => {
                // Never None: `app.jobs.view_jobs` keeps a sender alive.
                if let Some(answer) = answer {
                    land_view_answer(&mut app, answer);
                }
            }
        }
        if app.nav.focus != focus_before {
            tracing::debug!(from = ?focus_before, to = ?app.nav.focus, "focus changed");
            note_focus_change(&mut app);
        }

        // Drain whatever else is immediately ready before redrawing once
        // (burst coalescing for PTY output).
        while let Ok(ev) = $channels.rx.try_recv() {
            log_server_event(&ev);
            if let Some(perf) = &mut perf {
                perf.server(crate::perf::server_name(&ev));
            }
            handle_server_event(&mut app, ev, &mut out);
        }
        while let Ok(ev) = vim_rx.try_recv() {
            handle_vim_event(&mut app, ev);
        }
        note_preview_change(&mut app, preview_before);

        // A worker thread panicked: the loop is fine, the default hook's
        // message is smeared over the alternate screen, and the crash log
        // has the backtrace.
        if take_worker_panic() {
            repaint($terminal)?;
            app.chrome.flash = Some("a background task crashed — logged to tui.log".into());
            app.chrome.dirty = true;
        }

        report_working_directory(&app, &mut directory_sent, $terminal.backend_mut())?;

        // Mouse handlers only record the pointer shape they want; emit the
        // OSC 22 request when it changes. Terminals without pointer-shape
        // support (Terminal.app) parse and drop the sequence.
        if app.chrome.pointer_shape != pointer_sent {
            pointer_sent = app.chrome.pointer_shape;
            use std::io::Write;
            let backend = $terminal.backend_mut();
            let _ = write!(backend, "\x1b]22;{}\x1b\\", pointer_sent.osc_name());
            let _ = backend.flush();
        }

        // A copy that had to be delegated to the attached $terminal (OSC 52 —
        // the only clipboard reachable from a headless `nebula ssh` host).
        // BEL-terminated on purpose: it is the form every OSC 52 implementer
        // accepts, ST is not.
        if let Some(payload) = app.chrome.pending_clipboard.take() {
            use std::io::Write;
            let backend = $terminal.backend_mut();
            let _ = write!(backend, "\x1b]52;c;{payload}\x07");
            let _ = backend.flush();
        }

        // A turn reached FINISHED: ring the DONE SOUND. The bell goes out
        // through the same $terminal as the OSC writes above, so over ssh it
        // rings the $terminal the user is sitting at. CONFIG.JSON is read
        // fresh, like every other setting.
        if std::mem::take(&mut app.chrome.pending_ding) {
            if let Some(sound) = crate::config::Config::load().done_sound() {
                alerts::play_sound($terminal.backend_mut(), sound);
            }
        }

        // One or more turns stopped to ask the user: ring the FEEDBACK
        // SOUND once for the lot and, while the $terminal window is in the
        // background, name each of them in a desktop notification — never
        // over ssh, where the desktop is the wrong machine's. `off` is
        // silence for both.
        let alerts = std::mem::take(&mut app.chrome.pending_feedback);
        if !alerts.is_empty() {
            if let Some(sound) = crate::config::Config::load().feedback_sound() {
                alerts::play_sound($terminal.backend_mut(), sound);
                if !app.chrome.window_focused && !app.chrome.is_remote {
                    alerts::notify_desktop(&alerts);
                }
            }
        }

        for req in out.drain(..) {
            if $channels.tx.send(req).await.is_err() {
                app.chrome.conn = ConnState::Disconnected;
                app.chrome.dirty = true;
            }
        }

        if app.chrome.should_quit {
            // Whatever the pull-request lookups learned since the last tick,
            // written inline: the process is about to end, and a write
            // handed to a thread here could be cut off with it.
            if let Some((cache, store, live)) = crate::pr_cache::take_flush(&mut app) {
                crate::pr_cache::write_all(&cache, &store, &live);
            }
            // Persist selection so the next launch restores it.
            let _ = $channels
                .tx
                .send(ClientRequest::SaveUiState {
                    json: ui_state_json(&app),
                })
                .await;
            return Ok(app.chrome.pending_ssh.take());
        }
    }

    }};
}

pub(crate) async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<BufWriter<Stdout>>>,
    channels: &mut ipc::IpcChannels,
) -> Result<Option<crate::hosts::HostEntry>> {
    main_loop_body!(terminal, channels)
}
