# Shift+G Opens The Repo's Git Host, Released As v0.3.0 — 2026-08-24

**Asked:** "is there a release skill in this repo?", then "commit and push and do another release", then
"make a skill called release which kicks in and does these similar steps the next time someone asks".

**Did:** Released **v0.3.0** — `c553409`, tag pushed, all four binaries attached. Feature commit
`b00ce46` (OPEN REPO) adds `crates/nebula-tui/src/remote.rs` (`repo_url`, `web_url`) plus `open_repo_in_browser`
in `event_loop.rs`, bound to `Action::OpenRepo` / `shift+g`. `ef56fca` checks in `CLAUDE.md`,
`.claude/MEMORY.md`, and the new RELEASE SKILL (`.claude/skills/release/SKILL.md`).

**Gotchas:**
- **Another agent was editing the same tree (the SHARED CHECKOUT) the entire time**, mid-way through a `--workspace` feature (STARTUP WORKSPACE):
  `protocol.rs`, `registry.rs`, `server.rs`, `app.rs`, `ipc.rs`, `main.rs`, `e2e_pty.rs` all turned
  modified while this task ran. It bit three separate ways — (a) `git add` on `event_loop.rs` captured
  **66 lines when the reviewed change was 56**, silently dragging in their
  `run_app(workspace: Option<String>)`; (b) the shared index was **reset out from under a staged
  commit**, so `git commit` answered "no changes added to commit"; (c) a `git worktree add` under the
  scratchpad was **pruned away while in use**. What worked: do the whole release in a private RELEASE WORKTREE
  on its own branch and `git push origin <branch>:main`. **Never `git add` in the SHARED CHECKOUT.**
- Local `main` stays behind `origin/main` after that push — it is checked out and dirty, so it can't be
  fast-forwarded. Say so explicitly; the next `git pull` has to reconcile.
- `e2e_tui::tui_projects_worktrees_agents_navigation` **failed at `origin/main` too** at the time:
  `FOOTER_TERMINAL_LOCKED = "Ctrl+q: panels"` (`crates/nebula/tests/e2e_tui.rs:29`) while the FOOTER
  rendered `^q: panels`. Introduced by `87d2b24` and shipped red in v0.2.0. **Fixed since — the whole
  e2e_tui suite is 6/6 green as of 2026-08-24.** The standing lesson: always re-run a failing test
  against `origin/main` before blaming your own diff.
- The RELEASE WORKFLOW (`.github/workflows/release.yml`) publishes with `generate_release_notes: true`, which is a bare commit
  list, not a changelog. `gh release edit vX.Y.Z --notes "…"` afterwards is the step that makes it one.
