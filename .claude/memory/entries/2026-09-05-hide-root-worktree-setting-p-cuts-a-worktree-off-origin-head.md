# An Experimental `hide_root_worktree` SETTING: The ROOT WORKTREE Row Goes, `p` On The WORKTREES PANEL Cuts A Fresh WORKTREE Off The Fetched `origin/HEAD` — 2026-09-05

**Asked:** "in a worktree, add an option in settings so that we don't show the main root in the
worktrees list.  the intent is a new prompt while having the worktree list selected should create a new
worktree automatically and then allow me to run multiple sessions inside that worktree.  keep this
option disabled by default but put it under an experimental tab in settings so a user can turn it on.
verify that all worktrees are created using the latest version of remote main (no whatever main has
locally)"
→ refined: "Do this in a WORKTREE. Add a SETTING, off by default, on a new Experimental tab in the
SETTINGS OVERLAY (assuming before Hotkeys) that hides the ROOT WORKTREE row from the WORKTREES PANEL.
With it on, a QUICK PROMPT (`p`) opened while the WORKTREES PANEL has FOCUS first creates a fresh
WORKTREE (assuming a random BRANCH NAME GENERATOR name), launches the agent there and selects it, so
later `p`/`n` from the SESSIONS PANEL land in it; everything else keeps today's behavior. Also make
every worktree creation with no explicit base — `n`, `nebula worktree`, this flow — `git fetch origin`
and branch from `origin/HEAD` (normally `origin/main`) instead of local HEAD, falling back to local HEAD
only when there is no origin or the fetch fails."

