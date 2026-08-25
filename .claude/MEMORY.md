# Nebula Memory

Work log written by the `nebula-memory` skill. Newest first. Read this before starting a task; append
after finishing one. See `.claude/skills/nebula-memory/SKILL.md` for the entry format and the rules
about what is worth recording.

> **Provenance.** Everything dated 2026-08-04 through 2026-08-24 is a backfill, reconstructed on
> 2026-08-24 from the 152 session transcripts in `~/.claude/projects/…-nebula/`, `git log`, and the
> verified notes in that project's `memory/` directory. Prompts are quoted from the transcripts, and
> every file and symbol named below was confirmed to still exist. The **Did** lines are grounded in
> commits and code; where a session's outcome could not be verified it was left out rather than guessed.
> Entries written from here on are first-hand. The ~300-line pruning rule in the skill applies to
> ongoing appends — this backfill is deliberately over it.

## Entries

### Release v0.5.0 — 2026-08-25

**Asked:** "commit push and tag a new release 0.5.0 version"

**Did:** Followed `.claude/skills/release/SKILL.md` in a private worktree. Two commits landed on `main`:
the already-finished-but-uncommitted pill-rail fix + `assets/screenshot.png` (17320c5), then the version
bump to 0.5.0 (1018bdf, tagged `v0.5.0`). All 4 release-matrix targets built and the notes were rewritten
by hand.

