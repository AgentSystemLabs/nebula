# Released v0.15.0 By Cherry-Picking Only The Post-v0.14.0 Delta Onto origin/main — 2026-08-28

**Asked:** "pull latest from main, commit push and make another relase"

**Did:** Cut **v0.15.0** (0.14.0 → minor: new user-facing features). Carried the OPEN PRS → Claude
SESSION launch (`CreatePrAgent`, MIGRATION 22 `agents.pr_url`), the NEW LINK removal / OPEN PRS group,
NEBULA TUNNEL's ttyd reuse, and the PROMPT DADDY / OUTPUT DOCTOR skill revisions. Private worktree on
`release-v0.15.0` off `origin/main`, **735 passed, 0 failed**, clippy clean, three commits
(`055809b` feature / `4072100` scaffolding / `2d0a187` Release), `git push origin release-v0.15.0:main`,
tag, all four matrix targets green, notes via `gh release edit --notes-file`. Then — the "pull latest"
half — fast-forwarded the SHARED CHECKOUT itself: `git stash -u && git pull --ff-only origin main`,
verified `git diff origin/main` empty, dropped the stash. The pre-merge shared-tree content survives on
branch `snap-shared` (commit `b6b48ea`); the scratch worktrees are removed.

**Gotchas:**
- **The shared tree was 30 commits / 1 release behind, and its uncommitted diff was two layers**: the
  v0.14.0 work (already on origin, in merged form) *plus* the post-v0.14.0 work. Applying the whole
  `git diff HEAD` with `--3way` would have re-fought the dedup pass (PR #18) over the v0.14.0 half too.
  Better: the previous release's scratchpad still held its `local.patch`, so `git apply` it onto the
  shared HEAD in a scratch worktree → commit **S** (tree at the v0.14.0 cut), then apply today's
  `git diff HEAD` + `cp` the untracked files → commit **T**; `git cherry-pick --no-commit T` onto
  `origin/main` merges *only* S..T (20 files, 7 conflicted, all mechanical). Untracked files as they
  stood at the earlier cut come from that release's tip (`git show b29fdd5:TERMS.md`).
- **PROTOCOL VERSION collided silently.** origin's `r` project-rename (`7b4201b`) had bumped 28 → 29
  and the PR-agent work had independently bumped 28 → 29; `protocol.rs` auto-merged at 29 with a new
  `CreatePrAgent` variant on top. Shipped as **30**. Before committing a merged release, diff
  `PROTOCOL_VERSION` and the `MIGRATIONS` tail against `origin/main` — the migration (22) happened to be
  unique this time.
- Taking either side of the `use nebula_core::{…}` import hunk loses the other side's names
  (`TerminalId` from the dedup pass vs. `LinkId` dropped by the LINK removal) — rebuild that line, don't
  pick a side. Everything else the dedup pass changed under the new work was a constructor swap
  (`MenuItem { .. }` → `MenuItem::new`, `out.push(…)` → `send_with(app, out, intent, |req_id| …)`,
  `Duration::from_secs(5)` → `EVENT_TIMEOUT`, `"NEBULA_AGENT_CMD"` → `env::AGENT_CMD`).
- On origin, `7b4201b` inserted `tui_project_rename_shows_the_folder_and_empty_undoes_it` *between* the
  old links test's doc comment and its `#[test]`; the LINK removal's conflict there was where to put the
  new `tui_manual_link_add_is_unavailable` comment — drop the four stale `/// Links live in …` lines.
- Reusing the previous release's `vtarget` as `CARGO_TARGET_DIR` (same deps, nothing else building
  there) made clippy + the full suite a few minutes instead of a cold build.
- Housekeeping the shared repo still carries: eleven `release-v0.x.y` branches, the v0.14.0 release
  worktree in another session's scratchpad (shows in `git worktree list` and as a WORKTREES PANEL row),
  and two older stashes (`pre-v0.13.0-pull`, `pre-v0.10.0`). Not touched here.
- The installed `~/.cargo/bin/nebula` is still 0.13.0 and the live TUI/daemon are `target/debug`; the
  PROTOCOL VERSION bump means the next install needs NEBULA KILL (or MAKE CYCLE) before the new TUI
  attaches.