**Did:**
- The "verify" was a fix. `git.rs::add_worktree` ran `git worktree add <path> -b <branch>` with no
  start point, so `n` in the WORKTREES PANEL and a bare `nebula worktree` both cut from the ROOT
  WORKTREE's HEAD — and the SHARED CHECKOUT was `0 2` behind `origin/main` at task start
  (`git rev-list --left-right --count HEAD...origin/main`), so this session's own WORKTREE was cut by hand
  with `--base origin/main`. Now `registry.rs::create_worktree` — the choke point for the TUI's
  `CreateWorktree` and for `nebula worktree` (`enter_worktree`) — routes a base-less create through
  `git::add_worktree_off_default` → `git::default_base`: `git remote get-url origin` (none → HEAD),
  `fetch_origin` (`git fetch --quiet origin`, `kill_on_drop`, a 30 s `REMOTE_TIMEOUT` shared with the `set-head` round-trip because both run
  under the DAEMON's `worktree_ops` lock; failure → HEAD plus a `tracing::warn!`), then `origin_head`
  (`git symbolic-ref -q --short refs/remotes/origin/HEAD`, with one `git remote set-head origin --auto`
  when the symref is missing). The branch is cut `--no-track`. An explicit `--base` (and
  `add_pr_worktree`'s `origin/<head>`) keeps `add_worktree`'s tracking semantics untouched.
- SETTING `hide_root_worktree` on a new `Experimental` tab of the SETTINGS OVERLAY
  (`config.rs::SETTINGS_TABS`, before Hotkeys; hint 73 columns): the seven `config.rs` edits, the
  `App.hide_root_worktree` mirror through `event_loop.rs::set_hide_root_worktree` (clamps `sel_worktree`
  — `apply_config` has no `out`, so the TERMINAL PANE catches up on the next move), and
  `App::visible_worktrees` dropping `is_main` rows while it is on. The root and its sessions stay in the
  tree and the PALETTE; the row alone goes.
- QUICK PROMPT: `quick_prompt.rs::QuickTarget { Worktree(id) | NewWorktree { project, branch } }`
  replaces `QuickLaunch.worktree`. `open_quick_prompt` with the SETTING on and `Focus::Worktrees` opens
  on a `NewWorktree` target named by `branch_name::random_name(app.project_branches(..))` (title
  `Quick prompt · new worktree <branch> (claude)`), whatever row the cursor is on. New
  `event_loop/quick_launch.rs`: `submit` sends `CreateWorktree { base: None }` carrying
  `PendingIntent::LaunchInCreatedWorktree { launch, text }`; the Ack (`launch_in_created_worktree`)
  selects the new row, puts FOCUS back on the panel `p` was pressed in, retargets and calls
  `create_agent` with the text as STARTING PROMPT; an Error reopens the box with the text. The pickers
  get a context checkout from `picker_context` (`KindPicker::quick_prompt(context, back)`,
  `AgentPresetsView::new(context, ..)` — the hidden ROOT WORKTREE for a `NewWorktree` target), and the
  `NewAgentOfKind { quick: Some(back) }` arm rebuilds the launch from `back.launch.target`.
- Docs: `docs/configuration.md` (27 keys, the Experimental tab, the row), `docs/keys.md` (`n`'s base,
  `p` with the SETTING on), `docs/how-it-works.md`.
- Gate: `make ci` green in this WORKTREE with `CARGO_TARGET_DIR=<scratchpad>/vtarget` — nebula-tui 607,
  nebula-daemon 196, e2e_pty 29, e2e_tui 8 — plus a targeted rerun for the Tab round-trip assertion added
  after it. New tests: `git.rs` ×3 (`default_base_is_none_without_an_origin`,
  `default_base_falls_back_to_head_when_the_fetch_fails`,
  `a_worktree_off_the_default_base_starts_at_the_fetched_origin_head`), `config.rs` ×1, `event_loop.rs`
  ×3, `quick_prompt.rs` ×1. Not exercised: the live DEV INSTANCE (this session's daemon is the release
  binary; restarting it kills the session).

**Gotchas:**
- `git worktree add -b <branch> <path> origin/main` makes the new branch *track* `origin/main` — git's
  `branch.autoSetupMerge` default whenever the start point is a remote-tracking branch. `nebula worktree
  hide-root-auto-worktree --base origin/main` (this session's relocation) therefore left the task branch
  with `@{upstream}` = `origin/main`: `git status` read against main and a bare `git push` would have
  aimed at it (`push.default=simple` refuses, `upstream` sends). `git branch --unset-upstream` fixed it;
  the DAEMON's default-base path passes `--no-track` for exactly this, while an explicit `--base` still
  tracks because `add_pr_worktree` relies on `feat-x@{upstream}` = `origin/feat-x`.
- `git remote add origin … && git push -u origin main` never writes `refs/remotes/origin/HEAD`, so
  `git symbolic-ref refs/remotes/origin/HEAD` fails on hand-wired repos and on every test fixture;
  `git remote set-head origin --auto` (one `ls-remote`) records it — `origin_head` does that once and
  re-reads.
- A test that "proves the fetch" by committing locally and pushing proves nothing: the push updates the
  local `origin/main` too. The origin has to move from a *second clone* (`git clone origin other`,
  commit, push) so the local remote-tracking ref is stale until `default_base` fetches.
- The QUICK PROMPT's `Tab` picker rebuilt the launch from the picker's own `NewAgentOfKind.worktree`
  (`of_kind(worktree, …)` in the quick arm): a target that is not that checkout would have been silently
  retargeted at the picker's context — the hidden root. The arm reads `back.launch.target` now, and the
  auto-worktree test walks the round trip.
- `make ci` runs `cargo fmt --all -- --check` *before* clippy and the tests, so Rust written by a script
  that is not rustfmt-clean aborts the gate at step four with nothing built (`make[2]: *** [ci] Error 1`
  and no `test result:` lines): `cargo fmt --all` first, then `make ci`.
- `git worktree add --no-track -b <branch> <path> [base]`: the flag has to come before `-b`; with no
  base it is harmless (HEAD is never a remote-tracking branch), which is why `add_worktree_inner` can
  pass it unconditionally on the default path.
- Landing this from a WORKTREE branch: `origin/main` had gained one commit that also prepended a MEMORY LOG
  index line and edited the Candidates ledger (two `UU` files, `gotchas.md` auto-merged) — and the
  auto-merge put `gotchas.md` at 301/300, each side having been under the cap alone. Run
  `make memory-check recall-eval terms-check` *after* the merge and before the merge commit, with `&&`
  between them: a `;` chain committed over the red gate and had to be amended; the twin to merge was in
  the RELEASE SKILL group, the largest.
