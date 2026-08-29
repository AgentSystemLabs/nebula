# Released v0.18.0: A Shared `vtarget` Swapped The E2E Binary, And The ROOT Row Pin Had Stale E2E Steps — 2026-08-29

**Asked:** "commit push release" (RELEASE SKILL trigger — PROMPT DADDY skipped). Mid-task: "after you're
done, explain what you found reading the memory log and how it changed the outcome of this request".

**Did:** Cut **v0.18.0** (0.17.0 → minor: Cursor MODEL / EFFORT with `cursor_catalogue.rs`, TYPE-AHEAD in
the MODEL / EFFORT submenus and the PRESET EDITOR, the PALETTE's attention order, the ROOT row pinned first
in the WORKTREES PANEL, the PILL ROW rail / TAB UNDERLINE in the STATUS DOT color, `make prune`; plus the
Mission Control skills / hooks / `.mcp.json` removed and every MEMORY LOG entry rewritten in TERMS). The
SHARED CHECKOUT's HEAD *was* `origin/main` (`0 0`), so the delta was a plain copy into the RELEASE
WORKTREE: 131 modified + 11 untracked `cp`'d through `while IFS= read -r`, 7 `git rm`, `git diff HEAD |
shasum` equal in both trees, every untracked file `cmp`-identical. `git worktree list` showed another
session's `release-v0.18.0` worktree (scratchpad `53eb0464…`, 10:28–10:32 the same day): its `test.log`
ended in `No space left on device` — the gate that the `make prune` task then fixed — and its `notes.md`
was a RELEASE NOTES draft, reused and extended. `git branch -m release-v0.18.0 release-v0.18.0-abandoned-1030`
freed the name; the worktree was left in place. Gate on the borrowed `9fc688a9…/vtarget`: fmt, `make
memory-check` (index 151/200, gotchas 299/300), clippy `-D warnings` clean; `cargo test --workspace` failed
one test, `tui_projects_worktrees_agents_navigation`, deterministically (2/2 reruns), at
`crates/nebula/tests/e2e_tui.rs:530`: the delta pins the ROOT row first (`app.rs::visible_worktrees` sorts by
`(Reverse(w.is_main), Reverse(interacted))`, lib test `worktrees_sort_by_last_interaction` and the README
updated, **no MEMORY LOG entry**), so the walk after `feat-a` earned its stamp is `[main, feat-a, feat-b]`,
not the `[feat-a, main, feat-b]` the test's comment documents — `k` now reaches `main ⌂ root`, `j` comes
back; swapped the two keys and the comment (same edit copied to the SHARED CHECKOUT's pristine file).
The "whose fault" check — `origin/main` in a SCRATCH WORKTREE `$S/base`, same `CARGO_TARGET_DIR` — passed
in 6 s, and then poisoned the gate (gotcha 1): two more red runs before `cargo clean -p nebula -p
nebula-tui -p nebula-daemon -p nebula-core` (24,666 files, 7.1 GiB) and a full re-gate: **796 passed, 0
failed** across 11 binaries. Three commits (feature = `crates` + README + ARCHITECTURE + Makefile + scripts
/ scaffolding = `.claude` + TERMS + the MC removals + `docs/client-daemon-interaction.html`, an unreferenced
mermaid diagram from 09:22 / `Release v0.18.0`), push behind the pinned-SHA guard (`af8b032`, origin had not
moved), tag, all four RELEASE WORKFLOW targets green, `gh release edit --notes-file`. RELEASE NOTES carry a
`⚠️ Heads up` for `nebula kill` although PROTOCOL VERSION stayed 34: the Cursor `--model` join lives in the
DAEMON (`registry.rs::agent_spawn_command_with`). Reconciled the SHARED CHECKOUT: every dirty file
`cmp`-identical to the release tree → `git stash push -u` + `git pull --ff-only` → clean at `3931dca`.
Added one sentence to `.claude/skills/release/SKILL.md` §2 at the trap site (gotcha 1).

**Gotchas:**
- **Two worktrees built into one `CARGO_TARGET_DIR` share the `nebula` bin's unit hash.** Building
  `origin/main` in `$S/base` overwrote `vtarget/debug/nebula` (only one `deps/nebula-<hash>` was built all
  day, and the uplift `cmp`-equalled it); back in the RELEASE WORKTREE the bin was "fresh", so
  `CARGO_BIN_EXE_nebula` ran `origin/main`'s TUI for two E2E runs — the screen showed the *old* worktree
  order and the fixed test failed on `k` from row 0. `rm debug/nebula` re-links the same file. Fix:
  `cargo clean -p` the four workspace crates, or give the base check its own target dir. The v0.17.0
  entry's "fingerprints are path-keyed" was wrong for the bin; corrected in `gotchas.md`.
- **An existing `release-vX.Y.Z` branch is a prior attempt, not a collision** — find it with `git worktree
  list`, read its scratchpad `test.log` / `notes.md` for why it stopped (ENOSPC here) before deciding, and
  `git branch -m` it aside — that works while the branch is checked out in the other worktree.
- **Lib-green is not panel-order-green.** The ROOT-row pin shipped on `nebula-tui` lib tests + README with
  no E2E run; E2E TUI is the only gate that walks the WORKTREES PANEL's order end to end, and its comment
  spelled out the old order. Any change to `visible_worktrees` / a panel sort key needs
  `cargo test -p nebula --test e2e_tui`.
- The same needle (`main ⌂ root`) sits on four `wait_for_*` lines; the panic's `tui.log tail` keys
  (`T`, `^q`, `Left`, `j`) placed the failure at line 530, not the 479 the first grep suggested — match the
  tail against the test body before reading the predicate. `cut`-ing the dump to 47 lines hid the tail
  the first time.
- A `cd` inside one Bash call carried into the next calls here (`find docs`, `git diff -- README.md`
  silently empty) until the harness reported "Shell cwd was reset" — use absolute paths and `git -C`
  throughout a release.