**Gotchas:**
- **`origin/main` moved out from under the release mid-task** — PR #12 (`worktree-hl-panel-nav`) merged
  while this was running, touching `event_loop.rs` and `.claude/MEMORY.md`, the same two files the
  uncommitted release content touched. The skill's "bring files in by content" step means literally
  that: a blind `cp` of the shared tree's modified file over the fresh `origin/main` copy would have
  silently reverted the merged PR. `git diff -- <file> > x.patch` then `git apply --check x.patch` against
  the fresh worktree file proved whether it was safe — `event_loop.rs` and `ui.rs` applied clean (my hunk
  didn't overlap their edits), `.claude/MEMORY.md` did not (both added an entry right after `## Entries`)
  and needed a manual insert instead of `git apply`.
- **The uncommitted changes in the shared tree were not scratch work** — they were a finished bugfix
  from a prior session, already written up in `.claude/MEMORY.md` (the "Black Notches" entry) and never
  committed. Worth reading the diff and the existing memory entry before assuming uncommitted files are
  someone's mid-task state to leave alone.
- Local `main` in the shared tree is now behind `origin/main` by both PR #12 and this release — its next
  `git pull` needs to fast-forward through both.

### Black Notches At The Selected Pill's Corners (Issue #6) — 2026-08-24

**Asked:** "try to fix this style issue, see attached image https://github.com/AgentSystemLabs/nebula/issues/6"
— the issue body: "there are little black bars top and bottom when the focus terminal setting is enabled,
find a way to make sure those are gray and make the row itself."

**Did:** `render_pill` in `crates/nebula-tui/src/ui.rs`. The rail now owns the pill's first column
outright: `PILL_RAIL` (`█`) on the text row, and each pad row's own `PILL_HALF` glyph (`▄`/`▀`) drawn in
the rail color instead of the old `PILL_RAIL_CAPS` quadrants (`▖`/`▘`, added in `4bea626`). Updated the
glyph assertions in `event_loop.rs::pill_rail_spans_pads_and_sessions_match_worktree_stride` and added
`ui.rs::pill_rail_leaves_no_untinted_quarter_at_the_corners`.

**Gotchas:**
- **A terminal cell holds one glyph and two colors, so three colors in one cell is impossible.** The pad
  row's rail cell wants panel-bg (outside the pill), rail, *and* fill — a quadrant cap can only pick two,
  so the fill quarter beside it fell through to bare background. That is the whole bug; there is no
  cleverer glyph. Options are: rail takes the full cell (chosen), or the fill takes it and the rail stops
  at the text row.
- **The setting in the issue is `focus_tint` ("Focused panel tint"), not anything called "focus
  terminal".** The notch has always been there — without the tint it's the terminal's own background
  (`#282c34` on this user's Terminal.app) against `sel_bg` `#3a3a3a` and nearly invisible. `draw_focus_tint`
  only repaints cells whose `bg == Color::Reset`, so it turns exactly that stranded quarter near-black.
- **Do not evaluate a TUI style change by reading code.** Mocking the four candidate geometries as PNGs
  settled it in one look — the "fill quarter with `bg = fill`" variant sprouts a gray tab above the pill,
  and a half-block rail with full-width caps flares into an I-beam.
- **You can render the real buffer without tmux or a font.** A temporary `#[test]` that draws
  `ui::draw` into a `TestBackend`, dumps `symbol\tfg\tbg` per cell, plus a ~60-line pure-Python PNG
  writer that paints block glyphs as rects and any text glyph as a bar, reproduces the artifact exactly
  and proves the fix. Much cheaper than the `NEBULA_RUNTIME_DIR` + tmux screenshot harness for anything
  made of block-drawing characters.

### h/l Move Between Panels (Issue #8) — 2026-08-24

**Asked:** "work on https://github.com/AgentSystemLabs/nebula/issues/8 in a worktree make pr when done,
move notes and links to other hotkeys, but h and l should be for left and right" — issue #8 asks for the
vim pairing, since `h`/`l` opened the ssh hosts picker and the add-link prompt instead.

**Did:** Four `defaults:` arrays in `crates/nebula-tui/src/keymap.rs` — `focus_left` → `["h", "left"]`,
`focus_right` → `["l", "right"]`, `hosts` → `["shift+h"]`, `new_link` → `["shift+l"]`. No dispatch code
changed. New test `h_and_l_walk_panel_focus_like_the_arrows` in `event_loop.rs`; the hosts and link tests
(8 unit + 3 in `crates/nebula/tests/e2e_tui.rs`) now drive `⇧H`/`⇧L`. PR #12 off `worktree-hl-panel-nav`.

**Gotchas:**
- The user said "move **notes** and links", but notes is `e` and never conflicted — the two actions
  actually sitting on `h`/`l` were **hosts** and links. Read the issue, not just the prompt.
- **Never bulk-replace `Char('h')`/`Char('l')` in `event_loop.rs`.** Most hits are overlay-local grammar
  the keymap doesn't own and must not change: settings tab/value cycling (~3495-3514), the diff and tree
  browsers (~2919, ~2960-2966). Only the *test* presses needed swapping.
- Footer and Help hints come from `Keymap::first(action)`, so **the chord you want displayed has to lead
  the `defaults:` list** — `["h", "left"]` shows `h`, `["left", "h"]` would still show `←`.
- Changing a default is safe for existing users: `Keymap::overrides()` (keymap.rs:860) persists only rows
  that differ from `defaults`, so an untouched action picks the new key up on upgrade while anyone who
  rebound it keeps theirs.
- `defaults_do_not_collide_within_a_scope` is the guard for this kind of edit — it fails loudly if a new
  default double-books a chord in the same `Scope`.

### The Version Nameplate In The Footer's Left Edge — 2026-08-24

**Asked:** "display the version number of nebula in the bottom bar somewhere, I think bottom left should
say nebula vx.y.z"

**Did:** `draw_footer` (`crates/nebula-tui/src/ui.rs`) now splices `nebula v{env!("CARGO_PKG_VERSION")}`
+ `"  ·  "` in at span index 1 (after the leading pad, ahead of `◇ workspace`), styled `th.dim`. The
splice happens **after** `left` is computed, not where the workspace span is pushed, because the decision
needs the width the usage readout left behind: it is skipped when `app.flash.is_some()` **and** the spans
already measure wider than `left.width`. New unit test
`footer_shows_the_nebula_version_but_never_truncates_a_flash` covers both branches. `nebula --version`
(clap, `crates/nebula/src/main.rs:10`) reads the same workspace version, so the two agree by construction.

**Gotchas:**
- **The footer's left edge is a fixed column budget and it was already full.** The nameplate costs 18
  columns (`nebula v0.2.0` + separator) and everything downstream of it — workspace, hostname, conn,
  breadcrumb, hints/flash — just shifts right and clips off the end. Anything else added here pays the
  same toll.
- **A clipped flash is a broken feature, a clipped hint list is not.** The e2e
  `tui_pull_request_row_leads_the_links_group` caught this: at `COLS = 120` the bar rendered
  `… #7 Attach links    the pull request link c` — the flash lost `an't be deleted`. Hint lists are
  ordered by importance and truncate harmlessly; flashes are sentences. Hence the flash-only yield rather
  than a blanket "only if it all fits", which on a 120-col terminal would have hidden the version almost
  always.
- That failure looked like a flake at first — it passed alone and `tui_link_crud_in_sessions_panel` failed
  alongside it once, then didn't. Only `tui_pull_request_row_leads_the_links_group` was real. The panic
  message's `--- screen ---` dump is what identifies it: read the rendered footer line, don't rerun blind.
- `splash_footer_lists_only_keys_that_work` (`event_loop.rs`) asserts `m: menu` reaches the bar and was
  sized at exactly `TestBackend::new(140, 30)` — 18 columns of nameplate pushed `m: menu  ?: help` off.
  Bumped to 160. Any future footer addition trips this test first; it is a width canary, not a key bug.
- This tree's `Cargo.toml` is still `0.2.0` while `origin/main` is `0.4.0` (see the v0.4.0 entry below), so
  a binary built here reads **nebula v0.2.0**. That is the built version, not a bug in the readout.

### Retiring Closed Pull Requests From The OPEN PRS Group — 2026-08-24

**Asked:** "when a pr is closed, we should periodically check from github to see if we should remove from
our list, also maje sure draft prs are included in that list we show" — ambiguous between the OPEN PRS
group and the worktree's own PR row in LINKS; the user picked **the OPEN PRS group** when asked.

**Did:** `OPEN_PRS_REFRESH` 3min → **60s** in `crates/nebula-tui/src/event_loop.rs` (that beat *is* the
pruning mechanism — `--state open` stops returning a merged/closed PR, so nothing tracks closures
separately). `note_open_prs_answer` gained an `out: &mut Vec<ClientRequest>` 4th arg and now calls two
new helpers: `reconcile_open_pr_cursor` (follows the cursor's PR across a reorder by URL; on retirement
clamps to the nearest surviving row, `restore_session`s if that's a checkout, and flashes
`#N is no longer open`) and `forget_retired_prs` (retains `pr_detail` / `pr_detail_failed` to URLs still
in some project's list). New `PrDetail::is_open()` + `drop_retired_pr` retire a row the instant the
hover-detail fetch comes back `MERGED`/`CLOSED`, ahead of the next list. Drafts needed **no code change**
— verified live that `gh pr list --state open` returns them (24 of 75 on `cli/cli`) — so the work there
was a doc note on `list()` and two regression tests. 5 new tests; workspace suite 598 green, clippy
unchanged (3 pre-existing warnings).

**Gotchas:**
- **`schedule_pr_detail` zeroes `app.pr_preview_scroll`.** Calling it unconditionally from the cursor
  reconcile meant every 60s refresh yanked a reader back to the top of the PR conversation they were
  halfway down. It is now called only on the branch where the row actually went away. Any new caller on
  a *timer* path has the same trap.
- Capture the cursor's PR **before** mutating `app.open_prs`, and follow it by **URL, not index** —
  `gh pr list` is newest-first, so anyone opening a PR reshuffles every row below it and an index-based
  cursor silently lands on a different pull request.
- The reconcile is inherently scoped to what's on screen: `visible_open_prs()` reads the *selected*
  project, so a late answer for a different project finds the cursor's URL unchanged and no-ops. No
  project-id comparison needed.
- Retiring the row the cursor is on is jarring without the flash — the pane just jumps. `app.flash`
  turns it into an explanation, and it's the one place both the list refresh and the detail-driven
  eviction funnel through.
- `note_open_prs_answer` is also called from `lookup_open_prs`'s not-a-directory branch, so `out` had to
  be threaded through that too (and into the git-poll tick's call).
- `gh pr list --state open` includes drafts by construction — do not "fix" a missing draft by adding a
  filter. If drafts ever go missing the bug is downstream, in `parse_list` or the `is_draft` badge at
  `ui.rs:2615`.

### Released v0.4.0 From A Tree Two Sessions Were Writing To — 2026-08-24

**Asked:** "commit and push and do a release" (the open-PR reading pane + PR diff work below).

**Did:** **v0.4.0** — `1ac59a3`, tag pushed, all four binaries attached, changelog replaced. Three
commits: `cca5fbb` (feature), `684d562` (memory), `1ac59a3` (version bump). Local `main` is still at
`c340baf` (v0.2.0) and two releases behind `origin/main`.

**Gotchas:**
- **The shared tree held two agents' unfinished features at once** — a Claude Cloud launch flow and
  per-instance workspaces — tangled into `app.rs`, `ui.rs`, `event_loop.rs`, `lib.rs`, `README.md`. What
  worked: diff the *working tree* against `origin/main` per file, split into hunks, classify each one,
  and apply only mine onto the pristine copy. Scripts worth rebuilding:
  `hunks.py <old> <new>` (list hunks with a preview), `show.py <old> <new> 3,7` (dump specific ones),
  `pick.py <old> <new> <out> 0,2,6` (apply a subset with `patch -p0`, which tolerates the wrong line
  numbers a filtered patch carries). Residual-hunk count after picking is the check: it must equal the
  number you classified as theirs.
- **A hunk is not a semantic unit.** Two of mine had another session's line inside them — a
  `switch_workspace(...)` call adjacent to a `Palette::new` signature change, and a whole test of theirs
  (`palette_query_terms_match_independently_across_a_space`, which used *my* `seed_open_prs` helper to
  test *their* fuzzy change) sitting next to my test block. Both compiled fine in the shared tree and
  only failed in the isolated build. **The green gate is the only thing that catches this** — grepping
  the staged diff for their identifiers (`cloud`, `startup_workspace`, `switch_workspace`,
  `is_multiline`) afterwards is a cheap second check and found nothing left.
- **`README.md` had been rewritten wholesale** by the other session (243-line hunk). Hunk surgery was
  hopeless; re-applying my three edits by hand onto `origin/main`'s copy took two minutes.
- **A green shared tree is not evidence about `main`.** `e2e_tui::tui_projects_worktrees_agents_navigation`
  passed locally the whole time because someone else had already fixed `FOOTER_TERMINAL_LOCKED` there —
  at `origin/main` it was still red, as it had been since v0.2.0. I had "corrected" the memory entry
  below to say it was fixed; that was wrong, and it is fixed properly now (in `e2e_tui.rs`, shipped in
  v0.4.0). Always run the doubted test in a **detached worktree at `origin/main`**.
- Use a separate `CARGO_TARGET_DIR` for the release worktree (`$SP/vtarget`). Sharing the main one with
  a concurrently building session makes both thrash fingerprints.

### Reading A Pull Request And Its Diff Inside Nebula — 2026-08-24

**Asked:** "is it possible so that when I hover over a PR i nthe open pr list, it'll show the contents of
the PR directly in nebula for me to read? also the ability to just view the git diff of that PR directly
in nebula?" (follow-on to the OPEN PRS group below.)

**Did:** Hover (cursor rests on an open-PR row) → the terminal pane becomes a reader: headline, state,
`+adds -dels · N files`, description, then the whole conversation. New
`crates/nebula-tui/src/pr_preview.rs` (`wrap`, `fit`, `lines`) builds it as a flat `Vec<Line>` so
scrolling is a slice. Fetch is `pull_request::detail()` (`gh pr view <n> --json …`), debounced
`PR_DETAIL_DEBOUNCE = 300ms` via `schedule_pr_detail`/`lookup_pr_detail`, cached per URL for the
session. `g` on a PR row runs `pull_request::diff()` (`gh pr diff <n>`), `split_unified_diff()` cuts it
per file, and `open_pr_diff_view` opens the **existing** `DiffView` on it via a new
`DiffView::prefetched: Option<HashMap<String,String>>` that `git_diff::load_selected_diff` reads instead
of shelling out. `ui::terminal_frame` now delegates to `titled_frame(title, …)` so the pane can be
called PULL REQUEST.

**Gotchas:**
- **`draw_terminal` returning early on a PR row is the whole trick** — the attachment underneath stays
  live, so walking into the OPEN PRS group and back never churns detach/attach. Exact copy of the
  `divider_focused()` branch three lines above it. Do not try to "clear" the terminal for this.
- **ratatui silently clips an overwide `Line`, taking the rest of the row with it.** A header row built
  from spans (state · author · base ← head) blew past the pane at width 24 and the test
  `no_rendered_line_overflows_the_pane` is what caught it. Everything the preview emits now goes through
  `wrap` (prose) or `fit` (span rows); `fit` drops whole segments from the end, then ellipsises.
- Reviewed ✓ marks are **deliberately not persisted** for a PR diff: `review::store_marks` prunes any
  key that isn't a directory on disk (`store.worktrees.retain(|root, _| Path::new(root).is_dir())`), and
  a pull request has no path. In-session marks still sink files to the bottom, which is the useful half.
- `shift+up`/`shift+down` are already `move_project_up/down`, so the preview scrolls on
  **PgUp/PgDn/Home/End** (+ wheel) only — handled as raw `KeyCode`s *before* the keymap lookup in
  `handle_key`, since those chords are unbound at panel scope.
- Key handlers can't reach the loop's channels, so the `gh pr diff` sender lives on
  `App::pr_diff_tx` — the `vim_tx` precedent. Without it `request_pr_diff` silently no-ops (which is
  also why its test installs a channel by hand).
- `gh pr view --json` field names verified live: `author`/`baseRefName`/`headRefName`/`additions`/
  `deletions`/`changedFiles`/`body`, comments carry `createdAt`, reviews carry `submittedAt` + `state`.
  Reviews with **no `submittedAt`** are your own pending draft — drop them.
- `split_unified_diff` takes the path from `+++ b/…` when present and falls back to the `diff --git`
  header, because a **deleted** file's `+++` is `/dev/null` and a **rename**'s two halves differ. The
  invariant worth keeping: every input line lands in exactly one chunk (asserted in the unit test, and
  verified against real `gh pr diff -R cli/cli` output).
- A `gh` that can't answer is remembered in `pr_detail_failed` — without it the pane re-asks on every
  pass and sits on "reading it…" forever.

### Space In A Fuzzy Query Is An AND, Not A Char — 2026-08-24

**Asked:** "I want the / fuzzy finder to be more fuzzy, like I should be able to type neb #10 and it
would have displayed the pr that had the #10 in it, right now it shows nothing if I type \"neb #10\""

**Did:** `crates/nebula-tui/src/fuzzy.rs::fuzzy_match` now splits the query on whitespace and requires
every term to subsequence-match the candidate *independently* — fzf's extended-search AND. The
best-of-starts greedy pass moved into a new `match_term(term, cand)`; `fuzzy_match` sums the term scores
and unions their positions. `rank`'s empty-query guard became
`query.split_whitespace().next().is_none()`. One matcher change covers all four call sites (`/` palette,
diff-view file filter, `f` file finder, `tree_browser.rs:300`). 8 unit tests in `fuzzy.rs` plus
`event_loop.rs::palette_query_terms_match_independently_across_a_space`; workspace suite 578 green.

**Gotchas:**
- The bug was never in the palette — it was one line of matcher semantics. `neb #10` against
  `nebula/#10 Credit…` failed because the greedy pass demanded a *literal space* between `neb` and
  `#10`, and the only space in that row sits after the `#10`. Nothing about the palette, the PR rows,
  or `TextInput` was wrong; `KeyCode::Char(' ')` reaches `palette.query` fine via the catch-all at
  `text_input.rs:183`.
- Terms match independently, so their spans can **overlap and arrive out of order** (`"ne neb"` yields
  `[0,1,2,0,1]`). `positions` feeds `ui.rs::fuzzy_highlight_spans`, which wants one ascending run —
  sort + dedup before returning or the highlight breaks.
- Whitespace-only queries needed their own guard in `rank`. `"  "` is not `is_empty()`, so it fell into
  the scoring path where every candidate scores 0 and the length tiebreak silently **re-sorted the whole
  list shortest-first** — a query that says nothing must not reorder anything.
- The three clippy warnings on `nebula-tui` (`ui.rs:2316`, `ui.rs:2446`, `config.rs:895`) are
  pre-existing, not from this change.

### Open Pull Requests Under The Worktrees List — 2026-08-24

**Asked:** "when a user opens a project, it should try to fetch all open pull requests and display those
on the bottom of the worktrees list, so a user can easily see which pull requsts are still open and enter
or click into them to open in browser. make sure you make this efficient as gh might have rate limits,
and some projects might have a LOT of opened pull requests, also make sure a user can easily fuzzy find
(/) to those pull requests by title which when opened will open the browser instead of trying to switch
to a session or worktree, etc"

**Did:** New `OpenPr` + `list()` + `parse_list()` in `crates/nebula-tui/src/pull_request.rs`
(`gh pr list --state open --limit 100 --json number,url,title,isDraft`, `LIST_LIMIT = 100`). Cached per
project as `App::open_prs: HashMap<ProjectId, OpenPrs>` with `open_prs_inflight`; driven from
`lookup_open_prs`/`note_open_prs_answer`/`schedule_open_prs_lookup` in `event_loop.rs`, riding the
existing `GIT_POLL` tick. `draw_worktrees` in `ui.rs` was rewritten from a straight-line renderer into a
`WorktreeEntry` virtual-row layout with a follow-window (`worktrees_scroll`/`worktrees_anchor`, wheel
support) mirroring `draw_sessions`, and grew an `OPEN PRS · N` group. `PaletteTarget::PullRequest(url)`
makes them fuzzy-findable; `jump_to_target` opens the browser instead of moving a cursor.

**Gotchas:**
- **The whole design hinges on PR rows sorting *after* the checkouts.** `sel_worktree` now indexes the
  combined list, so every existing `visible_worktrees().iter().position(...)` (there are ~8 of them, in
  `restore_context`, `reconcile_selection`, `jump_to_target`, `select_worktree_by_id`, the saved UI
  state) stays correct untouched, and `selected_worktree()` returns `None` on a PR row for free — which
  is what makes `p`/`d`/`e`/`n` silently no-op there instead of acting on the wrong worktree. Only
  `move_selection` and `clamp_selections` needed the new `worktree_row_count()`.
- `restore_session` must NOT run when the cursor lands on a PR row — it detaches the terminal when
  `selected_worktree()` is None, so arrowing into the group would blank the pane. Guarded in both
  `move_selection` and the left-click handler. The Sessions panel does go empty there; that's the
  project-divider precedent (`divider_focused()`), not a bug.
- **`gh pr list` returns a bare JSON array**, not an object — `parse_list` takes `as_array()`, unlike
  `parse` which reads fields off a map. Verified against `gh pr list -R cli/cli`.
- `Some(vec![])` (repo genuinely has nothing open) and `None` (no `gh`, no remote, timeout) must stay
  distinct: `None` keeps the last good list on screen rather than blanking the group over one flaky
  round trip. Both back off, `OPEN_PRS_RECHECK_MIN` 30s → `OPEN_PRS_RECHECK_MAX` 10min; a non-empty
  answer settles on `OPEN_PRS_REFRESH` — **60s as of the retirement entry above, not the 3min this
  entry originally shipped**.
- Rate-limit floor: `schedule_open_prs_lookup` pulls the deadline in to `at + OPEN_PRS_MIN_AGE` (30s)
  instead of clearing the entry the way `schedule_pr_lookup` does for worktrees — otherwise bouncing
  between two projects spends an API call per switch.
- The double-click chain (`app.last_session_click`) is shared with the Sessions panel's link rows. A
  click on a *checkout* row has to clear it, or click-PR → click-worktree → click-PR reads as a
  double-click and launches the browser.
- `open_url` short-circuits to `true` under `cfg!(test)`, so Enter/double-click paths are safe to test
  end-to-end — assert on `app.flash == "opened github.com/o/r/pull/7"`.
- The two `clippy::type_complexity` warnings in `ui.rs` (2316, 2446) pre-date this work — the worktrees
  tuple annotation is unchanged from HEAD. `e2e_tui::tui_projects_worktrees_agents_navigation`, recorded
  below as failing at origin/main, **now passes** (6/6 green).

### Claude Cloud Launch From The Session Picker — 2026-08-24

**Asked:** "ok add in an option so a user can press tab when hovered over the claude option in the new
session harness selection modal, and when they press tab, it should toggle claude cloud which will mean
now when they press enter, it'll show 1 more dialog prompt so a user can type their prompt and then invoke
claude using the --cloud argument, then that will launch claude with --cloud and their prompt, make sure
the prompt word wraps and allows a user to read multiple lines when they are prompting."

**Did:** `ContextMenu::toggle_hovered_claude_cloud` in `crates/nebula-tui/src/app.rs` and the menu key
path in `event_loop.rs` make `Tab` toggle `Claude · cloud`; Cloud bypasses the bare-Claude prewarm and
opens `PromptKind::ClaudeCloudTask`, a wrapped multi-row editor rendered by
`ui.rs::multiline_input_lines` (`Shift+Enter` or legacy-terminal-safe `Ctrl+J` inserts a hard line).
`ClientRequest::CreateAgent` carries
a request-only `cloud_prompt` across protocol v24; `registry.rs::claude_cloud_spawn_command` launches a
fresh `claude --cloud=<task>` and validates the task before persistence. Failed requests reopen the
populated editor, failed synchronous spawns roll back the agent row, and server logs include request and
launch metadata without the task. README documents the flow. Full workspace suite: 563 tests green.

**Gotchas:**
- A Cloud create must never adopt or refill the normal Claude warm slot: that PTY already started bare
  and cannot retroactively receive `--cloud` plus the task.
- Claude 2.1.241 declares `--cloud [description|session_id|url]` as an optional-value flag. Bind the task
  as one `--cloud=<task>` argv item; passing a dash-prefixed task as the next item can turn it into a
  different Claude option.
- The login-shell wrapper puts the task in both its `-c` string and Claude's argv. TUI and daemon both
  reject NUL and tasks over 16 KiB before inserting a row, and shell quoting prevents injection, but the
  task can still be visible to local process inspection — do not put secrets in it.
- The Cloud task is intentionally request-only, not persisted on `Agent`: a later restart follows the
  established local Claude resume/fresh path. Only a synchronous create error retains the in-memory
  draft and reopens it for retry.

### Workspaces Are Per Instance, Not Daemon-Global — 2026-08-24

**Asked:** "when I load up 2 separate nebula instances, they both seem to switch workspaces when one
does... this isn't how it should work, each new nebula instance can point to a different workspace (or
even point to a separate host - verify that is possible)" Then: "please verify that this issue won't
happen again when I'm doing development" and "update nebula-memory as well after you've confirmed this
is fixed."

**Did:** The open workspace was daemon-global: `store.set_active_workspace` plus a
`ServerEvent::ActiveWorkspaceChanged` broadcast every client applied. Deleted that event outright
(PROTOCOL_VERSION **22 → 23**) and moved the scope onto the connection — `handle_client` in
`crates/nebula-daemon/src/server.rs` holds `workspace: Option<WorkspaceId>`, `OpenWorkspace` sets it,
and `add_project` takes it as a new 4th arg. `registry.rs::open_workspace` became
`set_default_workspace` (persists, notifies nobody). TUI-side, `switch_workspace` and
`reseat_deleted_workspace` in `event_loop.rs` replaced the removed `ActiveWorkspaceChanged` arm, and
`apply_startup_workspace` lands the new `nebula --workspace <name>` flag on the first snapshot.
Separate hosts already worked and needed no change (see gotchas).

**Gotchas:**
- **Per-connection state alone is not enough; it has to be pinned at `Subscribe`.** The first cut left
  `workspace = None` until a client switched, falling back to the store default — so instance B, which
  had never touched its workspace, silently followed A's switch on its next `AddProject`. "The current
  default" is not a stable answer once anyone can move it. `e2e_pty::workspace_scope_is_per_connection`
  is the test that caught it; it fails on the None-fallback version.
- `None` still has to survive for connections that **never** subscribe — that is the one-shot
  `nebula add`, whose workspace genuinely is the current default. Don't default it at connect time.
- `apply_startup_workspace` must run **before** `restore_ui_state` in the Snapshot arm: the restored
  project id only resolves against the workspace actually on screen.
- Semantics that changed and are now documented: **`nebula workspace open <name>` no longer switches a
  running TUI.** It sets where the *next* instance boots. Aiming one live window is `nebula --workspace
  <name>`. Removing the broadcast is exactly the reported bug, so there was no way to keep both.
- **Separate hosts already work and are fully independent** — `nebula ssh HOST` (`crates/nebula/src/ssh.rs`)
  `exec`s `ssh -t` and runs a whole remote nebula with its own daemon, socket and SQLite. Two local
  instances against different daemons also work via `NEBULA_RUNTIME_DIR` + `NEBULA_DATA_DIR`. Note the
  TUI's `h` picker is a *handoff*: it quits the local TUI and execs over it rather than opening a second
  window.
- `crates/nebula/tests/e2e_tui.rs`'s `FOOTER_TERMINAL_LOCKED` had been stale since `87d2b24` — it
  expected `"Ctrl+q: panels"` while `KeyChord::display()` renders `^q`. Six e2e_tui tests were failing on
  main for that alone; fixed to `"^q: panels"`. If e2e_tui fails on a footer string, suspect the constant
  before the code.
- Verified live, not just by unit test: two TUIs in one tmux server against an isolated daemon
  (`NEBULA_RUNTIME_DIR=/tmp/nbws`, short for SUN_LEN). Pane 0 pressed `w j Enter` → footer `◇ client`
  showing its project; pane 1 stayed `◇ default` showing its own. Full suite: 553 tests green.

### Shift+G Opens The Repo's Git Host, Released As v0.3.0 — 2026-08-24

**Asked:** "is there a release skill in this repo?", then "commit and push and do another release", then
"make a skill called release which kicks in and does these similar steps the next time someone asks".

**Did:** Released **v0.3.0** — `c553409`, tag pushed, all four binaries attached. Feature commit
`b00ce46` adds `crates/nebula-tui/src/remote.rs` (`repo_url`, `web_url`) plus `open_repo_in_browser`
in `event_loop.rs`, bound to `Action::OpenRepo` / `shift+g`. `ef56fca` checks in `CLAUDE.md`,
`.claude/MEMORY.md`, and the new `.claude/skills/release/SKILL.md`.

**Gotchas:**
- **Another agent was editing the same tree the entire time**, mid-way through a `--workspace` feature:
  `protocol.rs`, `registry.rs`, `server.rs`, `app.rs`, `ipc.rs`, `main.rs`, `e2e_pty.rs` all turned
  modified while this task ran. It bit three separate ways — (a) `git add` on `event_loop.rs` captured
  **66 lines when the reviewed change was 56**, silently dragging in their
  `run_app(workspace: Option<String>)`; (b) the shared index was **reset out from under a staged
  commit**, so `git commit` answered "no changes added to commit"; (c) a `git worktree add` under the
  scratchpad was **pruned away while in use**. What worked: do the whole release in a private worktree
  on its own branch and `git push origin <branch>:main`. **Never `git add` in the shared tree.**
- Local `main` stays behind `origin/main` after that push — it is checked out and dirty, so it can't be
  fast-forwarded. Say so explicitly; the next `git pull` has to reconcile.
- `e2e_tui::tui_projects_worktrees_agents_navigation` **failed at `origin/main` too** at the time:
  `FOOTER_TERMINAL_LOCKED = "Ctrl+q: panels"` (`crates/nebula/tests/e2e_tui.rs:29`) while the footer
  rendered `^q: panels`. Introduced by `87d2b24` and shipped red in v0.2.0. **Fixed since — the whole
  e2e_tui suite is 6/6 green as of 2026-08-24.** The standing lesson: always re-run a failing test
  against `origin/main` before blaming your own diff.
- `.github/workflows/release.yml` publishes with `generate_release_notes: true`, which is a bare commit
  list, not a changelog. `gh release edit vX.Y.Z --notes "…"` afterwards is the step that makes it one.

### Project Memory System — 2026-08-24

**Asked:** "update claude.md to invoke a skill called nebula-memory which has instructions on how an
agent should summarize the original request, how we fixed or implemnted it, and any gotchya you ran
into along the way. update the claude.md to instruct agents to read the memory.md file that the skill
updates …" — then: "go through all previous sessions for this project and invoke the nebula-memory
skill starting with oldest last so we can document how we grew this project."

**Did:** Created `CLAUDE.md` (none existed — only an empty `CLAUDE.local.md`), the
`.claude/skills/nebula-memory/` skill, and this file. Backfilled the entries below.

**Gotchas:**
- Real user prompts are recoverable from the transcripts by filtering `type=="user"` **and**
  `promptSource=="typed"` **and** `origin.kind=="human"`. Without that filter you get 8544 tool-result
  records instead of 258 prompts.
- ~12 sessions in this project's transcript dir are not nebula work at all — they are Cartastrophe game
  sessions and one-off test prompts that happened to run from this cwd. Filter by content, not by directory.

### Sessions Ordered By Last Interaction — 2026-08-24

**Asked:** "order the sessions by last interaction date, also display a time last interacted next to the
session title to right but left of harness name, so the workflow is a session runs goes to top of list,
if anything else iteracts it would go top. when displaying the last interaction time just show '23m ago…'"
Follow-up: "commit and push, then release with good change log with detials on what changed, make release
skill when done to follow these steps." Related earlier ask (2c58d9c1): running / awaiting-feedback
sessions always pin to the top of the Recent list.

**Did:** Sessions sort by last-interaction timestamp with a relative age label; released as `c340baf`
(v0.2.0).

### Rebindable Hotkeys And Settings Tabs — 2026-08-24

**Asked:** "in the settings add a top tabs which a user can use arrows or tabs to navigate though.
challenge my prompt, pick the best user experience. make good tab categories for where to put settings.
now I need you to add in a setting for hotkeys, allow a user to customize ANY HOTKEY in the application…"

**Did:** New `crates/nebula-tui/src/keymap.rs` holds the rebindable key table; settings overlay grew
tabs. Landed in `87d2b24` alongside the cancel-status fix.

**Gotchas:**
- The user explicitly invited pushback ("challenge my prompt") — this is a standing preference on UX
  asks, not a one-off.

### Worktree Names With Spaces, Random Branch Names — 2026-08-24

**Asked:** "when I create a worktree name, allow a user to type in spaces in the worktree name but you
must convert the spaces to hyphens. also allow a user to just enter on the branch which will pick a
random branch name using three words combined such as yellow-fox-jumps <adj>-<noun>-<verb>"

**Did:** Added `crates/nebula-tui/src/branch_name.rs` for the `<adj>-<noun>-<verb>` generator; the
worktree name field slugifies spaces to hyphens.

### PR Links And New-Comment Counts — 2026-08-23 → 08-24

**Asked:** "I noticed that one of my sessions created a pull request but that link was not auto detected,
I think when I switch to a worktree you should run a background process to check if any pull request are
open and show them as links…" Then: "if possible, track how many NEW comments were added since the last
click on a pull request link, it would be nice to see when others have left comments…"

**Did:** `crates/nebula-tui/src/pull_request.rs` plus a `pr_seen` read-marker map on `App`
(`app.rs:1718`). Links pin to a worktree; commit `44bd270`.

**Gotchas:**
- `gh pr view --json comments,reviews`: `comments[]` has **`viewerDidAuthor`**, `reviews[]` does **not** —
  telling your own reviews apart needs `gh api user --jq .login`. Inline per-line review comments aren't
  exposed as a `--json` field at all; counting review submissions is the cheap approximation.
- Both timestamps are RFC 3339 UTC, which sorts **lexicographically in chronological order**. `pr_seen`
  stores the newest stamp seen at open time, so "newer than X" is a string compare — no clock, no date
  parsing, and no `chrono`/`time` dependency added to a deliberately dep-light workspace. Empty string
  works as the sentinel because every real stamp sorts above it.

### Cancelling Claude Left The Status Stuck — 2026-08-23

**Asked:** "I noticed that when I cancel Claude code, it never actually changed the status back to green
from that yellow animation. Can you debug and fix this?"

**Did:** Added `crates/nebula-daemon/src/pty/progress.rs`, which scans the PTY byte stream for OSC 9;4
progress edges; the pump emits `PtyEvent::Progress` and `status.rs` treats "progress cleared" as a
synthetic `Stop` (same subagent-drain bookkeeping), but only from Running/NeedsFeedback.

**Gotchas:**
- Esc-cancelling a Claude turn fires **no hook at all**. `Stop` is documented not to run on user
  interrupt, and the `idle_prompt` Notification that normally rescues a hookless turn end is suppressed
  because Claude gates it on 60s quiet **AND** the user not having touched the keyboard — pressing Esc
  *is* touching it. Verified against Claude Code 2.1.241 with a `pty.fork` harness; only
  `UserPromptSubmit` then `SessionEnd` ever fired.
- The window **title** is unusable as a busy/idle signal — during a permission prompt it shows idle (`✳`)
  while the OSC 9;4 progress state correctly stays busy (`3`). Trust the progress state, never the title,
  or you will green out an agent that is waiting on the user.
- Codex and cursor-agent emit no OSC 9;4 at all, so this path is inert for them.

### Shared Working Tree Is Raced By Other Sessions — 2026-08-23

**Asked:** (no prompt — surfaced mid-task) A `git stash push -m hotkey-wip` + pop cycle from **another**
Claude session reverted and then restored every uncommitted file mid-edit, and the pop left three
duplicated `activity:` fields in `event_loop.rs` test fixtures.

**Did:** Nothing to commit — recorded as a working rule.

**Gotchas:**
- The user runs nebula's own agents against this repo, so the main tree is routinely mid-refactor from
  someone else. A `cargo check`/`cargo test` failure often has nothing to do with your change — check
  whether the failing symbols belong to unrelated in-flight work before blaming your own edit.
- Re-verify your edits are still on disk after any unexplained state change. Never `git stash pop` or
  `git checkout` the shared tree on your own judgment.
- A self-contained new module can be checked in isolation with `rustc --test --edition 2021 <file>` when
  the crate as a whole won't build.

### MIT License And Dependency Audit — 2026-08-23

**Asked:** "change to MIT license" — then, separately: "is https://ratatui.rs/ used on this project? what
third party lib do we use?" and "verify we are on the latest version of all of these, and also verify they
are all MIT license or able to be used on this MIT tui I'm making."

**Did:** Added `LICENSE` (MIT) and audited workspace dependency licenses.

### Releases So The Installer Stops Falling Back To Cargo — 2026-08-22

**Asked:** "no prebuilt binary for this platform yet — falling back to cargo... fix. also update readme to
walk user how to use this"

**Did:** Cut real GitHub releases with binaries (`bcaa104`, then `4ddcc7e` v0.1.1, `0c178e2` v0.1.2) so
`install.sh` finds an artifact instead of building from source.

**Gotchas:**
- Two `gh` accounts are logged in. `webdevcody` is the admin; `codyseibert` has only READ on
  `AgentSystemLabs/nebula` and fails write calls with "must be a collaborator (createPullRequest)".
  **As of 2026-08-24 `webdevcody` is the active account** (it was `codyseibert` on 08-22, so check
  rather than assume): `gh auth status`, and `gh auth switch --hostname github.com --user webdevcody`
  if it has drifted back. `git push` is unaffected either way: it goes over SSH, not the gh token.

### Codex Hooks Moved To ~/.codex — 2026-08-22

**Asked:** (follow-on from the Aug 14 codex work — codex sessions still weren't reporting status)

**Did:** `22f1b24` moved codex's hooks to `$CODEX_HOME/hooks.json` and started trusting `idle_prompt`.

**Gotchas:**
- Codex gates hooks behind a trust modal keyed by the **hook file's absolute path**, recorded in
  `~/.codex/config.toml` under `[hooks.state."<abs path>:<snake_case event>:<group idx>:<hook idx>"]` as
  `trusted_hash = "sha256:…"` — **not** a plain sha256 of the command string, so don't try to precompute
  it. A project-local `.codex/hooks.json` therefore re-prompts in every new worktree, and an unanswered
  prompt means the hooks never run at all. `$CODEX_HOME/hooks.json` is a stable path → one approval
  covers everything.
- Codex discards raw stdout from hooks. Context injection only works through
  `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"…"}}`. Claude Code
  accepts that same envelope, so one response body serves both.
- `codex exec` **does** run hooks once trusted, so it's a fast harness — but it can't answer the trust
  modal, so grant trust first with one interactive run.

### Real Line Editing In Typed Fields — 2026-08-22

**Asked:** (session ran on branch `fixing-input-ux`, merged as PR #1)

**Did:** `cd07baa` gave every typed field real terminal line-editing.

### Workspaces And The o/t/e Hotkey Remap — 2026-08-21

**Asked:** "add the ability to do a nebula workspace add <name> and then later nebula workspace open
<workspace_name>, then all projects will scoped to that workspace. make sure the / fuzzy find doesn't
search over all workspaces. also include a workspace list and workspace delete and workspace rename…"
Separately, on keys: "right now I often press o to open a new project accidently and that opens the
notes… on the nebula landing screen… my first instinct was to press o to open a new project" →
"change the new terminal hotkey to t, and change the todos to instead just be e hotkey for not(e)s,
refactor the language so instead of it being todos it's just notes."

**Did:** `77a87ca` (workspaces, respawn moved agents, o/t/b remap) and `4bea626` (todos → notes, ssh host
picker, note badge glyph).

**Gotchas:**
- A workspace is **just a grouping of projects** — the same project may belong to several. An early
  version refused to add a project that already existed in another workspace; the user rejected that
  ("we should be able to add any projects to any workspaces").
- The user twice asked for the key-combo hints to be rendered at the bottom of a modal rather than behind
  submenus ("nah I'd rather it just show r and d in the bottom of the workspace panel like we do for the
  notes, we should need all these sub menus"). Follow the notes-modal pattern for any new modal.

### e2e Daemon-Boot Failures Have Two Different Causes — 2026-08-21 → 08-23

**Asked:** (no prompt — both surfaced while verifying other work)

**Did:** Nothing to commit. Both are environmental, and telling them apart saves hours.

**Gotchas:**
- **Cold-exec flake.** All 16 `e2e_pty` tests fail with `daemon socket never appeared`. First exec of a
  freshly relinked `target/debug/nebula` can stall for seconds on macOS signature validation, so the test
  panics at its 5s deadline, `TempDir` drop deletes the runtime dir, and the late daemon logs
  `FATAL bind …/daemon.sock: No such file or directory`. Fingerprint: orphaned
  `$TMPDIR/.tmp*/data/state/daemon.log` files. **Just rerun** — it passes clean the second time.
- **Orphaned daemons — leak fixed at the source 2026-08-24.** Same generic error, but **no `daemon.log`
  is written at all** and reruns don't help; a test that passes in the full suite fails alone, seemingly
  at random. Cause: dozens of stray `nebula daemon --foreground` processes, each holding watchers/fds.
  The leak was `e2e_pty.rs`'s `TestEnv` having **no `Drop`** — a test that panicked before its closing
  `Shutdown` dropped the `std::process::Child` without killing it, and the daemon detaches and outlives
  the whole `cargo test` run. `DaemonProc` (a `Deref`/`DerefMut` newtype around the `Child`, defined just
  above `connect()`) now SIGTERMs on drop, so panicking tests clean up. `e2e_tui.rs`'s `TuiHarness`
  always had its own `Drop`. Nothing should accumulate any more — **62 had piled up before this**, so a
  machine that predates the fix may still need one reap.
- Diagnosing a suspected orphan pile: `ps -eo pid,command | grep -c "[n]ebula daemon"`. Anything past a
  couple means leftovers. Reaping is safe **except for the live one** — filter to `target/debug/nebula
  daemon` (test daemons) and exclude `$(cat /tmp/nebula-501/daemon.pid)`, which is the
  `~/.cargo/bin/nebula daemon` running the session you are inside. Ask before bulk-killing: it's the
  user's machine and other agents' e2e runs may be in flight.
- `DaemonProc::drop` sends **SIGTERM, not SIGKILL** — the daemon's handler runs the same clean shutdown
  as `ClientRequest::Shutdown` and takes its PTY children with it; SIGKILL would orphan those instead.
  It also `try_wait()`s first: `Child::kill()` on an already-reaped child errors rather than signalling
  whatever now owns that recycled pid, but the check keeps the intent obvious. Drop only runs because
  the workspace has no `panic = "abort"` profile — verify that before relying on a drop guard here.

### Restyle, Focus Wash, And The Screenshot Harness — 2026-08-20 → 08-21

**Asked:** A run of visual passes: "would it be possible to space out the items in the projects worktrees
and sessions lists? like to make them feel like larger buttons, also visual hieachy…", "when a list panel
is in focus, render a themed gradient that comes up from the bottom, but very subtle…" → "the bottom focus
gradient looks like shit... let's think of a differnt indicator… maybe just make the entire panel a very
lightly colored (like 10% opactiy) theme color", and "when a session is running (when it's yellow status
or red), make the text animate with colors… it should be a sweeping animation."

**Did:** `d704da7` (borderless columns, raised-fill selection, quiet chrome) plus the animation pass, with
a settings toggle to disable animations for CPU.

**Gotchas (recipe for screenshotting the TUI with demo data):**
- Isolate with `NEBULA_RUNTIME_DIR=/tmp/<short>` (SUN_LEN!) and `NEBULA_DATA_DIR=<scratch>/demo/data`.
  Never touch the real daemon — and note the daemon **detaches and outlives the tmux server**, so
  `kill $(cat $NEBULA_RUNTIME_DIR/daemon.pid)` when done.
- **Set `NEBULA_AGENT_CMD` even if you never create an agent** — the warm-slot prewarm launches a real
  `claude` on its own (shows as "1 agent · ~600MB" with zero agent rows in the DB). `/bin/cat` works.
- **One Bash call per drive**: the sandbox kills the private tmux server when the tool call ends, so
  new-session, send-keys, captures and kill-server must all happen in a single call. Send one key per
  call with 0.3–1s sleeps — batched keystrokes concatenate into the name prompt.
- `tmux capture-pane -epN` — **without `-N`** tmux trims trailing styled spaces and any background fill on
  the rightmost pane silently vanishes from the capture.
- Color and animation checks don't need PNGs: `capture-pane -ep` keeps SGR escapes; decode with
  `LC_ALL=C sed 's/\x1b\[/¶/g'` and grep for `38;5;N`, capturing 2–3 frames ~350ms apart to prove motion.
- Chrome headless gets SIGKILLed on this Mac and charmbracelet freeze wrecks the cell grid — use a small
  pillow grid renderer instead.

### Sessions Auto-Rename Themselves — 2026-08-20

**Asked:** "add some type of hook into nebula and ability for claude to automatically rename the session,
update the system prompt to use the skill to tell nebula to rename the session after the initial prompt
was submitted, we should be able to creat a title between 3-4 words that describe the ask of the promp…"

**Did:** A `UserPromptSubmit` hook injects an instruction telling the agent to run `nebula rename <title>`.
Later extended to codex ("it doesn't seem lke when I send a prompt to codex it updates the session title…
look into how we do it for claude code and replicate that behavior").

**Gotchas:**
- This is why every session in this repo issues a `nebula rename` before doing anything. It is injected
  context, not something the user typed — don't mistake it for part of the request.

### Cursor's Hooks Are Not Claude-Shaped — 2026-08-20

**Asked:** "cursor doesn't seem to update the status of the wortree or sessions when it is running, debug
and fix, verify it has hooks, if not, then setup some type of skill that is injected to cursor as a system
prompt or something so that it knows how to phone home to nebula to update the status"

**Did:** `install_cursor_hooks` in `hooks/installer.rs:260` is its own writer (plus a migration purge of
nebula groups under every key), and the installer maps cursor event names onto Claude-equivalent
`hookEvent` query values so `parse_event` stays single-dialect. `HookPayload` in `hooks/mod.rs` grew
aliases.

**Gotchas:**
- The installer originally assumed "same hooks JSON shape across all three CLIs". Cursor **silently
  ignored** the PascalCase Claude-shaped groups, so no status ever phoned home — no error, just nothing.
- Cursor's dialect: camelCase events (`sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart/Stop`,
  `sessionEnd`), **flat** `{"command": …}` entries (no nested `hooks` array, no `type`), and a required
  top-level `"version": 1`. Hooks must print `{"continue": true}` to stdout or gating events degrade.
- Payloads carry `session_id` == `conversation_id` (the `--resume` chatId), have **no `cwd`** (use
  `workspace_roots[0]`), and subagent hooks use `subagent_id`, not `agent_id`.
- `beforeSubmitPrompt` and `stop` fire **only in interactive TUI mode**. A `-p` print-mode test fires only
  sessionStart / tool hooks / afterAgentThought / sessionEnd — **never conclude hooks are broken from a
  `-p` test**. To drive one interactively: pipe timed keystrokes through
  `script -q /dev/null cursor-agent --force --trust`.

### Idle Session Reaping And Metrics — 2026-08-20

**Asked:** "right now when a user opens a session, it takes some time I think for nebula to connect maybe
to the server and actually show the terminal... can we find a way to prefetch these connections…" → "add
logic to auto suspend or kill claude sessions that are not in focus…" → then the user pushed back on their
own idea: "I'm concerned now because some claude sessions might have schedules or long running jobs and I
don't want them killed.... is the latest change potentially breaking that requirement?" → "ok for now
never reap pinned sessions, also make this entire reap process a setting configurstion to just turn it
off." Alongside: "add some type of metrics modal which will show the overal usage of nebula combined with
all the other terminals open, including memory usage for individual and overall."

**Did:** `e11f838` — idle reaping, metrics tracking, memory stats in the footer.

**Gotchas:**
- **Pinned sessions are never reaped**, and reaping is switchable off entirely. That constraint came from
  the user realizing mid-feature that agents may be running long jobs — treat it as load-bearing.

### The Daemon Needs Its Own Session, Not Just A Process Group — 2026-08-20

**Asked:** "sometimes nebula will enter this state when I try to start a new claude terminal, it just
keeps writing strange tokens and the entire app is broken basically, I can't interact, it just happened
in a previous session I tried to open"

**Did:** `4502575`. `spawn_daemon` in `crates/nebula-tui/src/ipc.rs` now calls `setsid()` in `pre_exec`
instead of only creating a new process group, so the daemon holds **no controlling terminal** and nothing
it spawns can reach the user's terminal through `/dev/tty`. The `zsh -l -i -c "command -v claude"` CLI
probe in `nebula-daemon/src/registry.rs` also `setsid()`s (so even a `--foreground` daemon can't have the
probe shell steal a tty) and gained `.kill_on_drop(true)` — previously a hung probe leaked the child
forever when the 5s timeout dropped the future.

**Gotchas:**
- The garbage tokens were a **shell job-control fight over the controlling terminal**, not a rendering or
  vt100 bug. A new process group is not enough; it must be a new *session*.
- With no controlling tty, zsh's `/dev/tty` open fails and it skips job-control init entirely — that's the
  mechanism, and it's why the fix is one call in the right place.

### `zsh: killed` Is A Stale Code Signature, Not A Rust Bug — 2026-08-20

**Asked:** "debug why when I run nebula if fails … `nebula upgrade` → `zsh: killed nebula upgrade` …
`nebula` → `zsh: killed nebula`" Same thread: "nebula fails when I try to run it, give me hte proper
commands I should run locally to use the latest built version" → "make that into a single script and maybe
a makefile" → "rename kill-server to just kill, do that everywhere kill-server is too verbose."

**Did:** Added the `Makefile` for the local dev loop and renamed `kill-server` → `kill`.

**Gotchas:**
- The crash report says `SIGKILL (Code Signature Invalid)` / `Taskgated Invalid Signature` **even though
  `codesign -vv ~/.cargo/bin/nebula` reports valid on disk**. Cause: `cargo install --path` rewrote the
  binary **in place (same inode)** while the kernel held a cached signing blob for that vnode, so every
  later exec was killed.
- Fix is to refresh the inode, not the code:
  `cp ~/.cargo/bin/nebula ~/.cargo/bin/nebula.new && mv -f ~/.cargo/bin/nebula.new ~/.cargo/bin/nebula`.
  Identical bytes on a fresh inode exec fine.
- Confirm before debugging anything else: `~/Library/Logs/DiagnosticReports/nebula-*.ips`.
- A lingering `nebula daemon` from the old inode keeps running **old code**. `nebula kill` is the user's
  call — it stops live sessions.

### In-TUI File Tooling — 2026-08-19

**Asked:** Four asks in one evening: "when a user presses f show a fuzzy file finder…", "add the ability
for a user to press a hotkey to show a find in files search, basically it should run grep over the code
base… when a user presses enter it should show a vim terminal to allow editing that file, that vim
terminal must be a modal inside this app", "when claude code prints file paths, I want to be able to do a
option click… to actually open that file directly inside a file viewer (vim) inside nebula", and "add a
hotkey for t which shows a full tree browser modal with a view of the file content on the right…" →
refined to "in the file preview, it should be syntax highlighted, also when I select the file, it
shouldn't open a new vim modal, the right panel should just focus and let editing with vim."

**Did:** `998901f` (file finder, grep overlay, path links, in-TUI editor via
`crates/nebula-tui/src/vim_term.rs`) and `7ebc264` (tree browser with live filter and syntax preview).
Later `6787999` numbered the lines in file previews but not directory listings. The editor command is
configurable — the user asked for neovim support explicitly.

### Crash Logging — 2026-08-19

**Asked:** "make sure all errors in nebula are logged into a .log file somewhere so that I can debug when
it crashes. so far i've seen nebula randomly close out and crash twice now when trying to create a new
claude session, but I'm not sure how to debug"

**Did:** `71e62c7` — panic logging for both the TUI and the daemon.

**Gotchas:**
- Worth knowing that the "random crashes on new claude session" the user was chasing here were most likely
  the two separate problems diagnosed the next day: the stale code signature and the controlling-terminal
  fight. Crash logging is what made both findable.

### nebula ssh And Remote Hosts — 2026-08-19 → 08-21

**Asked:** "add a way for someone to launch nebula from the cli into a remote ssh. assume ssh keys already
allow access to the remachine. so something like nebula ssh HOST and when we get into the machine it
should install nebula if it doesn't already exist on the machine (remote exec of a script)…" Later: "add a
built in way so that nebula remembers the hosts you've recently done `nebula ssh` with so that a user can
press h to view all the hosts…"

**Did:** `8ddad36` (remote hosts, user config with settings overlay, fuzzy diff filtering) and the host
picker in `4bea626`.

**Gotchas:**
- The user also had to enable inbound ssh on this laptop to test it, and explicitly asked to confirm it
  was **local-network only, nothing from the public internet**. Don't widen that.

### Sessions Re-Home Into The Worktree They Create — 2026-08-18 → 08-24

**Asked:** "sometimes I'll be on the main root worktree and I'll start a session, and inside that session
I'll prompt it to do the work inside a worktree, which claude or codex will then create the worktree. if
possible, when this happens I want to move the session out of that main worktree root and move it to…"
Later, twice more: "there is a strange bug where … after I manually move a session to that work tree, at
some point in the future that original session seems to switch back to whatever worktree it originally
was…" and "the session takes a while before it is moved into the worktree… is there a way to make
automatically move…"

**Did:** `7570387` re-homes an agent row by hook-reported cwd. The cwd probe is the
`("PostToolUse", Some("Bash|EnterWorktree|ExitWorktree"))` matcher in `hooks/installer.rs`.

**Gotchas:**
- Claude uses its own **EnterWorktree** tool, not `git worktree add`. That creates a **locked** worktree
  at `<repo>/.claude/worktrees/<name>` on branch `worktree-<name>`.
- A Bash `cd` to a directory **outside the session's workspace root is silently reset** ("Shell cwd was
  reset to …") and the hook cwd never changes. So nebula's own sibling layout
  (`<repo>/../<repo>-worktrees/<branch>`, `git.rs` `worktree_dir`) is unreachable by cwd-following — only
  checkouts *inside* the repo re-home.
- Before the `EnterWorktree` matcher existed, the row only moved at the turn's `Stop` — measured **~34s
  late**, which is exactly the "takes a while" the user reported.
- **Hooks are snapshotted at session start**, so any hook-set change only reaches newly spawned sessions.

### Cmd+P Never Reaches The Agent In Terminal.app — 2026-08-18

**Asked:** "when I try command + p in a claude session, it just pastes the pi character and recommends I
run /setup-terminal which I already have, can you figure out if maybe command + p is not properly being
sent to the claude session? this is inside a terminal.app I'm running nebula. this works perfectly fine…"

**Did:** No code change — diagnosed as not-a-nebula-bug and gave remedies.

**Gotchas:**
- Terminal.app **never encodes Cmd into pty bytes** (⌘P is File→Print at the menu layer). The press
  arrives as Option+P's character `π`. Nebula's chain was verified sound end to end: kitty probe in
  `event_loop.rs` setup_terminal → legacy encoder swallows SUPER (`keys.rs` `encode_legacy`) → kitty
  re-encode would have sent `\x1b[112;9u`.
- Agent PTYs get `TERM=xterm-256color` (`pty/mod.rs`) but inherit the **daemon's** `TERM_PROGRAM`, so
  `/terminal-setup` run inside nebula detects whatever terminal the daemon was first spawned from, not
  the one currently attached.
- Remedy given: `/model` opens the same picker, or bind `ctrl+p` → `chat:modelPicker` in
  `~/.claude/keybindings.json`.

### Wheel Scrollback Vs Claude's Alt Screen — 2026-08-18 → 08-21

**Asked:** "when I scroll on my mouse wheel know (or track pad), it doesn't seem to scroll back in the
terminal session output, it instead just switches my previous entered prompts in the input" — and again
later: "…it instead it says 'Scroll wheel is sending arrow keys · use PgUp/PgDn to scroll' and it just
keeps showing previous prompts I'm using, how do I fix that"

**Did:** `handle_mouse` in `event_loop.rs` (see `mouse_protocol_mode` at `event_loop.rs:5199`) now
forwards a real SGR wheel report (`\x1b[<64;col;rowM` / 65) at the 1-based pane cell whenever
`screen.mouse_protocol_mode() != None`; arrow synthesis remains only for mouseless alt-screen apps
(plain vim/less).

**Gotchas:**
- Claude Code 2.1.x renders its main UI on the alternate screen and enables mouse tracking
  `?1000h ?1002h ?1003h ?1006h` **in the same write as** `?1049h`, so a vt100 replay sees both or neither.
- The old arrow-synthesis fallback is what triggered Claude's own `arrow-burst` detector and that warning
  banner. Check the child's mouse protocol mode in the vendored vt100 before assuming arrows are right.

### Optimistic Worktree Deletes And Stale Locks — 2026-08-18

**Asked:** "add some type of background task for deleting worktrees, I notice when i try to delete a
worktree, it often freezes up for a bit until it finally removes the worktree, I'd like it to do
optimistic client updates for when it's deleted and rollback if it fails…" Plus: "I'm trying to delete a
worktree and it says 'cannot remove a locked working tree, lock reason: claude session
menu-enable-level'. when I try to delete a worktree, it should force kill and remove any locked sessions…"

**Did:** `d214366` — deletes are optimistic with rollback, and stale session locks are force-unlocked.

**Gotchas:**
- The lock is not nebula's; Claude's EnterWorktree creates locked worktrees, so `git worktree remove`
  refuses until the lock is cleared.

### Codex And Cursor As Agent Kinds — 2026-08-14 → 08-15

**Asked:** "add support for codex as well, so when a try to load up a new session using the n hotkey, show
a modal that let's me pick codex or claude, make sure the codex setup has the proper hooks or whatever
else instlaled like we do in claude so that the status indicators can properly reflect the state of th…"
Then: "also add support for cursor cli as a session option" and "run codex with --yolo mode on codex
sessions, same with cursor if it has a type of yolo flag see how we do it on mission-control."

**Did:** `AgentKind` + a picker modal (`5092684`, `986f505`), cursor-agent as a third kind (`f5ed97d`),
permissions always skipped for both (`89f9860`).

**Gotchas:**
- `claude` takes `--model <alias>` and `--effort <low|…|max>`; `codex` takes `-m/--model` but effort only
  via `-c model_reasoning_effort=<…>`; `cursor-agent` has no model/effort knobs. Pick lists are hardcoded
  in `crates/nebula-tui/src/config.rs` (`CLAUDE_MODELS`, `CODEX_MODELS`) — "default" always means
  "pass no flag".
- Cursor has no PermissionRequest hook and nebula runs `cursor-agent --force`, so cursor agents report
  busy/idle but **never** needs-feedback. That is expected, not a bug.

### Vendored vt100 So Codex Scrollback Works — 2026-08-14

**Asked:** "scrolling back using codex doesn't work, but claude works fine, debug and fix"

**Did:** Vendored vt100 0.15.2 into `vendor/vt100` with a one-line semantic change and wired it via
`[patch.crates-io]` in the root `Cargo.toml`, so both `nebula-tui` and `tui-term` pick it up
(`d1d1a50`). Two regression tests in `app.rs` — one replays a codex-style region scroll, and it also
fails if anyone drops the `[patch.crates-io]` wiring.

**Gotchas:**
- The bug was in the parser, not in nebula's scroll handling. Codex is a ratatui **inline-viewport** app:
  it inserts history by setting a top-anchored DECSTBM scroll region (`ESC[1;{viewport_top}r`) and
  scrolling inside it. Stock vt100 0.15.2 **discards** any line scrolled out while a scroll region is
  active (`grid.rs`, `scroll_up`), so codex's scrollback stayed empty. Real terminals keep top-anchored
  region scrolls — which is why codex scrolls fine *outside* nebula.
- `vendor/vt100` is a **patched fork**. Do not upgrade or re-vendor it without re-applying this change.
- Full-screen apps are unaffected: the alternate screen's grid is created with zero scrollback capacity.

### Agents Spawn Through A Login Shell — 2026-08-14

**Asked:** "it seems like new sessions don't use my ~/.zshrc, verify the do on load"

**Did:** `1344cd6` — agents and terminals spawn through a login shell.

**Gotchas:**
- This wrap is why `NEBULA_AGENT_CMD` also has to *skip* it: without that, `~/.zprofile` resets PATH and
  the **real** `claude` CLI launches instead of a test stub.

### Terminals Removed, Then Brought Back — 2026-08-09 → 08-20

**Asked:** "remove the terminal section from the session list, I decided I don't care about terminals as
we can just use claude code to run terminal commands directly" — reversed 11 days later: "add a way to
create a new terminal already in the pwd of the worktree or root, figure out a good key binding for this
as cmd + t will open a new ghostty terminal if I'm using ghostty to run nebula" (`c318eedb`).

**Did:** Removed, then re-added on its own hotkey (`t` after the Aug 21 remap).

**Gotchas:**
- Recorded because the removal reads like a settled decision in the Aug 9 history and is **not** one.
  Don't cite it as precedent.

### Worktree Watcher And Selection Memory — 2026-08-05

**Asked:** "verify we have some type of directory watcher on .worktrees or the github worktrees so that
when a new worktree is created from an agent or manually it'll update the worktrees list automatically.
right now i created a worktree and it did not show up in that list until i restarted nebula" — then:
"change of plans, we should remember the last agent that was selected for that project so that if i
switch between projects it'll automatically just show the last selected worktree & agent…"

**Did:** `91c29c0` (auto-sync + selection restore) and `02bb5a3` (refresh branches on external checkouts).

### Project Dividers And Shift+J/K Reordering — 2026-08-05

**Asked:** "add a way to put dividers between projects, also a way to hold shift and move projects up and
down in regards to their order in the list so that I can group projects together" — then, after the first
attempt only swapped neighbours: "when I do shift j and k, it doesn't seem to move projects under
dividers, it just swaps projects, you must treat a divider as something I can move a project under or
above separate" and, escalating, "I should be able to move a project into any fucking divider I want."

**Did:** `98dc681` — reordering treats dividers as real positions, and dividers are labelable and movable.

**Gotchas:**
- Shift+↑/↓ is **undeliverable in Terminal.app**: `keyMappings.plist` has entries for `$F702`/`$F703`
  (Shift+←/→) but **none** for `$F700`/`$F701`, so Terminal drops the shift and sends a plain arrow.
  Shift+J/K works everywhere because crossterm tags uppercase chars with SHIFT.
- "Move" has to mean move-across-groups, not swap-with-neighbour. The first implementation satisfied the
  literal words and not the request.

### Install Script And The Org Slug — 2026-08-05

**Asked:** "if I wanted to provide one command for anyone to install or update this cli tool, what's the
best way? a .sh script in the repo? I don't want to use some third party registery at this point" →
"do the curl approach and put in the readme" → "why did you make the readme say webdevcody,,, this is part
of the agentsystemlabs org"

**Did:** `install.sh` + README one-liner (`95ac3da`), then `nebula upgrade` (`1c87c06`).

**Gotchas:**
- The repo slug is **`AgentSystemLabs/nebula`**, never `webdevcody/<repo>`. It is hardcoded in
  `install.sh` (`REPO=`) and the README. Assume other repos under `~/Workspace/AgentSystemLabs/` are
  org repos too.

### iTerm Swallowed Option+Delete — 2026-08-05

**Asked:** "when I have a session focused, option + delete doesn't seem to work to backspace by words when
I have nebula opened in iterm, fix"

**Did:** Fixed outside the codebase — set left Option → Esc+ in iTerm's Default profile.

**Gotchas:**
- iTerm2 3.5.10 in kitty mode only reports Option as the alt modifier when the profile's Option key is
  **Esc+** (`Option Key Sends` = 2). With "Normal" (the user's old setting) Option+Delete arrives as a
  plain Backspace and word-delete silently breaks.
- iTerm must **not** be running when editing its plist or it clobbers the write on quit. Its quit-confirm
  dialog can't be dismissed via osascript without accessibility permission — SIGTERM works and skips the
  pref flush.

### The Focus-Key Odyssey → Ctrl+Q — 2026-08-04

**Asked:** "make cmd arrow change focus of the panels, require an enter of the session panel to focus lock
into it" — which turned into a long elimination, punctuated by "I'm not even using ghostty you fuck" and
ended by "fuck it go back to control + q, also shift drag doesn't do shit. fix it".

**Did:** Ctrl+Q is the unlock/escape hatch. Fallbacks kept: Ctrl+] / Ctrl+Esc / Ctrl+←. Shift-drag was
replaced with app-side plain drag-selection in the terminal pane (REVERSED overlay for highlight, text via
vt100 `contents_between`, `pbcopy` on mouse-up).

**Gotchas:**
- **The user runs Terminal.app**, not Ghostty, despite Ghostty being installed. Terminal.app fails the
  kitty-keyboard probe, so Cmd-modified keys and Ctrl+Esc never reach the app there.
- Everything else was eliminated for a reason: Cmd+arrows (no kitty protocol), Ctrl+arrows (Mission
  Control), Ctrl+Esc / Option+Esc (undeliverable), Ctrl+]: vetoed on feel, double-Esc: implemented then
  reverted because Claude Code owns Esc, Shift+arrows and Ctrl+G/T: Claude Code binds them. **Ctrl+Q is
  settled — don't relitigate it**; the user's Cmd+Q-adjacency worry lost to familiarity.
- crossterm collapses a same-read `\x1b\x1b` pair into **one** Esc event (escaped-escape rule), which is
  what made double-Esc unworkable.
- "Shift+drag selects text" is a lie in Terminal.app — there's no mouse-reporting bypass there, unlike
  Ghostty/iTerm.
- The user runs `nebula` via a `~/.cargo/bin` symlink to `target/release` — **rebuild release and restart
  the TUI** before testing keybinding changes, or you are testing a stale process.

### Bootstrap: Daemon/TUI Split — 2026-08-04

**Asked:** "I want to build out a cli tool which is performant, uses very little memory, but kind of acts
like a multi plexer to allow creating new terminal windows (similar to ghostty). the main things I need to
include, like the peak user experience I'm going for is. left side panel for project, then if you c…"

**Did:** `47037e8`. Cargo workspace `crates/{nebula-core,nebula-daemon,nebula-tui,nebula}` shipping one
binary. A detached tmux-style daemon owns the PTYs (portable-pty, 1MB byte-ring scrollback with seq
numbers); the TUI attaches over a unix socket with length-prefixed MessagePack (`nebula-core/src/codec.rs`).

**Gotchas (locked decisions — user-approved, don't relitigate):**
- **No server-side VT grid.** Attach replays the ring into the client's vt100 parser plus a SIGWINCH
  resize-jiggle.
- **tui-term is a renderer only**, kept behind `nebula-tui/src/ui.rs` as a swap point.
- **Status comes from agent hooks, not MCP** — MCP was proven unreliable in ../mission-control. Managed
  hooks are merged into the worktree's settings and curl a loopback axum server with a per-boot bearer
  token. Keep the logic in the pure `AgentStatusMachine` (`nebula-daemon/src/status.rs`, unit-tested with
  injected clocks) and **never trust a bare `Stop`**.
- Kitty keyboard protocol passthrough (`nebula-daemon/src/pty/kitty.rs`) is what makes Cmd/Option combos
  and Shift+Enter reach Claude Code at all.
- **Unix socket paths must stay short** — SUN_LEN is ~104 bytes, so a long `NEBULA_RUNTIME_DIR` breaks
  `bind()`. This bites the test harnesses and the screenshot harness constantly.
- Ideas were borrowed from ../mission-control, but **all code is written fresh** — that was a hard user
  requirement.
