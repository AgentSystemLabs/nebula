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

### `r` Renames A Project Row Without Touching Its Folder — 2026-08-28

**Asked:** "Allow me to rename a project from the projects section, by using hte shortcut "r" if a project
is selected. Currently the project name is the folder name. We don't want to changhe the folder name, is
just a visual change, but then if the project is renamed, display under it on a smaller font size the
folder name still." — then, after the first cut shipped a plain `th.dim` foreground: "I told you to use a
smaller fontsize though for the project folder name that is being displayed under, and displayed it kinda
with less opacity, so the priority in the ui the project name we chose" — then "Yes but in Terms of
hierarchy we should display like the folder name in a smaller font size. Is there any way that we can do
that?" — then, on testing it: "After renaming the project, if I rename again to an empty string, it should
be like undoing the rename and come back to display as the default way"

**Did:** No schema change — `projects.name` was already a plain display column and `repo_path` already
carried the folder, so a rename is one `UPDATE`. `nebula-core/src/entities.rs` grew
`Project::folder_name(&Path)` (the last path component, `"project"` fallback — lifted verbatim out of
`add_project`, which now calls it) and `Project::folder_subtitle() -> Option<String>`, which returns the
folder name only while it differs from `name`. That derived check *is* the "has it been renamed?" flag;
no new field, no migration. Daemon: `Store::rename_project`, `Daemon::rename_project`
(`registry.rs:~739`, trims, and an **empty name resets the row to the folder name** — the only way back
from a rename), `ClientRequest::RenameProject` + **protocol 28 → 29**, `server.rs` dispatch. TUI:
`PromptKind::RenameProject` / `MenuAction::RenameProject`, the previously-empty
`Action::Rename => Focus::Projects => {}` arm in `event_loop.rs:1455`, a "Rename" row in both project
context menus (keyboard `m` and right-click), footer hint, help-overlay PROJECTS row, README keymap row.
`ui.rs::draw_projects` grows a renamed row to `PROJECT_BTN_H + 1` and renders the folder name on the row
under the name as `└ <folder>` — the `└` flush with the name's first column (not the status dot), the
whole line `th.dim` **plus `Modifier::DIM`**. Three signals, because a fourth is not available: see the
font-size gotcha below. Tests:
`rename_project_relabels_the_row_and_leaves_the_folder_alone` (registry),
`r_renames_the_selected_project_row`, `renaming_a_project_to_nothing_undoes_the_rename` and
`a_renamed_project_shows_its_folder_name_underneath` (event_loop), plus the real-PTY
`tui_project_rename_shows_the_folder_and_empty_undoes_it` (e2e_tui). Workspace: 698 green, fmt/clippy
clean.

**Gotchas:**
- **`submit_prompt` cancels empty input for every `PromptKind` not on an explicit allowlist**
  (`event_loop.rs:~3739`): it flashes `cancelled: empty input` and returns *before* the `match`, so the
  request never leaves the client. That made the daemon's "empty name resets to the folder name" branch
  **unreachable from the UI** while its daemon-side test passed the whole time — the user found it by
  using the feature. `RenameProject` had to join `NewAgent | NewWorktree` on that allowlist. Any future
  prompt whose empty value *means* something must opt in the same way, and only an end-to-end press
  proves it: a registry test and a "does the prompt open" test both stay green.
- **"Render this text smaller" — the settled answer, so nobody re-derives it.** The user asked three
  times; all three avenues were checked, not guessed:
  1. **A terminal cell has exactly one font size.** No SGR attribute scales text down. DECDHL/DECDWL only
     go *bigger* and re-flow the line.
  2. **Kitty's text sizing protocol (OSC 66) genuinely does it** — `OSC 66 ; n=1:d=2 ; text ST` renders at
     half size *inside the same cells* (`n`/`d` fractional scale, `v` for top/bottom/middle alignment), so
     the grid survives. **But only Kitty and Foot implement it.** WezTerm: "does not support the Text
     Sizing protocol". Ghostty 1.3.x *parses* OSC 66 and renders nothing (1.3.0 notes: "not implemented in
     the GUI yet", tracking issue ghostty-org/ghostty#10333, open). tmux strips it even under Kitty. And on
     a terminal that doesn't parse OSC 66 the whole run **including the text** is eaten as an OSC string,
     so the content vanishes — never emit it without a CPR-based support probe.
     `https://sw.kovidgoyal.net/kitty/text-sizing-protocol/`
  3. **The Unicode fallbacks are font-dead.** Checked the real `cmap`s with fontTools:
     small caps — `HackNerdFontMono-Regular` and `SFNSMono` missing **25/26**, Menlo 17/26; superscripts —
     Hack **26/26** missing, SF Mono 24/26, only Menlo covers them (missing `q`); subscripts worse. Digits
     have no small-cap form at all. They render as tofu or fall back to a proportional face and break
     column alignment.
  So the levers that actually work everywhere are **weight, opacity and position**: BOLD full-strength
  name, `th.dim` (the dimmest color the theme has — nothing below it) + `Modifier::DIM`, and a `└ `.
  **The user's terminal is WezTerm** (`TERM_PROGRAM=WezTerm`), with Ghostty 1.3.1 also installed — the
  older entry saying they're on Ghostty is stale.
- **`Modifier::DIM` really is opacity**: `ratatui-crossterm-0.1.2/src/lib.rs:441` maps it to crossterm
  `Attribute::Dim` = **SGR 2 (faint)**, which Ghostty renders by blending fg toward bg. Confirmed on the
  wire, not just in the buffer — drawing the `TestBackend` buffer's cells through a
  `ratatui::prelude::CrosstermBackend::new(&mut Vec<u8>)` and grepping the bytes shows `\e[1m` before the
  label and `\e[2m\e[38;5;8;49m` before the folder. **`TestBackend` alone cannot prove this**; a `Style`
  in the buffer says nothing about whether the backend emits it. Worth the 20 lines when a change's whole
  point is an attribute.
- **`App::new()` starts with `sel_project = 0`, so the first project row is drawn *selected***, and
  `render_button` lifts a `th.dim` fg to `th.muted` there. A style assertion on row 0 that expects
  `th.dim` fails with `left: Some(Gray) right: Some(DarkGray)` and reads like a theme bug. Seed two rows
  and assert the second for the unselected style. The faint attribute survives the lift; the color does
  not.
- **`render_button` took `spans: Vec<Span>` and adding a second `sub: Vec<Span>` fails borrowck**, not
  just clippy: `spans.iter_mut().chain(sub.iter_mut())` errors with "lifetime may not live long enough"
  because `&mut [Span<'a>]` is invariant and the two params infer independent lifetimes. Adding `<'a>` to
  both fixes it but then trips `clippy::too_many_arguments` (8/7). The shape that satisfies both is one
  `text: Vec<Vec<Span<'a>>>` param whose entries take consecutive rows from `text_row` — 7 args, no
  chain. `render_row` passes `vec![spans]`.
- **The Projects panel does not scroll** — `rows_rect` returns None past the bottom and `draw_projects`
  `break`s — and `PROJECT_BTN_H` has no callers outside that one function. So a variable row height is
  contained entirely in `draw_projects`; nothing in `event_loop.rs` does row math for this panel.
- **`seed_tree` names its project `demo` at `/tmp/demo`**, so `folder_subtitle()` is None and not one of
  the 462 existing nebula-tui tests grew a row. A new draw test that wants the subtitle must set
  `app.tree.projects[i].repo_path` to something *other* than the name — upserting a `project(id, name, n)`
  alone won't do it, since that helper derives `repo_path` as `/tmp/{name}`.
- **A `TestBackend` buffer line can't be byte-sliced** (`&line[..30]` panics inside `●`/`▌`); use
  `line.chars().take(n)` when dumping the buffer to eyeball a column.
- Protocol 29 means a **v28 daemon still running from before the build refuses the new client** until it
  is restarted — expected, and the client already offers the kill-and-restart.

### The Root Worktree Row's Lower Half Wasn't Clickable — 2026-08-27

**Asked:** "I can't seem to click on certain places of the root worktree row in some areas, other rows are
fully clickable zones, fix it"

**Did:** Every sidebar pill is a 3-row cell (pad, text, pad) stacked on a 2-row `PILL_H` stride, but its
hit rect was `rows_rect_at(inner, y, PILL_H)` — top pad + text only. Stacked rows hide that because the
next pill's top pad covers the gap; the root row sits over a quiet spacer row, so its bottom pad (the
lower half of the pill as drawn) fell through to `PanelBg`. Same dangling pad on the last pill of any
group and the last of the list, in both panels. New `pill_hit_height(top, next_top)` in
`crates/nebula-tui/src/ui.rs` (next to `rows_rect_at`) sizes the target as `min(3, next_top - top)`:
a shared pad row still goes to the lower pill (unchanged), an unshared one stays with its pill.
`draw_worktrees` and `draw_sessions` iterate the layout with `enumerate()` to peek the next top;
`draw_session_row` grew a `hit_h` arg. Tests: `ui.rs::worktree_pills_are_clickable_over_their_whole_height`
and `session_pills_are_clickable_over_their_whole_height` (both fail on the old `PILL_H` rect).

**Gotchas:**
- **The "blank row" between the root row / a group's last pill and what follows is not an empty row —
  it is that pill's un-overlapped bottom pad.** The layout only bumps `vrow` by 1 after such a pill, and
  `WorktreeEntry::height()` is already `PILL_H + 1`, so `next_top - top` is 3 there, 2 when stacked.
  My first sessions-panel test expected a real gap row before the UNPINNED header and was off by one.
- `draw_column` returns rows starting at `area.y + 3` (spacer, title, spacer), so in a
  `TestBackend` drawn from `Rect::new(0,0,..)` the first pill's rows are y=3..=5.

### The Memory Modal's Unknown Agents Are The Prewarm Pool — 2026-08-27

**Asked:** "I notice I have a BUNCH of unknown agents.. are these sub agents claude is spawning? is so
display grouped in tree format.. also try to figure out a better label for them. find root cause what
these are from"

**Did:** They are not subagents (the Agent tool runs in-process inside `claude`). Every `(unknown agent)`
row was a **prewarm-pool spare**: `Daemon::prewarm_agent` in `crates/nebula-daemon/src/registry.rs` boots
a real `claude` CLI per (worktree, kind) with a fresh `NEBULA_AGENT_ID` and *no store row*, so the TUI's
`app.tree.agents` lookup in the metrics modal missed. `SessionMetrics` (`nebula-core/src/protocol.rs`)
gained `prewarm: Option<PrewarmInfo { worktree, kind, model }>` (`#[serde(default)]`, no protocol bump);
`Daemon::session_pids` fills it from the `prewarmed` map; the modal in `nebula-tui/src/ui.rs`
(`Overlay::Metrics`) now lists spares under a `warm spares (N)` header as a `├`/`└` tree, named
`claude · opus`, placed in their worktree, inert on Enter, with their own `warm … pre-booted for new
agents` rollup line; `footer_usage` counts them as `N warm` instead of agents. Test:
`metrics_groups_prewarm_spares_under_their_own_header` in `event_loop.rs`.

**Gotchas:**
- **Root cause of the count, not just the label:** resting the sidebar cursor on a worktree for 250ms
  (`PREWARM_DEBOUNCE`, `schedule_prewarm` → `fire_pending_prewarm` in `event_loop.rs`) prewarms it, and each
  spare lives up to 15 min (`PREWARM_MAX_AGE`). Scrolling across 7 projects = 7 idle `claude`s at
  150–300 MB each (the CLI plus its MCP children — python + MissionControl's `recall-mcp.mjs`) ≈ 1.7 GB.
  Opt-out is the hand-added `"prewarm_agents": false` key in the data dir's `config.json`; the settings
  modal does not expose it. Not changed — the user has not asked for the policy to move.
- To identify a mystery daemon child, `ps -Eo command -p <pid>` shows its `NEBULA_AGENT_ID` /
  `NEBULA_DATA_DIR` env, and `grep prewarm <data_dir>/state/daemon.log` maps the id to its spawn
  (`prewarmed agent session agent=… worktree=<branch>` — the branch only, not the project, so `main`
  repeats across projects).
- `ps -axo … -p <pids>` on macOS ignores `-p` and dumps every process; drop `-a`.

### The Panel Walk Stops At The Pane Going Forward And Locks It — 2026-08-27

**Asked:** "when using control shift h or l it should auto focus on claude code session when focused.
also we shouldn't loop the nav, if the hit terminal panel then control shift l stops" — then, after the
first cut: "I think you misunderstood me, I liked the control shift h and control shift l, but now it
doesn't work, I just wanted so if a user presses control shift l and gets to the terminal it should auto
focus it and stop allowing the user to cycle next to the workspaces top nav"

**Did:** `Action::FocusNext` / `Action::FocusPrev` in `crates/nebula-tui/src/event_loop.rs:1275`.
Forward (Tab / `^⇧L`) from `Focus::Terminal` now stays put instead of wrapping to `first_focus()`, and
landing on the pane goes through new `enter_terminal_pane(app)` (`event_loop.rs:4695`): `focus =
Terminal` plus `term_locked = true` when `app.term` is live (an empty/exited pane is focused, never
locked). `Action::Activate`'s `Focus::Terminal` arm reuses it. Back (⇧Tab / `^⇧H`) **still wraps** from
the first panel — the bar when shown, Projects when hidden — into the pane, and that arrival locks too;
`^⇧H` is also the hatch back out, so `^⇧H` alone cycles Projects → pane → Sessions → … Docs: keymap
hints, help-overlay row, the locked-input comment, two README spots. Tests
`ctrl_shift_hl_walk_forward_stops_at_the_pane_and_back_wraps_into_it`,
`focus_walk_includes_the_workspaces_bar_only_when_shown`, and the e2e_tui walk section. 686 green.

**Gotchas:**
- **The first cut also removed the backward wrap, and the user read that as "^⇧H doesn't work".** The
  ask named one direction only. `^⇧H` from the top panel is the one-key jump into the agent, and
  taking it away broke a habit. Scope the no-wrap to the direction that was asked for.
- **`Ctrl+→` is now the only unlocked way into the pane.** `FocusTerminal`'s documented purpose is
  "cross without locking", so it was left alone — but the old comment claiming "Tab / Ctrl+arrows do not
  lock, Enter does" was load-bearing prose in the escape-hatch block and had to be corrected.
- **`^⇧L` pressed again on a locked pane is forwarded to the agent, not swallowed** — the locked path
  only intercepts `UnlockTerminal` chords and the hardwired `^q`. Kitty terminals send a harmless
  `CSI 108;6u`; legacy degrades it to `Ctrl+L` (0x0C), pre-existing and unavoidable.
- **Proving a key does what the user sees, not what the unit test sees:** a throwaway e2e_tui test that
  sent the raw kitty bytes (`\x1b[108;6u` / `\x1b[104;6u`) to the real binary settled "is it the code
  or the build?" in one run. `strings target/debug/nebula | grep <new hint text>` settles which build
  is running. Both were fine — the disagreement was about the spec.
- `^⇧H`/`^⇧L` only reach nebula on a terminal that encodes the chord — it did when this was written
  (Ghostty). **Stale as of 2026-08-28: `TERM_PROGRAM=WezTerm`**, Ghostty 1.3.1 still installed. Check
  `TERM_PROGRAM` rather than trusting either note; the older Terminal.app note is stale too.
- In `crates/nebula/tests/e2e_tui.rs`, "the walk stops here" is untestable with `wait_for_text` alone —
  it passes trivially if the footer is already up — so the stop is proved by pressing the extra key and
  then walking one step the other way. ⇧Tab is `\x1b[Z`, Ctrl+→ is `\x1b[1;5C`.
- `e2e_pty::external_worktrees_are_adopted_and_dropped` failed once mid-run and passed alone on rerun —
  the usual e2e flake, unrelated to a TUI keymap change.

### A Hidden Workspaces Bar Moves Its Name Onto The Projects Header — 2026-08-27

**Asked:** "when a user has the workspaces top bar hidden, display the selected workspace name in place
of where it says Projects inside the projects list"

**Did:** `draw_projects` (`crates/nebula-tui/src/ui.rs:2475`) now computes its title instead of passing
the literal: `"PROJECTS"` while `app.show_workspaces`, otherwise
`app.tree.active_workspace_name().to_uppercase()`, truncated to `area.width - (ROW_GUTTER + 1 + " · n")`
so a long workspace name can't run into the column rule. The ` · n` project count stays either way, and
`draw_column` is untouched — WORKTREES/SESSIONS still pass literals. New test
`a_hidden_bar_moves_the_workspace_name_onto_the_projects_header` (event_loop.rs) renders hidden, switches
to `ws2`, and shows the bar again. 685 workspace tests green, no new clippy warnings.

**Gotchas:**
- Upper-cased on purpose — every other column header is all-caps, and a natural-case name in that slot
  breaks the row. Verified by eye against a rendered `TestBackend` buffer, not just an assertion.
- `Tree::active_workspace_name` falls back to `"default"` when no `Workspace` entity has arrived, so a
  bar-hidden render reads `DEFAULT · 1` even with `tree.workspaces` empty. Several existing tests set
  `app.show_workspaces = false` purely to size the panels (`embedded_terminal_renders_pty_output`,
  `link_rows_render_under_a_links_header`, `archived_list_scrolls_by_wheel_and_follows_the_cursor`) and
  now draw that header — harmless, but a future `text.contains("PROJECTS")` in one of them would fail.
- One existing assertion had to move: `shift_w_toggles_the_workspaces_bar_and_parks_focus` checked
  `lines[1].starts_with("   PROJECTS")` on the hidden path; it's `"   DEFAULT"` now.

### A Paused Rebase Renamed The Worktree Row To `detached @ …` — 2026-08-27

**Asked:** "something in this conversation caused the worktree to show up in the UI but then it switvhed
yp detached at f816b5f.  I wouldn't have expected the wortree name to become detached"

**Did:** Diagnosed, then fixed in `95c0a18`. The cause was the `git rebase origin/main` in the entry above
pausing on conflicts: a rebase parks HEAD on the commits it replays, so `git worktree list --porcelain`
prints `detached` (no `branch` line) for the checkout for as long as it sits there. The 2s worktree sync
(`reconcile_project_worktrees`, `registry.rs`) saw `known.branch != entry.branch`, wrote `detached @
f816b5f` into the row and broadcast it, then wrote the branch back on the tick after `rebase --continue`
finished. `git::list_worktrees` (`crates/nebula-daemon/src/git.rs`) now resolves a branch-less entry
through the new `rebasing_branch(checkout)`: `git rev-parse --absolute-git-dir`, then
`<git-dir>/{rebase-merge,rebase-apply}/head-name` (`refs/heads/<branch>` — the same file `git status`
reads to say "rebasing branch X"). Only a checkout with no rebase in progress, or one rebasing from an
already-detached HEAD (`head-name` reads `detached HEAD`), still gets `detached_label`. Test
`a_paused_rebase_keeps_the_worktree_on_its_branch` walks mid-rebase → `--abort` → `checkout --detach`.
147 daemon tests green.

**Gotchas:**
- **The row heals itself, so the bug is easy to write off as cosmetic — it isn't.** `nebula worktree
  <name>` finds its target by `w.branch == branch` (`registry.rs:~1377`) and *creates* a worktree when
  nothing matches, so an agent running it mid-rebase would have tried to add a second checkout for a
  branch that already has one. Anything keyed on the branch string is blind for the whole pause.
- The rebase state lives in the **per-worktree** git dir (`<repo>/.git/worktrees/<name>/rebase-merge/`),
  not the shared `.git` — `git_common_dir` in `lib.rs` deliberately hops *to* the shared one for the
  mtime probe and is the wrong helper here; `rev-parse --absolute-git-dir` from the checkout is right.
- `git worktree list` may print canonical paths while `add_worktree` returns the tempdir spelling
  (`/var/…` vs `/private/var/…` on macOS): the existing `remove_worktree_*` tests only assert
  `e.path != wt`, which passes trivially either way. Canonicalize both sides when a test needs to *find*
  the entry.
- `git::current_branch` is a second, uncalled copy of this label logic with its own `detached@` format.
  Left alone, but don't reach for it thinking it agrees with `list_worktrees`.
- **A "load race" can be a build-layout race.** After this change `workspace_scope_is_per_connection`
  (e2e_pty) failed 7 of 11 *idle* runs while the pre-change `git.rs` passed 13 of 13 — yet a probe
  showed `rebasing_branch` was never called and the porcelain parse was identical. Adding file I/O to
  `list_worktrees` for the probe made it pass 3/3: pure timing. It was the documented Ack-beats-upsert
  race, and a code change that never runs in the test still shifts the odds. Fixed on the test side
  (`6638952`): wait for the upsert *and* the Ack, as `cli_add_project` does; the TUI never relied on the
  order (`event_loop.rs` "usually lands just before this Ack; if not, …"). A/B the old file in place
  (`git show <sha>:path > path`, run, `git checkout -- path`) before believing either verdict.

### Released v0.13.0 Off A Checkout Three Releases Stale — 2026-08-27

**Asked:** "commit push and make a release for me" (after the `nebula ssh` clipboard fix in the entry
below).

**Did:** Cut **v0.13.0** (0.12.0 → minor: new user-facing features). `release` skill as written: private
worktree on `release-v0.13.0` off `origin/main`, twelve files copied in by content,
`cargo test --workspace --no-fail-fast` in an isolated `CARGO_TARGET_DIR` — **655 passed, 0 failed**
across all seven binaries — three commits (feature / `.claude/MEMORY.md` / `Release v0.13.0`),
`git push origin release-v0.13.0:main`, tag, all four matrix targets green, notes replaced via
`gh release edit`. Carried the OSC 52 ssh clipboard route, the cloud-mirror work, and the tab underline
glyph. `random.txt` left untracked for the third release running.

**Gotchas:**
- **The shared checkout was 7 commits / 2 releases behind `origin/main`** (local `main` at
  `0361f0a`/0.10.0, origin at v0.12.0) — worse than the v0.12.0 entry's three. Same tell, same rule:
  `git diff` vs HEAD showed 20 files / 3,453 insertions, `git diff origin/main` showed 15 / 1,292, and
  only the second is the release. This has now happened three releases in a row because each release is
  cut from a worktree and never fast-forwards the shared tree — **assume it, don't check for it**.
- Untracked files read as *pure deletions* in `git diff origin/main -- <path>`: `pty/cloud.rs` showed
  286 deletions purely because it is untracked locally while origin has it committed. `cmp` against the
  worktree's copy before concluding anything — it was byte-identical and needed no copy at all.
- Vetting copied files means checking the **deletion** side of each hunk, not the addition side: the
  question is whether the stale tree reverts something origin committed. Here all 48 deletions in
  `registry.rs` were the cloud-mirror rewrite replacing the older attach logic, which is fine — but that
  is only visible by reading them.

### Copy Over `nebula ssh` Asks The Terminal, Not The Remote Box — 2026-08-27

**Asked:** "when I try to copy text after doing nebula ssh into an ubuntu machine I spun up, it keeps
saying copy failed (clipboard unavvailable). help me debug if this is something I need to support in
nebula using ssh -X or if I just need to install something on the device"

**Did:** Neither — added an OSC 52 route. `nebula ssh` execs `ssh -t HOST '… exec nebula'`
(`crates/nebula/src/ssh.rs:40`), so the *whole TUI* including the copy path runs on the remote;
`copy_to_clipboard` (`crates/nebula-tui/src/event_loop.rs`) then shells out to wl-copy/xclip/xsel on a
headless VM that has none of them and no `DISPLAY`. New `copy_and_flash(app, text, label)` fronts all
three copy call sites (file finder Ctrl+y, tree view Ctrl+y, `copy_selection`): local → platform tool as
before; `app.is_remote` (already computed from `SSH_CONNECTION`/`SSH_TTY` via
`nebula_core::host::is_remote_session`) → queue `App::pending_clipboard`, which `main_loop` writes as
`\x1b]52;c;<b64>\x07` through `terminal.backend_mut()` next to the existing OSC 22 pointer-shape write.
Dependency-free `base64_encode` (RFC 4648 vectors test) rather than pulling a crate for one call site.
Rejected `ssh -X`: it needs xclip remotely *plus* XQuartz locally, and lands the text in the X11
clipboard rather than the macOS pasteboard.

**Gotchas:**
- **The "clipboard unavailable" failure path no longer exists** — OSC 52 is always available as a
  fallback, so a copy can now silently no-op on a terminal that drops OSC 52 (Terminal.app does; Ghostty
  and iTerm2 don't). That is why the flash says `copied N chars (via terminal)` on the OSC 52 route: the
  route is the only thing we can honestly report. iTerm2 additionally needs Settings → General →
  Selection → "Applications in terminal may access clipboard".
- **BEL-terminate OSC 52, not ST.** The neighbouring OSC 22 write uses `\x1b\\`; OSC 52 is the sequence
  where BEL is the form every implementer accepts.
- The payload cannot be written with a bare `std::io::stdout()` from the copy helper — ratatui owns a
  `BufWriter<Stdout>` that would interleave. Queue it on `App` and let the main loop write it through the
  backend, exactly as `pointer_shape` does.
- **The fix has to be on the *remote* binary.** `nebula ssh` only installs nebula when the remote lacks
  it, so an existing remote keeps its old version until `nebula upgrade` runs *there* — testing this from
  a freshly built local binary proves nothing.
- Not covered: `nebula ssh` launched from *inside* a local nebula pane. The remote's OSC 52 hits the local
  nebula's vt100 parser and dies; passing it through the PTY layer is unbuilt.

### The Open Tab's Underline Was A Half-Cell Away From The Tab — 2026-08-27

**Asked:** "fix the small gap between the header workspace name and the bottom bar if possible" — with a
screenshot of the Workspaces bar: the open tab's dark `sel_bg` block, a strip of black, then the green
accent underline.

**Did:** One glyph. `crates/nebula-tui/src/ui.rs::draw_workspaces_bar` drew the open tab's underline as
`━` (U+2501) into the rule row; it now draws **`▀`** (U+2580, upper half block) with the same `th.accent`
fg. The tab's `sel_bg` fill still stops at `area.height - 1`, so the block's bottom edge and the half
block's top edge are the same pixel row — flush, no gap. Rejected: extending the fill through the rule row
and painting `set_bg(th.sel_bg)` under a kept `━` — it closes the black gap but leaves a half-cell of
`sel_bg` *below* the accent, so the tab hangs past its own underline. Tests updated in
`crates/nebula-tui/src/event_loop.rs`: the cell assertion in the tab-surface test (`"━"` → `"▀"`, plus the
why) and `rule.contains("━━")` → `"▀▀"` in `the_workspaces_bar_sits_directly_above_projects`. 452
nebula-tui tests green, fmt clean.

**Gotchas:**
- **A sub-cell gap is invisible to `TestBackend`.** The buffer holds a symbol and a style, not pixels, so
  no `buffer_text` or cell assertion can see that `━` renders at the cell's *midline* and leaves the top
  ~40% of the cell unpainted. The only guard available is asserting the symbol, which is why the test
  carries the reason in a comment.
- **Box-drawing line glyphs never touch a cell edge; block elements do.** Any "join two rows of fill"
  problem in this TUI is a block-element problem (`▀`/`▄`), not a heavier-line problem.
- **Judging this needs a real font raster, not a terminal.** Pillow + `/System/Library/Fonts/Menlo.ttc` at
  ~27px, drawing the candidate cell stacks side by side, settles it in one PNG — no demo daemon, no tmux.
  See the `tui-screenshot-harness` note for the full-app version when a change is bigger than a glyph.
- The accent underline now sits slightly *above* the `─` edge rule flanking it (mid-cell vs top-of-cell).
  That is deliberate — the tab indicator reads as heavier than the divider — not a misalignment to "fix".

### Cloud Rows Mirror Their Session Instead Of Dying At Create — 2026-08-27

**Asked:** "still when I try to create a claude cloud session, it doesn't seem to update me with the
changes, it just says use a command to resume, and if i leave and come back to that terminal it errors
out.  find a way to allow the cloud session output to show up in the ui" → chose, from the options put
to them: auto-follow until the pane is touched, plus a send-message path.

**Did:** Probed the real CLI first (2.1.247, see the gotchas), then built the follow on `--teleport`.
`crates/nebula-daemon/src/pty/mod.rs`: `PtySession.input_seen` (AtomicBool, set by `write_input` on a
non-empty write only — resizes don't count). `registry.rs`: `Daemon.cloud_attach_gated` (AtomicBool, set
when `arm_cloud_attach_fallback` sees the refusal) routes every later re-entry through the new pure
`cloud_reentry_launch(id, gated)` → `Teleport` instead of re-flashing the red error; `arm_cloud_follow`
watches a `CloudLaunch::Create` PTY for its id + exit and then calls `attach_cloud_agent` unasked, so a
create no longer leaves a dead "Resume with:" pane; `start_cloud_mirror`/`refresh_cloud_mirror`/
`stop_cloud_mirror` + `Daemon.cloud_mirrors` re-teleport the row every `CLOUD_MIRROR_REFRESH` (45s,
`NEBULA_CLOUD_MIRROR_SECS` overrides, `0` disables, floor 2s); `cloud_worktree_for` is the re-home split
out of `attach_cloud_agent`; `send_cloud_message` runs `claude -p <msg> --cloud=<id>` via
`tokio::process` + `login_shell_wrap` and refreshes after. `validate_cloud_text` now covers both the
launch task and the message. Protocol **v28**: `ClientRequest::SendCloudMessage` and runtime-only
`Agent.cloud_mirroring` (set in `agent_entity` like `alive`). TUI: `PromptKind::CloudMessage` (multiline),
`MenuAction::SendCloudMessage`, "Send to cloud session" menu item, `PendingIntent::ReopenPromptOnError`
so a failed send hands the text back, and a `cloud ↻` accent badge in `ui.rs`. 654 tests green.

**Gotchas:**
- **Live attach is still gated off** (verified 2026-08-27 under a real PTY): `claude --cloud=<id>` prints
  `Error: Attaching to an existing cloud session is not enabled for your account.` Without a TTY it now
  fails differently — `non-interactive --cloud <session_id> requires a prompt` — so a non-PTY probe reads
  like the gate is gone. Probe under `script -q`.
- **`claude --teleport=<id>` is a repeatable snapshot pull, and that is the whole mechanism.** Verified
  three runs against one live session: it re-fetches the transcript each time, picks up turns taken
  since (a `-p --cloud` message sent between runs showed up), is idempotent in the same worktree, and
  does **not** end the cloud session. It is a fork to local, not a live link — hence the re-teleport loop.
- `claude -p "msg" --cloud=<id>` still only prints `Sent to cloud session.`; `--output-format
  stream-json` is refused outright for `--cloud`. There is no reply to stream.
- The CLI binary carries `/v1/code/sessions/{id}/events/stream` (real SSE, what the CLI itself uses).
  Rejected: it needs the OAuth token scraped out of the macOS keychain and is undocumented. Reading the
  keychain was blocked by the auto-mode classifier, which is the right instinct.
- **A mirror must stop when its pane is gone, or cloud rows become unreapable.** The idle reaper kills
  unattended sessions; a mirror that respawns on every tick would fight it forever. `refresh_cloud_mirror`
  returns `Ok(false)` when the session is absent — that also stops a teleport that dies on every try.
- The mirror ends on the first keystroke, not on the row gaining a local `session_id` — a teleport sets
  one immediately via the hooks. `restart_agent` therefore routes on `session_id.is_none() ||
  cloud_mirror_active(id)`.
- A mirror that quits must re-broadcast the row, or the `cloud ↻` badge keeps promising refreshes.
- e2e can't tell a stub's attach from its teleport (`NEBULA_AGENT_CMD` spawns override the argv verbatim,
  no cloud flag), so the gate is unit-tested on `cloud_reentry_launch` instead. And the recorded
  "upsert beats the stub's first line" race bites again: a `!cloud_mirroring` predicate matches the
  create's own upsert, so wait for the run count first, then require lit-then-quiet.

### Released v0.12.0 From A Checkout That Was Behind Its Own Work — 2026-08-27

**Asked:** "commit push release"

**Did:** Cut **v0.12.0** (0.11.0 → minor: new user-facing feature) carrying the done-vs-read violet work
in the entry below. `release` skill as written: private worktree on `release-v0.12.0` off `origin/main`,
five files copied in by content (`README.md`, `app.rs`, `event_loop.rs`, `theme.rs`, `ui.rs`),
`cargo test --workspace --no-fail-fast` in an isolated `CARGO_TARGET_DIR` — **647 passed, 0 failed**,
exit 0 — three commits (feature / `.claude/MEMORY.md` / `Release v0.12.0`), `git push
origin release-v0.12.0:main`, tag, all four matrix targets green, notes replaced via `gh release edit`.
`random.txt` left untracked again.

**Gotchas:**
- **The shared checkout's `main` was three commits behind `origin/main`, and its working tree already
  held everything v0.11.0 shipped.** So `git diff` (vs local HEAD) showed 19 files / 2,255 insertions
  and `git diff origin/main` showed 5 — the second number is the release. Always scope the "what is
  unreleased" question to `origin/main`, never to HEAD, in this tree: the previous release was cut from
  a worktree and never fast-forwarded the shared checkout.
- `cargo test --workspace` **fail-fast makes a flake look like a truncated suite**: run 2 died at
  `e2e_pty` (1 of 23) and never started `e2e_tui` or the lib tests. Runs 1 and 3 were fully green. Use
  `--no-fail-fast` for the gate so one flaky e2e can't hide the other six binaries' results.

### Done Reads Violet And Says "done" — 2026-08-27

**Asked:** Four turns, one thread. "don't put the number of running sessions in workspace hreader tabs,
just show \"2 done\"" → "replace word \"new\" with \"done\"" → "can you make the status dot for done a
different color than green so it's obvious something needs to be addressed" → the correction that settled
it: **"no you misunderstood, it should be green after I focus on the session, but purple when done and not
yet read"**.

**Did:** The unread finish is now a state with its own color, not a synonym for finished.
`workspace_running` → **`workspace_unseen`** (`crates/nebula-tui/src/app.rs:1477`) counts `a.unseen`,
mirroring `worktree_unseen` / `project_unseen`, so all three tiers count the same thing and the count dies
as you read. `ui.rs::status_dot` took an **`unseen: bool`** third arg: `Finished` draws `th.done` when
unread and `th.ok` once seen; every other status ignores it. All four call sites pass it (workspace tab,
project row, worktree row, session row — `a.unseen && !a.archived`), and `PaletteItem` gained an `unseen`
field so `/` splits the same way. Wording: `unseen_badge` → ` n done` (was ` n new`), session row's
harness-slot takeover → ` done`. New theme role **`done`** = `Color::Indexed(141)` violet, `Indexed(45)`
turquoise in the `rose` preset (whose `special` is already 141). `th.ok` keeps green for diff-adds,
`⏻ connected`, reviewed-file ticks — and now for read finishes. The PR link row's ` n new` was left alone:
it counts unread review comments, not finished turns. README dot table, feature bullet, badge paragraph and
`Shift+W` row updated. Tests: theme asserts `done` differs from `ok`/`warn`/`err`/`special`/`dim` in all 5
presets; `unwatched_finishes_badge_the_rows_until_read` now asserts violet before the read and green after,
on the session row *and* the project row above it. 450 nebula-tui + 143 daemon + 7 core green, fmt clean.

**Gotchas:**
- **The panel columns are a fixed 20 cells wide, so a wider badge clips instead of reflowing** — widening
  the `TestBackend` from 100 to 120 changed nothing but the TERMINAL pane. ` 1 new` → ` 1 done` is one
  cell more and the worktree root row started rendering `root 1 don`.
- **The project and worktree name budgets never billed the pill marker.** `render_pill` / `render_button`
  both `spans.insert(0, marker)` — one cell, rail or space — but `ui.rs` subtracted only the dot's 2
  (`saturating_sub(2 + badge_len)`). Pre-existing off-by-one that only bit once the badge grew; now 3.
  The PR row two arms down had it right all along (`saturating_sub(3)` for a 2-char `↗ `), which is what
  confirmed the convention.
- With that fixed, `main` ellipsized to `ma…` to keep ` ⌂ root`. The root badge now **yields** to a branch
  it would otherwise truncate (`● main 1 done`, not `● ma… ⌂ root 1 done`) — the ⌂ is decoration, the
  branch is the row's identity. e2e's `wait_for_text("main ⌂ root")` still passes: no badge, no contention.
- `"client 1 done".contains("client 1 ")` is **true** — a negative assertion written to prove the bare
  running count was gone would have passed for the wrong reason. Dropped it.
- **"Done" is ambiguous and the first two readings were both wrong.** It went `Running` count → all
  `Finished` → unread-`Finished`, and the dot went all-`Finished`-violet → violet-only-while-unread. The
  distinction the user wanted was never finished-vs-not, it was **read-vs-unread** — the same axis
  `Agent::unseen` already tracked for the counters. When a color is asked for "so it's obvious something
  needs to be addressed", find the flag that already means "needs addressing" instead of coloring a status.
- Test counts drifted 450 ↔ 451 between back-to-back runs and two workspaces-bar tests failed once and
  never again: another session was editing this shared checkout mid-build. Rerun before debugging (see
  [Shared tree races] in the user memory).

### A Workspace Switch Cold-Booted A Fleet Of Claude CLIs — 2026-08-26

**Asked:** "in a worktree, debug a performance issue when a user switches between workspaces sometimes it
seems like it's lagging or stuck loading up multiple claude sessions as the terminal panel doesn't show for
like 5-10 seconds"

**Did:** Three compounding causes, all confirmed against the live `~/.nebula-dev` daemon log and DB, fixed
in `327757f` (originally `7246a47`; the branch sat unmerged for a day and was rebased onto `origin/main`
at v0.13.0 on 2026-08-27 — see the rebase notes at the end of this entry).

1. **Every switch attaches a *dead* session.** `session_idle_timeout` defaults to `5m`
   (`nebula-daemon/src/config.rs:44`), and `reap_idle_sessions` kills everything in a workspace nobody is
   attached to — the dev `daemon.log` is wall-to-wall `reaping idle session … idle_secs=300`. So
   `switch_workspace_inner` → `restore_context` → `restore_session` → `attach` lands on a dead sref and
   the daemon's `Attach` arm cold-spawns `zsh -l -i -c 'exec claude --resume <sid>'`. The replay it sends
   back is **empty** (fresh ring), so the pane rendered a blank vt100 grid with no indication of anything
   happening.
2. **250ms later the prewarm booted every *other* session in the worktree, inline.**
   `PrewarmWorktreeSessions` was handled synchronously on the connection's request loop
   (`server.rs:426`, the old comment said "Deliberately inline"), and `prewarm_worktree_sessions` called
   `ensure_session` for every dead non-archived row. `main` in the dev DB has **5 agents** → 5 concurrent
   login-shell + claude boots, plus a 6th from `PrewarmAgent`, all starving the one the user was waiting
   on — and stalling that client's `Input` frames for the whole burst.
3. **`attach` had no debounce**, unlike prewarm. In the Workspaces column `move_selection` runs a full
   `switch_workspace` per row (`event_loop.rs`), so walking past four workspaces cold-spawned four CLIs
   and abandoned three, each then living 5 more minutes.

Fixes: `Daemon::spawn_gate` (`registry.rs`) makes `ensure_session`'s check-and-install atomic, which is
what lets the sweep leave the request loop — `run_worktree_prewarm` is now its own task, boots one session
per `PREWARM_STAGGER` (1.5s), skips `is_alive` rows, and `prewarm_sweep` aborts a superseded sweep.
TUI-side, `attach` defers the request by `ATTACH_DEBOUNCE` (180ms) via `pending_attach`, with
`attached_sref` tracking what the daemon actually holds while `term.sref` runs ahead; `attach_now` /
`preview_selected_now` skip the wait for explicit picks. `AttachedTerm::painted` drives a `starting…` tag
plus a centered notice in `draw_terminal`. 631 tests green when written; **657 green after the rebase**
(exit 0, `--no-fail-fast`, all 7 binaries).

**Gotchas:**
- **Measure the boot before blaming the code.** One fresh `claude` under `zsh -l -i` on this machine is
  **0.67s to first byte, 1.47s to a painted screen** — and that's without `--resume` reloading a
  transcript. Six of those at once is the whole 5-10s. A pty-fork bench spawning 4 at once got the shell
  OOM-killed (exit 137) and 3 at once never finished in 120s; benchmark agent CLIs **one at a time**.
- **`app.term` and the daemon's attachment are now two different things.** Tests that set
  `app.term = Some(AttachedTerm::new(…))` directly leave `attached_sref` unset, so a `detach_if_attached`
  keyed only on `attached_sref` silently stopped emitting `Detach` and broke 4 tests. Both
  `detach_if_attached` and `release_attachment` deliberately fall back to `term.sref` — a `Detach` the
  daemon holds no attachment for is a no-op there (`server.rs` `attached.remove()` returns `None`).
- **Debouncing `attach` breaks every test that asserts an immediate `Attach` after a selection move.** 10
  failed. Only 2 were genuinely the new contract (`session_arrows_preview_without_focusing`,
  `switching_contexts_restores_the_remembered_session` — both now call `fire_pending_attach` to settle).
  The other 8 were paths that *should* stay immediate: wrap `reconcile_selection` and `jump_to_target`
  (both have early `return`s, so wrapping beats appending) and route clicks through
  `preview_selected_now`.
- **Input is dropped for a session the daemon hasn't spawned**, so a pending attach must land before the
  first keystroke. `handle_terminal_event` fires it up front when `term_locked`; the two paths that take
  the lock without attaching (`Action::Activate` on an already-focused pane, `Action::Zoom`) fire it too.
- The 100-col draw-test truncation trap from [A Workspaces Column Left Of Projects] bites again: the
  `starting…` test asserts pane body text, so it needs `show_workspaces = false` **and**
  `TestBackend::new(140, 30)` or the string is clipped mid-assert.
- The `switch_workspace_quietly` double-attach gotcha below is unaffected — the debounce happens to mask
  that churn now, but the quiet variant is still what makes a cross-workspace jump correct.

**Rebase onto v0.13.0 (2026-08-27), 20 commits later:**
- Only two files conflicted (`registry.rs`, `event_loop.rs`); `server.rs`, `app.rs` and `ui.rs` merged
  clean. Both `registry.rs` conflicts were additive-on-both-sides (main's `pending_moves` /
  `cloud_attach_gated` / `cloud_mirrors` vs. this branch's `spawn_gate` / `prewarm_sweep`) — keep both.
- **The one semantic conflict is `attach`.** Main added `mark_agent_seen` to the top of it (see
  [Unwatched Finishes Count On The Project And Worktree Rows]) on the reasoning that every path landing
  the pane on a session goes through `attach`. After this branch's split that funnel is `attach_inner`,
  so the call moves there — keyed to the **pane swap, not the Attach**, because the user is reading the
  screen during the debounce just the same. Putting it in `attach` alone would have skipped
  `attach_now` / `preview_selected_now` and leaked unread counts on every explicit pick.
- Two of main's newer tests failed, and they are not the same kind of failure:
  `switching_back_to_a_workspace_restores_project_worktree_and_session` is *exactly* the path the
  debounce exists for, so the test was updated to the new contract (pane restores now, Attach after
  `fire_pending_attach`). `snapshot_reattaches_the_remembered_session` is not — a boot restores one
  remembered session once, with no cursor sweep to wait out, so the Snapshot arm moved to
  `preview_selected_now` and main's assertion stands unchanged. **A failing attach-timing test is a
  question about which path it is, not a licence to relax the assertion.**
- Struct drift in the branch's new test only: `Project` lost the four `divider_*` fields (migration 18)
  and `Agent` gained `unseen` / `cloud_session_id` / `cloud_mirroring`. `cargo build` was clean and only
  `cargo test --no-run` surfaced it — test-only literals need the test compile to be checked.
- The workspaces *column* became a top tab bar on main, but `move_selection` there still does a full
  `switch_workspace` per step, so the premise of cause 3 survived the rework and
  `walking_the_workspaces_column_attaches_only_where_it_stops` passes untouched.
- `cargo fmt --check` and clippy are **dirty on `origin/main` itself** (a `base64_encode` line, 7 clippy
  warnings incl. `needless_return` at `event_loop.rs:5197`). None are in this diff — confirm with
  `git diff origin/main --stat -- <file>` before assuming a warning is yours.

### Released v0.11.0 Out Of A Shared Tree That Moved Mid-Release — 2026-08-26

**Asked:** "commit push release"

**Did:** Cut **v0.11.0** (0.10.0 → minor: new features) from the ~1,900 lines of uncommitted work sitting
in the shared checkout — cloud re-attach, unseen counters, workspace-delete confirm, workspace context
restore, notes removal, the protocol-skew message. Followed the `release` skill: private worktree on
`release-v0.11.0` off `origin/main`, files copied in by content, `cargo test --workspace` in an isolated
`CARGO_TARGET_DIR` (**647 passed, 0 failed**, all 7 binaries incl. e2e_pty 23 / e2e_tui 5), three commits
(feature / `.claude/MEMORY.md` / `Release v0.11.0`), `git push origin release-v0.11.0:main`, tag, then
`gh release edit --notes-file`. All four matrix targets green; 4 assets attached. `random.txt` (untracked
scratch, "nothing here is load-bearing") deliberately left out.

**Gotchas:**
- **`for f in $(git diff --name-only)` silently copies nothing in zsh.** Unquoted expansions are not
  word-split, so the loop ran once with all 19 paths as a single filename and `cp` failed with one
  `No such file or directory`. The tell was `git status` in the new worktree showing only the untracked
  file. Use `... | while IFS= read -r f`.
- **`cargo test … | tail -60` reports `tail`'s exit code, not cargo's.** The first run "passed" with exit 0
  while the tail showed no e2e results at all. Redirect to a file and check `$?` directly, or set
  `pipefail` — never trust a piped cargo exit status for a green gate.
- **An untracked file reads as `deleted` in `git diff <commit> -- <path>`.** Diffing the shared tree
  against the release commit showed `cloud.rs | 286 ------` because untracked files aren't in the index.
  It was byte-identical (`cmp`); nothing was lost. Verify with `cmp` before believing a deletion.
- **The shared tree moved between the snapshot and the push.** `git diff | shasum` went `8d64c39` →
  `3b9c7e0`: another session changed the Workspaces-bar badge from "count running" to "count done"
  (`workspace_running` → `workspace_done` in `app.rs`/`ui.rs`, plus a README cell). Not in v0.11.0, by
  design. Checksum the diff before and after the copy — it is the cheapest proof of what you actually shipped.
- The user's local `main` stays at `0361f0a` while `origin/main` is `8102fa4`; the working tree still holds
  every released change as uncommitted edits, so a plain `git pull` will refuse. Branch `release-v0.11.0`
  is kept locally as the handle to those commits.

### `nebula rename` Broke On A Protocol Skew The Error Message Misdiagnosed — 2026-08-26

**Asked:** "why is it printing … Error: daemon speaks protocol v26, this client v24 — run `nebula kill`
and relaunch I've ran kill but the hook still seems to fail" — then "why doesn't make dev do this
already though", and after the diagnosis "what do you recomend" → fix the message, not the plumbing.

**Did:** Diagnosis: `make dev` runs `target/debug/nebula`, which spawns its daemon from `current_exe()`
(`nebula-tui/src/ipc.rs:52`), so the daemon is always this checkout's build. But the auto-title hook
injects a **bare** `nebula rename` (`AUTO_TITLE_INSTRUCTION`, `nebula-daemon/src/hooks/mod.rs:29`), and
the agent's shell resolves that on PATH to `~/.cargo/bin/nebula` — 0.9.0/v24 there against the daemon's
v26. Fix: `handshake()` in `nebula-tui/src/ipc.rs` now calls a new `version_skew_message()`, which
compares the two versions, prints both binaries' paths, and recommends `make install` when the *client*
is older and `nebula kill` only when the *daemon* is; new `daemon_exe_path()` resolves the daemon's
binary from `paths::pidfile_path()` via `/proc/<pid>/exe` or `ps -p <pid> -o comm=`. Two tests in a new
`mod tests`. Then `make install` (0.9.0 → 0.10.0). Rejected: a daemon-side PATH shim (agents run through
`$SHELL -l -i -c`, `registry.rs:2015`, and this user's `.zshrc` prepends to PATH on ~11 lines, so a
prepended dir lands behind them and breaks silently later), and an absolute path in the instruction
(`CLAUDE_ALLOW_RULES` is `Bash(nebula rename:*)` — an absolute path stops matching and every auto-title
turns into a permission prompt).

**Gotchas:**
- `nebula kill` is the wrong advice when the **client** is the older side, and it was the message's only
  advice. Killing the daemon just makes the live TUI respawn an identical one from `current_exe()`, so
  the skew survives every restart — a guaranteed dead end for whoever follows it.
- Do **not** add a field to `ServerEvent::Incompatible` to carry the daemon's path. That frame is what a
  *newer* daemon sends an *older* client and rmp is positional, so the clients that need the information
  most would fail to decode it and report "unexpected handshake reply" instead.
- `paths::buildstamp_path()` is a **content hash** (`lifecycle.rs::fingerprint_file`), not a path — it
  can prove the daemon is stale but cannot name its binary. Pidfile + `ps` is the way.
- Only the *model-facing* instruction goes through the CLI. The real hooks are `curl` to
  `$NEBULA_API_URL/api/hooks/...` (`hooks/installer.rs:100`) — loopback HTTP, unversioned, fail-soft. So
  a protocol bump can only ever break the auto-title and `nebula worktree` instructions, never status
  reporting. Note the `silence` branch there: `UserPromptSubmit` keeps stdout precisely so the injected
  instruction reaches the model; every other event is fully silenced.
- The dev instance's isolation is *inherited*, not propagated: `NEBULA_RUNTIME_DIR`/`NEBULA_DATA_DIR`
  come from the Makefile's `DEV_ENV` and flow down make → TUI → daemon → agent PTY. The daemon itself
  only ever adds `NEBULA_AGENT_ID`/`NEBULA_API_URL`/`NEBULA_API_TOKEN` (`registry.rs:2023`). Nothing
  propagates *which binary*, which is the entire hole.
- The shared tree moved 26 → 27 under me mid-task (the notes-removal entry below), flipping the skew:
  after `make install` the PATH binary is v27 and the still-running dev daemon is v26, so the hook stays
  broken — now correctly reported as "The daemon is the older build". Restarting that daemon stops live
  sessions, so it was left to the user.

### Removed The Notes Feature Outright — 2026-08-26

**Asked:** "remove the ability to add notes" — asked twice, identically. The literal words name only the
*add* path, so I put the scope to the user before cutting: remove adding only (leaving a list that can
shrink but never grow) vs. remove the feature entirely. They chose **entirely**.

**Did:** Full-stack removal. `nebula-core`: `Note`/`NoteOwner` + `Entity::Note`/`EntityId::Note`
(`entities.rs`), `id_newtype!(NoteId)` (`ids.rs`), the four `ClientRequest::{Create,Update,Delete}Note` /
`SetNoteDone` variants and `Snapshot.notes` (`protocol.rs`), `PROTOCOL_VERSION` 26 → 27. `nebula-daemon`:
the `// ---- notes ----` blocks in `store.rs` (7 fns) and `registry.rs` (4 fns), the 4 `server.rs` arms,
plus **migration 21 `DROP TABLE IF EXISTS notes`**. `nebula-tui`: `Action::Notes` and its `e` binding
(`keymap.rs`), `NoteView`/`NoteInput`/`Overlay::Notes`/`PendingIntent::SelectCreatedNote`/`Tree.notes`
(`app.rs`), the modal draw + `note_badge` + both footer hints (`ui.rs`), and in `event_loop.rs` the
`NoteCmd` key handler, the mouse handler, `open_note_view`/`open_notes_for_owner`/`select_note_by_id`,
both context-menu rows, and the two delete-cascade `retain`s. Docs: README key table x2 + the SQLite
bullet, ARCHITECTURE.md's note-list paragraph. Tests: deleted `store::note_crud_roundtrip_and_cascade`,
the 3 `event_loop` note tests, and e2e `tui_note_modal_crud_and_badge`. 645 tests green, clippy clean
(7 pre-existing warning sites, none mine), `cargo fmt` applied.

**Gotchas:**
- **`row_badges` in `ui.rs` lost an argument.** It was `(unseen, notes, th)` feeding two badge makers;
  it is now `(unseen, th)`. `ProjectRowData`/`WorktreeRowData` each lost their `(usize, usize)` note-stats
  tuple slot, so the destructuring at both call sites had to shrink with them.
- **Don't cut a match arm by slicing to the *next* arm's name without checking arm order.**
  `Overlay::Metrics` sits **before** `Overlay::Notes` in `ui.rs`'s draw match, so slicing
  `[index(Notes) .. index(Metrics)]` had a negative span and silently **duplicated ~750 lines** instead of
  deleting any. The tell is the file getting *longer*: `wc -l` went 4467 → 5199 and
  `grep -c "Overlay::Metrics(view) => {"` returned 2. Find the arm's own closing brace instead.
- **Three keymap/settings tests used `Action::Notes` as an arbitrary subject**, not because they were
  about notes — they bind `g` to it to collide with `Git diff`. Swapped to `Action::OpenRepo`
  (`keymap.rs`) and `Action::Help` (`event_loop.rs`); Help was the right stand-in for
  `confirming_a_duplicate_moves_the_chord_off_its_old_action` because its final assertion needs an action
  that **opens an overlay**, which `OpenRepo` (spawns a browser) does not.
- **A stale `"keybindings": {"notes": "…"}` in a user's `config.json` is harmless** — `Keymap::from_overrides`
  already ignores unknown action ids, covered by `a_broken_override_falls_back_instead_of_stranding_the_user`.
- `e` is now **unbound**. `splash_footer_lists_only_keys_that_work` asserted `"e: notes"` was *absent*
  from the splash; that string can never appear now, so it came out of the dead-key list.
- The tree carried ~1700 lines of another session's uncommitted work (including a new
  `crates/nebula-daemon/src/pty/cloud.rs`). Baseline `cargo check` was green **before** starting, which is
  what made it safe to attribute every later error to my own edits — do that check first in a shared tree.

### Switching Workspaces Kept The Project Cursor At Row 0 — 2026-08-26

**Asked:** "when i switch between workspaces it should remember the last project, worktree, session
slection"

**Did:** Two of the three were already implemented and simply unreachable. `remember_context` /
`restore_context` (`crates/nebula-tui/src/event_loop.rs`) have kept `App::last_worktree_for_project` and
`App::last_session_for_worktree` since the panel work — but `switch_workspace_inner` hard-set
`app.sel_project = 0`, and both maps are keyed off the project the cursor lands on, so coming back to a
workspace restored *the first project's* worktree and session. Added
`App::last_project_for_workspace: HashMap<WorkspaceId, ProjectId>` (`crates/nebula-tui/src/app.rs:1920`),
recorded at the top of `remember_context`, and new `restore_workspace_project(app)` called from
`switch_workspace_inner` immediately before `restore_context` — only on the `restore: true` path.
Test `switching_back_to_a_workspace_restores_project_worktree_and_session`. 650 workspace tests green,
clippy clean.

**Gotchas:**
- **Order is load-bearing.** `restore_context` reads `selected_project()` to find the remembered
  worktree, and `restore_session` reads `selected_worktree()`. The project has to land *first* or the
  other two restore against row 0's context — which is the original bug, just moved.
- **`remember_context` early-returns when the selected project has no worktree** (`let Some(wid) = …
  selected_worktree() else { return }`). The per-workspace project record goes ABOVE that return, or an
  empty project silently never gets remembered.
- **`switch_workspace_quietly` must keep landing on row 0.** Restoring there re-introduces the
  attach-then-detach double the `/`-crosses-workspaces work added the quiet path to avoid
  (see [The Workspaces Column Remembers Itself]).
- **A one-project-per-workspace test can't fail.** Row 0 and "the row we left on" have to differ at all
  three levels, so the test seeds a second project (`p2`) with a non-main worktree (`w2b`) and its own
  agent. Same shape as the "only discriminates if the remembered session differs" trap in the 08-25
  entry — confirmed by commenting out `restore_workspace_project` and watching it go red
  (`left: Some("demo")`).

### `make dev` Still On 0.9.0: Pulling v0.10.0 Under Real In-Flight Work — 2026-08-26

**Asked:** "make dev is still showing the wrong version. pull latest from main into this"

**Did:** Same root cause as the v0.4.0 entry below — the shared checkout was at `1506bbf` / 0.9.0 while
`origin/main` was `0361f0a` / v0.10.0 (PR #16, the tab-bar merge). The difference this time: the dirty
tree was not stale leftovers but ~1,300 lines of *uncommitted, un-branched* work (the three entries just
under this one: cloud re-attach, workspace-delete confirm, unseen badges) based on `249668e`, overlapping
almost every incoming file. Recipe that worked, in order: `git stash create` (a stash commit without
touching the tree) → `git worktree add --detach <scratch> <that sha>` → `git merge origin/main` there →
resolve the 7 conflicts → `cargo build`/`clippy`/`test` with `CARGO_TARGET_DIR` outside the repo → then
in the shared tree `git diff --quiet <sha>` (nobody else edited meanwhile), `git stash push -u`,
`git merge --ff-only origin/main`, `git restore --source=<scratch commit> --worktree -- <files>`. Result:
HEAD = origin/main, working tree = v0.10.0 + the three features, still uncommitted, `target/debug/nebula
--version` → 0.10.0. The WIP is kept as `stash@{0}` ("cloud/unseen/ws-delete wip before v0.10.0 pull").

**Gotchas:**
- `git stash create` ignores untracked files, so the scratch merge failed with `E0583 file not found for
  module cloud` (`pty/cloud.rs`). Copy untracked files in by hand, and after the ff restore them from the
  stash's third parent: `git restore --source='stash@{0}^3' --worktree -- <paths>`.
- Conflict shape when the WIP already contains part of the incoming range: hunks where origin/main's later
  commits did not touch the file (registry.rs, store.rs migrations 19/20) resolve as **ours** wholesale;
  only files the tab-bar prototype (`30042e9`) rewrote needed thought — `leftmost_focus` → `first_focus`
  in `app.rs`, the `Action::Delete` arm in `event_loop.rs` (local `open_delete_confirm` already routes
  `Focus::Workspaces` through `open_remove_workspace_confirm`, so `ours` wins), and `ui.rs` where the
  `TAB_*` consts and the `ProjectRowData`/`WorktreeRowData` aliases land on the same lines (keep both).
  `git diff <base-commit> origin/main --stat -- crates/` tells you which files need thought.
- `git diff --quiet <commit>` only compares tracked files — it will say the tree matches even when
  untracked WIP is missing. `cmp` the untracked files separately.
- `workspace_scope_is_per_connection` (e2e_pty) failed twice under a parallel clippy+test run and passed
  alone: the Ack-beats-upsert load race the v0.10.0 entry describes, not the merge (test fixed
  2026-08-27, `6638952`).
- The old Makefile's dev daemon lives at `/tmp/nebula-dev`; the new per-checkout slot is
  `/tmp/nebula-dev-<8 chars of shasum of $CURDIR>` (`2f3f877f` for the main checkout), so the new
  `dev-stop` cannot see a daemon the old recipe started. A `make dev` TUI that was already open keeps its
  0.9.0 daemon until it quits — the old recipe's trailing `dev-stop` then reaps it. Quit and rerun.
- The new slot also means a fresh `$HOME/.nebula-dev/nebula-<slot>` data dir: first `make dev` re-seeds
  from the real DB instead of reusing the old `~/.nebula-dev/nebula.db`.

### Cloud Rows Re-Enter Their Session On Restart — 2026-08-26

**Asked:** "find if there is a way to attach claude when waiting for the cloud to finish so thhat they
don't need to go into abrowser to use" → after the diagnosis (live attach is flag-gated off for this
account, see the 08-24 Cloud entry): "yes do it" — capture the `session_…` id from the spawn output, keep
it on the Agent row, try `--cloud <id>` and fall back to `--teleport <id>` in a fresh worktree.

**Did:** `Agent.cloud_session_id` (entities.rs, store migration 20, protocol v26 — rmp positional
structs, so a new field is a bump). `crates/nebula-daemon/src/pty/cloud.rs::CloudScanner` reads the id
(`claude.ai/code/session_…` / `--teleport session_…`) and the attach refusal (`… not enabled for your
account`) off the PTY stream; `PtySession::arm_cloud_scan` replays the ring first so arming after spawn
cannot miss it; sightings are `PtyEvent::CloudSession` / `CloudAttachRejected`, persisted in
`watch_for_exit`. `registry.rs`: `CloudLaunch::{Create,Attach,Teleport}` drives
`claude_cloud_spawn_command` (`--cloud=<task>`, `--cloud=<id>`, `--teleport=<id>`);
`restart_agent` (now async) routes a row with `cloud_session_id` and no local `session_id` to
`attach_cloud_agent`, which re-homes a main-checkout row into a `cloud-<last 8 of id>` worktree, spawns
the attach, and `arm_cloud_attach_fallback` respawns as a teleport once the refusal was *seen* and the
child exited. **Superseded in part on 2026-08-27** ([Cloud Rows Mirror Their Session Instead Of Dying At
Create]): a create now re-enters on its own, the attach is only tried until this daemon has seen it
refused once, and the teleported pane keeps re-teleporting until it is typed into. `ClientRequest::AttachCloudAgent` + the "Attach cloud session" menu item force the chain
any time; the sessions list shows a `cloud` badge. e2e `cloud_row_captures_its_session_id_and_reenters_it`
walks the whole chain with a three-run stub. README step 4 documents it.

**Gotchas:**
- The teleport fallback must key on the refusal text, not on "exited non-zero fast": a deliberate kill
  (restart/archive) of a *working* attach exits non-zero too and would have spawned a stray teleport.
- Both `--cloud` and `--teleport` take an *optional* value — always bind with `=`; verified both forms
  parse (`--cloud=<id>` → the refusal, `--teleport=<id>` → teleport's stash prompt).
- Teleport refuses a dirty tree ("Stash changes and continue?") and both CLIs switch the checkout's
  branch, hence the mandatory fresh worktree for rows in the main checkout. The placeholder
  `cloud-…` branch stays behind once teleport checks the cloud branch out on top of it.
- `SessionEnded` only flips status from Running/NeedsFeedback, so the dead create row stays gray
  `Fresh` — the `cloud` badge and `alive:false` are the only tells. (The create no longer leaves a dead
  row at all as of 2026-08-27, but a failed create still does.)
- In e2e, a spawn's `EntityUpserted{alive:true}` reaches the client before the stub has executed a
  line: don't assert on stub side effects inside the `read_events_until` predicate (it is only
  re-evaluated per event) — wait for the event, then poll the file.
- Running `claude` from an untrusted dir (the scratchpad) hangs on the workspace-trust prompt; probe
  CLI behaviour from the repo checkout.

### Workspace Delete Asks First — 2026-08-26

**Asked:** "make sure deleting a workspace shows a confirmation"

**Did:** All three no-confirm paths — `d` in the Workspaces column, "Delete workspace" in its `m` menu, and
`d` in the `w` switcher — now go through the new `open_remove_workspace_confirm`
(`crates/nebula-tui/src/event_loop.rs`), which opens a `ConfirmDialog` with the new
`PendingAction::RemoveWorkspace { id, reopen_picker: Option<usize> }` (`app.rs`). `run_pending_action`
sends the `RemoveWorkspace` request on `y`; the daemon is still the guard (empty workspaces only, never
the last one), so a refusal after confirming just flashes as before. README rows for `w` and the
Workspaces column say "delete asks first". Tests `workspaces_column_verbs_act_on_the_open_workspace` and
`switcher_r_and_d_act_on_the_hovered_workspace` cover Esc (nothing sent) and `y` on both paths.

**Gotchas:**
- **A confirm replaces the overlay it came from, so the switcher's `d` needs a way home.** The old `d`
  left the `w` menu up and let the `EntityRemoved` delta drop the row in place. The confirm is
  `app.overlay`, which evicts the menu, so `reopen_picker` carries the switcher's hover row and both
  answers reopen it there (`reopen_workspace_picker`, also now used by `refresh_workspace_picker`) —
  the `ResetSettings` reopen-the-settings-overlay precedent. The Confirm key handler's Esc/`n` arm is
  where that routing lives; a confirm opened from the column passes `None` and closes to the panels.
- **The confirm dialog is sized to its longest message line** (`ui.rs` `Overlay::Confirm`, min 52
  columns, no wrapping), so a single ~85-column sentence outgrows a narrow terminal. Put a `\n` in
  long messages; the bulk-delete confirms already do.
- The switcher test renames `ws2` to `client` before it presses `d`, so asserting the dialog message
  against `'ws2'` fails — match the current name (or the id in the action), not the seed name.

### Unwatched Finishes Count On The Project And Worktree Rows — 2026-08-26

**Asked:** "i neeed a way to track when a session goes from yellow to green, it should put a counter in
the projects, worktrees row so I know how many terminals I need to check, as I navigate down into rows it
should decrement and eventually hide the notification counts"

**Did:** New `Agent::unseen` flag (`crates/nebula-core/src/entities.rs`), owned by the daemon.
`Store::set_agent_status` (`crates/nebula-daemon/src/store.rs`) now returns `(stamp, unseen)` and keeps
the flag in the same `UPDATE`: running/needs_feedback → finished raises it, staying finished keeps it,
leaving finished clears it, archived rows never raise it and `set_agent_archived` clears it; migration 19
adds the column. `ServerEvent::StatusChanged` carries `unseen`; new fire-and-forget
`ClientRequest::MarkAgentSeen { id }` → `Daemon::mark_agent_seen` (`registry.rs`) broadcasts the agent
upsert only when the flag actually flipped. `PROTOCOL_VERSION` 24 → 25. TUI:
`event_loop.rs::mark_agent_seen` runs from `attach()` (every path that lands the pane on a session goes
through it — cursor walk, restore, palette, snapshot re-attach) and from the `StatusChanged` arm when the
flip is for the session already in the pane; `app.rs::worktree_unseen` / `project_unseen` count;
`ui.rs::row_badges` draws ` n done` (`th.done`) on project and worktree rows (it also carried a note badge
until notes were removed 2026-08-26), and a
session row swaps its ` claude` harness badge for ` done` (the link row's unread-count idiom). The badge
read ` n new` in `th.ok` until 2026-08-27 — see [Done Reads Violet And Says "done"] for the rename and
for the dot splitting violet (unread) from green (read) on the same `unseen` flag this entry added. README
"Status dots" documents it. Tests: store `unseen_follows_the_status_and_clears_on_seen`; registry
`status_broadcast_carries_the_unseen_flag`, `mark_agent_seen_broadcasts_only_a_flip`; event_loop
`an_unwatched_finish_counts_until_the_cursor_lands_on_it`, `a_finish_in_the_pane_on_screen_is_already_seen`,
`unwatched_finishes_badge_the_rows_until_read`.

**Gotchas:**
- Daemon-side on purpose: the counter's whole point is turns that finished while no TUI was open, which a
  TUI-side set (even persisted in `UiState`) can never see — and two clients would clobber one blob.
  `pr_seen` / `MarkPrSeen` was the template.
- The rule lives in the SQL `CASE` of `set_agent_status`, not in `AgentStatusMachine`: the machine dedups
  unchanged statuses (`set_status` only emits on change), and the flag has to be atomic with the status
  it qualifies. Red → green counts (NeedsFeedback → Finished happens directly on a Stop / `idle_prompt`
  after a prompt); Fresh → Finished does not — a Stop nebula never saw the prompt for is not a
  yellow-to-green.
- Adding a field to `Agent` touches ~15 struct literals across four crates. The perl pattern
  `archived_at: 0,\n\s+pinned: false,` catches all but the test helpers that pass `pinned` as a
  variable and the one literal with `pinned: true` — let the compiler list the rest.
- `e2e_pty::workspace_scope_is_per_connection` failed 2 of 2 full-suite runs made while a
  `cargo build --release` ran alongside, then passed alone and 2 of 2 idle full-suite runs. Its
  `expect("AddProject upserts the project")` only sees the broadcast upsert if it lands before the Ack —
  `server.rs` writes the Ack from the request loop and the upsert from the broadcast forwarder, so under
  CPU contention the Ack can win. **Fixed 2026-08-27 (`6638952`):** the test now waits for the upsert as
  well as the Ack, like `cli_add_project` always did — see [A Paused Rebase Renamed The Worktree Row].

### One Nebula Per Checkout: Auto-Port For `browser`, Per-Path Dev Slots — 2026-08-26

**Asked:** "merge latest from origin main into this and verify it works, find a way to be able to run nebula
without port conflicts as i run in various worktrees"

**Did:** Surveyed what actually binds anything first — the answer is *one* thing. The daemon uses a unix
socket (`paths::socket_path()`), and the hook receiver already binds `127.0.0.1:0`
(`nebula-daemon/src/hooks/mod.rs:156`), so neither can ever clash. The only fixed port in the tree is
ttyd's, so that plus the Makefile is the whole fix.

(1) **`nebula browser` chooses its port.** New `resolve_port` / `probe` / `free_port`
(`crates/nebula/src/browser.rs`); `run_browser` takes `Option<u16>` and `main.rs`'s `--port` lost its
`default_value_t`. No `--port` → 7681 when free, else a kernel-chosen one with
`nebula browser: 7681 is busy — serving on N instead` on stdout. `--port 0` → any free one (it used to
`bail!("needs a fixed port")`). `--port N` → that port or an error, deliberately: silently moving would
break an `ssh -L N:localhost:N` aimed at it. `probe` binds and immediately drops a `TcpListener` — a
listener that never accepted doesn't enter TIME_WAIT, so ttyd rebinds cleanly a moment later.

(2) **`make dev` / `make browser` are keyed to the checkout.** `DEV_SLOT := $(shell printf '%s' '$(CURDIR)'
| shasum | cut -c1-8)`, `DEV_RUNTIME := /tmp/nebula-dev-$(DEV_SLOT)`,
`DEV_DATA := $(HOME)/.nebula-dev/$(notdir $(CURDIR))-$(DEV_SLOT)`, and `PORT ?=` (empty → no `--port`,
so (1) picks). New `make dev-ls` lists every slot and whether its daemon is up.

Verified for real, not just by test: two `nebula browser` processes at once took 7681 and 49293, both
answering `200`; and the merge of `origin/main` (v0.10.0) into the prototype branch builds and passes
629 tests. Merge conflicts were two and both trivial (main added `reset_settings`/`reopen_settings`
directly above `set_show_workspaces`; both sides prepended a memory entry).

**Gotchas:**
- **Sharing `DEV_RUNTIME` across checkouts is worse than a port clash, and silent.** Both worktrees'
  `make dev` pointed at `/tmp/nebula-dev`, so the second TUI just *connected to the first's daemon* and you
  drove the other checkout's binary — the exact "I rebuilt and my change isn't there" the Makefile warns
  about elsewhere. Worse, `dev-prep` calls `dev-stop`, so starting one worktree SIGTERMed the other's
  daemon out from under it. Nothing reports either; it looks like your build didn't take.
- **The runtime dir gets the hash alone, not the worktree name.** It holds the unix socket and SUN_LEN is
  104 bytes on macOS; `/tmp/nebula-dev-<8hex>/daemon.sock` is 36 and safe for any checkout name. The
  readable name goes in `DEV_DATA`, which has no such limit.
- **The old flat `~/.nebula-dev/{nebula.db,config.json,state/}` is now orphaned** beside the new
  `<name>-<slot>/` dirs. `dev-seed` re-copies from the real DB per slot, so nothing is lost — but the old
  files are dead weight and can be deleted.
- **`~/.nebula-dev/config.json` has `show_workspaces: false`.** A `make dev` in a fresh slot inherits it
  through `dev-seed`, so the Workspaces bar starts hidden and looks broken. `Shift+W`.
- **A merge that compiles can still be semantically stale.** `cargo build` passed; `cargo test` then failed
  on `Project has no field named divider_after` — main had removed the divider fields
  (see [Project Dividers Removed]) and a *test helper* I'd added still set them. Build the tests, not just
  the lib, before calling a merge verified.
- **Killing a `nebula browser` parent leaves its ttyd running.** Ctrl+C works because the two share a
  process group; a bare `kill <pid>` does not reach the child, and the orphan keeps the port. Kill the
  ttyd pid too, or you'll be hunting a phantom "port in use" later. Related: [Orphan e2e daemons].
- `nebula browser` really does `open` the URL — running it twice to test opened two tabs in the desktop
  browser. There is no `--no-open`.


### Prototype: The Workspaces Column Became A Top Tab Bar — 2026-08-26

**Adopted — on `main`** as of merge commit `0361f0a` (PR #16). It was written on the worktree branch
`worktree-workspace-tabs` and awaiting a verdict when this entry was first written; that verdict came in.
[The Workspaces Column Drags To Resize] and the column half of [A Workspaces Column Left Of Projects] are
superseded wholesale by it.

**Asked:** "in a worktree, protype having the workspaces actually be a top bar with WORKSPACES on the left
aligned vertically above PROJECTS, but on the right it lists out the workspaces as tab buttons, each with a
shortcut of cmd + [1-9] to select the workspace (or click), or when focus a user can use navigation keys to
toggle through"

**Did:** Replaced `draw_workspaces` (the 18-wide left column) with `draw_workspaces_bar`
(`crates/nebula-tui/src/ui.rs:~2320`), a 3-row strip across the top of the body: blank spacer, then
`   WORKSPACES · n` plus one tab per workspace, then a full-width rule **broken under the open tab** so it
reads as joined to the panels. The label reuses `ROW_GUTTER`, so it lands on the same x=3 / row-1 grid as
the panel headers and sits exactly `WORKSPACES_BAR_H` rows above `PROJECTS`. Each tab is
` <digit> <dot><name><count> ` — the count was running-sessions until 2026-08-27, when it became
` n done`, the workspace's unread finishes; see [Done Reads Violet And Says "done"] — and the bar scrolls
horizontally with `‹`/`›` marks when the tabs
outrun the width.

Layout plumbing: `App::workspaces_panel_w()` → `workspaces_bar_h()`, `WORKSPACES_BAR_H = 3` replaces
`DEFAULT_WORKSPACES_PANEL_W`, and **splitters were reindexed back to `0..3`** (`splitter_x(idx)` is now
`panel_widths[..=idx].sum()`, `set_splitter` lost its `idx == 0` branch and its offset). `App::workspaces_w`
and `UiState::workspaces_w` are gone — old blobs still load, the key is just ignored.
`leftmost_focus()` → `first_focus()`, since the bar is above rather than left.

Keys: new `Action::SelectWorkspace(u8)` with nine `workspace_slot!` `ACTIONS` rows
(`keymap.rs`, ids `select_workspace_1..9`), each defaulting to **both** `cmd+N` and the bare `N`. In the
bar, `←`/`→` walk the tabs (`move_selection`, which still does a full `switch_workspace` per step), `↓`
steps out to Projects, `↑` no-ops; `Tab`/`Shift+Tab` still include the bar, `←` from Projects no longer
does. `⌘N`/`N` fires from any panel and deliberately leaves focus where it is.

Post-merge with v0.10.0: 447 nebula-tui unit + 22 e2e_pty + 6 e2e_tui + 130 daemon green, fmt clean,
clippy identical to a stashed
clean-tree baseline. Release binary built at
`.claude/worktrees/workspace-tabs/target/release/nebula`.

**Gotchas:**
- **`⌘1`–`⌘9` cannot work in the user's terminal and never will.** `keymap::host_warning` already returns
  `Reach::Blocked` for any SUPER chord — Terminal.app never encodes ⌘ into pty bytes (⌘P is File→Print at
  the menu layer). The bare digits are what actually fire; the ⌘ bindings are there for iTerm2/Ghostty in
  kitty mode and for the Hotkeys tab to flag honestly. Digits were entirely unbound before this
  (`grep 'defaults: &\[' keymap.rs` confirms), so nothing collided — but a digit pressed in *any* panel now
  switches workspace, which is a behavior change worth re-checking if something starts jumping.
- **`hit_at` is first-match, so `HitTarget::PanelBg` must be pushed after every tab rect.** Registering the
  bar's background before the tabs made every tab click a no-op that only moved focus — and the symptom was
  a *click* test failing three asserts later, not where the push was.
- **`render_button`'s dim→muted lift is not free when you render a Paragraph yourself.** The open tab is
  drawn with `.style(row_bar(..))` rather than through `render_button`, so the "● " fresh dot stayed
  `th.dim` and sank into the selection fill; the lift loop has to be copied. `TestBackend` reports it as
  `left: DarkGray / right: Gray`.
- **`normalize_panel_widths` only ever shrinks.** A test asserting `[20, 22, 38]` after normalizing to a
  100-wide body is wrong — the defaults `[20, 22, 32]` already fit the 80-column budget, so nothing moves.
- **`crates/nebula/tests/e2e_tui.rs` identifies the focused panel by a literal footer string.**
  `FOOTER_WORKSPACES` was `"w: switcher"`; rewording the Workspaces footer hint broke
  `tui_projects_worktrees_agents_navigation` with a 20-second timeout rather than an assertion — the failure
  reads like a hang, not a text change. Any footer edit has to check those five consts.
- **An empty non-default workspace still shows the splash on `origin/main`.** The
  [Splash Is Scoped To The Default Workspace] fix is in the shared checkout's uncommitted diff, not in
  `origin/main`, so a worktree cut from `origin/main` does not have it: a test that switches to a freshly
  seeded workspace gets the nebula splash instead of the panels. Seed a project into the workspace you
  switch to.
- The shared tree was dirty from other sessions again ([Shared tree races]) while `HEAD...origin/main` read
  `0 0`, so `EnterWorktree` off `origin/main` gave a clean base with none of their in-flight edits.

### The Startup Snapshot Restores The Cursor But Left The Pane Blank — 2026-08-26

**Asked:** "when nebula first loads, it seems to auto remember my last select pref, but it doesn't seem
to show the focused session terminal"

**Did:** The `ServerEvent::Snapshot` arm in `crates/nebula-tui/src/event_loop.rs` (`handle_server_event`)
called `restore_ui_state` to re-seat `sel_project` / `sel_worktree` / `sel_session` from the persisted
`UiState` blob and then stopped — nothing ever sent the `Attach`. `restore_ui_state` now returns `bool`
("the remembered `session_agent` landed under the cursor") and the Snapshot arm calls `preview_selected`
when it does, so the pane comes back with the row, focus staying on the panels exactly like a cursor
move. No blob, or a blob whose session is gone/archived, still leaves the pane blank. Unit test
`snapshot_reattaches_the_remembered_session`. TUI-only change: reopening the TUI is enough, the daemon
does not need a restart.

**Gotchas:**
- `restore_context` / `restore_session` can't be reused on the startup path: they read
  `last_worktree_for_project` / `last_session_for_worktree`, which `remember_context` only fills as the
  user moves *away* from a context — both maps are empty on the first snapshot, so `restore_session`
  would blank the pane it was asked to restore. The blob's ids are the only memory at boot.
- Snapshot is a one-shot reply to `Subscribe` (event_loop.rs:136); the TUI never resubscribes, so
  attaching there can't double up. If a reconnect path ever re-sends it, `attach`'s already-attached
  early return is what keeps this safe.

### "Do This In A Worktree" Goes Through `nebula worktree`, Not A Button — 2026-08-26

**Asked:** "remove the move to worktree button and instead find a better way to hook into when a user
prompts for a worktree, claude via a skill + system prompt or something knows to create the proper
worktree in nebula and assiocate the sesion with it"

**Did:** The Sessions context-menu verb "Move to worktree" and its picker are gone
(`MenuAction::MoveAgent`/`MoveAgentToWorktree`, `open_move_agent_picker` in
`crates/nebula-tui/src/event_loop.rs`); `ClientRequest::MoveAgent` stays as the daemon primitive
(e2e `move_agent_respawns_live_session_in_target_worktree` still covers it). In its place:

- **CLI** `nebula worktree [name…] [--base <ref>]` (`crates/nebula/src/main.rs`,
  `ipc::enter_worktree_for_current_agent`) — same `NEBULA_AGENT_ID` + socket path as `nebula rename`;
  spaces slugify, no name = `branch_name::random_name`. Sends the new `ClientRequest::EnterWorktree`,
  gets `ServerEvent::WorktreeEntered { worktree, outcome: EnterOutcome }` back. Its stdout is written
  for the model that ran it ("finish now, you'll be resumed inside the worktree").
- **Daemon** `Daemon::enter_worktree` (`registry.rs`): existing branch row or `create_worktree` (nebula's
  `<repo>-worktrees/<branch>` layout), `set_agent_worktree` + broadcast **immediately**, and — only if
  the PTY is alive — an entry in the new `pending_moves` map. `complete_pending_move`, called from the
  hook drain loop in `lib.rs`, does the kill + `claude --resume <sid> … "<relocation prompt>"` respawn on
  `Stop` (or `Notification idle_prompt`). Any other spawn of the agent clears its pending entry.
- **Claude guidance** rides `--append-system-prompt` on every non-cloud claude spawn
  (`CLAUDE_WORKTREE_GUIDANCE`, `agent_spawn_command_with`): don't use `EnterWorktree`, run
  `nebula worktree <name>`, then end the turn. Installer adds `Bash(nebula worktree:*)` to the allow list
  next to the rename rule (`CLAUDE_ALLOW_RULES`). Rejected a `~/.claude/skills` install: a skill is only
  loaded on description match and would live outside nebula's per-spawn hook management.
- **TUI** follows a daemon-initiated re-home: an agent upsert whose `worktree_id` changed for the
  *selected* session sets `select_when_seen` (event_loop.rs `Entity::Agent` arm), so the cursor and pane
  ride along instead of landing on whatever slid into the slot. Also helps the hook-cwd reparent.

Tests: `enter_worktree_*` + `pending_relocation_ignores_the_old_cwd_until_the_turn_ends` (registry),
`spawn_command_initial_prompt_is_claudes_positional_argument`,
`selection_follows_the_selected_agent_when_the_daemon_rehomes_it` (TUI), and e2e
`nebula_worktree_cli_relocates_the_session_when_the_turn_ends`. README updated.

**Gotchas:**
- **The CLI the model runs *is* the session's foreground tool call.** `move_agent`'s kill-and-respawn
  can't be reused directly — it would cut claude off mid-turn with a dangling tool_use. Hence the
  two-phase design: row now, process at the turn's `Stop`. Ordering in the `lib.rs` drain loop matters:
  `reparent_agent_by_cwd` runs *before* `complete_pending_move`, because the `Stop` payload itself still
  carries the old checkout's cwd and must be ignored (pending guard in `try_reparent_agent_by_cwd`)
  before the pending entry is consumed. The e2e posts a `PostToolUse` + `Stop` with the old cwd to pin
  this down.
- **Claude can't `cd` out of its start directory** (hook cwd is reset outside the workspace root — see
  the 08-23 EnterWorktree experiment in the user's auto-memory), so a restart is the *only* way to put
  the process in nebula's sibling worktree layout. `claude --resume <sid> "<prompt>"` is the documented
  resume-with-initial-prompt shape and `--append-system-prompt` is listed for interactive use in
  `claude --help` (2.1.246); the argv shape is unit-tested, but **the live auto-continue after a resume
  was not exercised in this session** — first thing to watch when trying it for real. Codex/cursor get no
  continuation prompt (unknown whether their resume takes one); they come back idle.
- **Proving a respawn landed in the right directory without attaching:** the e2e stub agent does
  `pwd >> $NEBULA_AGENT_ID.pwd` on every boot, so "one line, then two lines with the second ending in
  `repo-worktrees/feat-x`" is the whole assertion — cheaper than the Attach/Input/`pwd` dance the
  MoveAgent e2e uses.
- `agent_spawn_command` (the 5-arg form) is now `#[cfg(test)]`: production goes through
  `agent_spawn_command_with(.., initial_prompt, guidance)`, and clippy flagged the wrapper as dead.

### Settings Reset To Defaults Behind A Confirmation — 2026-08-26

**Asked:** "on settings modal add a hotkey to reset to default with confirmation that your settings will
be cleared"

**Did:** `Shift+R` anywhere in the settings overlay (tab strip or list) swaps in a `ConfirmDialog` with
`PendingAction::ResetSettings`; confirming runs `reset_settings` (`crates/nebula-tui/src/event_loop.rs`),
which calls the new `Config::reset_to_defaults()` (`config.rs`), `apply_config`s the result, replaces
`app.keymap` with the default keymap, and reopens the overlay on its remembered tab/row with an info
notice. Esc/`n` on that particular confirm reopens the overlay too (special-cased in the Confirm key
handler) instead of dropping back to the panels. The key is deliberately not in the rebindable keymap —
none of the overlay's own keys are. Hints in `ui.rs::settings_keys_hint` and the README say `R: reset all`.

**Gotchas:**
- **`Config::save()` is a patch, not a write — it can't reset.** It merges the TUI's known keys into
  whatever JSON is already in `config.json`, on purpose, so daemon-owned keys (`prewarm_agents`,
  `prewarm_sessions`, hand-added ones) survive every overlay edit. Saving `Config::default()` through it
  would leave those behind. `reset_to_defaults` writes over `json!({})` via the split-out `write_into`,
  so the file reads as never-edited; `config::tests::reset_rewrites_the_file_from_scratch` pins the
  difference.
- **The settings modal's inner width is 82 columns and `settings_keys_hint` is `truncate`d to it** (with
  a leading space, so ≤81 usable). Adding a key to the hotkeys-tab hint pushed it to 86 and silently
  chopped the end; shorten wording rather than appending.
- **Another session was editing `app.rs` while this ran:** `SettingsView::new` grew a third `on_tabs`
  argument between my first read and my edit. Re-read any signature you call right before writing the
  call, then `grep` your symbols after the build to confirm they're still on disk (see [Shared Working
  Tree Is Raced By Other Sessions]).

### Settings Opens On The Tab Strip — 2026-08-26

**Asked:** "when I load up the settings, it should always focus on the tab (or last selected tab +
option combo)"

**Did:** `App::settings_on_tabs` (`crates/nebula-tui/src/app.rs`, default `true`) joins the existing
`settings_tab` / `settings_selected` memory, and `SettingsView::new` grew a third `on_tabs` arg. A first
open now parks on the tab strip (←/→ walk tabs immediately, no ↑ first); every later open restores the
tab, the row, *and* whether the cursor was on the strip or in the list. `remember_settings_focus` is
called from `SettingsCmd::FocusTabs` / `EnterList` and from both settings mouse-click paths in
`event_loop.rs`.

**Gotchas:**
- Twelve existing tests silently assumed `s` lands **in the list** — with the strip focused, `j` means
  "drop into the list" and `Enter` means the same, so hotkey-capture and value-cycling tests all failed
  in ways that looked like keymap bugs (`left: "?" right: "F6"`). The fix is one place: the shared
  `open_settings_on` test helper now presses `↓` after picking the tab. Route new settings tests through
  it rather than pressing `s` and navigating.
- All this state is per-process only — `UiState` (the blob persisted in the daemon DB) was deliberately
  left alone, so a fresh `nebula` always starts on the strip.

### Project Dividers Removed From The Projects Column — 2026-08-25

**Asked:** "remove the ability for a user to divide the projects column"

**Did:** Deleted the divider feature end to end (~1.4k lines). `Project` lost its four `divider_*` fields
(`crates/nebula-core/src/entities.rs`); `ClientRequest::SetProjectDivider` / `MoveDivider` are gone from
`protocol.rs` and `server.rs`; `Daemon::set_project_divider` / `move_divider` and the leading-divider
hand-down in `remove_project` are gone from `registry.rs`, and `move_project` is now a plain remove/insert
that renumbers `sort_order` to the display index. Store: `insert_project` / `set_project_position` /
`get_project` / `load_tree` no longer touch the columns, and **migration 18** is four
`ALTER TABLE projects DROP COLUMN`s (`migration_18_drops_the_divider_columns` seeds a v17 DB with a
labeled divider and checks `PRAGMA table_info`). TUI: the `ProjectRow` enum is gone — `App::project_rows()`
is now `Vec<usize>` (indices into the full `tree.projects`, workspace-filtered) and
`selected_project_row()` became `selected_project_index()`; `divider_focused()`, `select_divider_when_seen`,
`PromptKind::DividerLabel`, `MenuAction::{SetProjectDivider,LabelDivider}`, `Action::ToggleDivider` (`-` is
now unbound), `ui::divider_spans`, the "you're focused on a separator" pane, and
`SelectionSnapshot::{project_kind,divider_chase}` are all removed. README lost its three divider rows.
Workspace: 618 tests green, clippy/fmt clean.

**Gotchas:**
- A user keybinding config that still names `toggle_divider` is harmless: `Keymap` logs
  `ignoring keybinding for unknown action` and moves on (`keymap.rs:~849`).
- Migrations 2, 3, 7 and the migration-14 table rebuild still spell out the divider columns — they must,
  since they already ran on every existing DB. Only migration 18 drops them; don't "tidy" the old SQL.
- Two older entries (PR rows in the Worktrees panel, PR preview pane) described their behavior as a copy of
  "the divider precedent" — those early returns now stand on their own and the entries were updated.

### The Splash Is Scoped To The Default Workspace — 2026-08-25

**Asked:** "when a user has multiple workspaces, and he hovers over a workspace with no projects, it should
NOT show the nebula splash screen. that screen should only show when a user is on default workspace with no
projects"

**Did:** `App::splash_showing()` (`crates/nebula-tui/src/app.rs:~2170`) now requires the open workspace to
be the built-in `default` one before an empty tree counts as a first run: new
`Tree::in_default_workspace()` (`app.rs:~1640`, compares `active_workspace` to
`nebula_core::DEFAULT_WORKSPACE_ID`). `splash_preview` (N) is unchanged. An empty non-default workspace
now renders the normal layout — Workspaces column plus the three panels with their existing "no projects
yet / n adds one" hints — instead of swapping the whole body for the nebula. The "hover" in the request is
`move_selection` in the Workspaces column (`event_loop.rs:~4864`), which does a full `switch_workspace`
per step, so previously stepping onto a fresh workspace hid the column you were stepping through.
Flipped `switching_to_empty_workspace_blanks_the_pane` to assert `!splash_showing()`, added
`empty_non_default_workspace_keeps_the_panels_not_the_splash` (TestBackend draw: "WORKSPACES" and "no
projects yet" on screen after the step, splash back after stepping to the empty default). nebula-tui: 446
green.

**Gotchas:**
- The shared tree didn't compile while this was done — another session was mid-removal of the divider
  feature (`ClientRequest::MoveDivider` / `SetProjectDivider` gone from nebula-core, ~700 lines in flux).
  Verified by `git worktree add --detach <scratchpad>/wt HEAD`, re-applying only these hunks there, and
  running `cargo test -p nebula-tui` in that worktree. Same recipe works for any change while
  [Shared tree races] is in effect; remove the worktree afterwards (`git worktree remove --force`).
- `Tree::has_visible_projects()` is deliberately still workspace-scoped and still drives the panel hints,
  `Action::New`'s add-project shortcut, and the splash's own "create your first project" line — only the
  splash gate got the default-workspace condition. Don't fold the check into `has_visible_projects`.

### The Workspaces Column Remembers Itself, And `/` Crosses Workspaces — 2026-08-25

**Asked:** "remember if someone had the workspaces panel collapsed so you don't show it the next time,
also allow to configure showing it or not in the settings" — then, mid-task: "also allow the jump to to
include the entire workspaces path so I can quickly jump between workspaces whenever"

**Did:** Two things.

(1) **Visibility moved out of the UI blob into the config file.** `UiState::show_workspaces` is deleted
(`crates/nebula-tui/src/app.rs`); the home is now `Config::show_workspaces` (default true), with a
`SettingKind::ShowWorkspaces` row at the bottom of the **Appearance** tab. `Action::ToggleWorkspaces`
(`event_loop.rs:~1330`) saves the file as it flips, so the choice survives a kill, a crash, or a closed
`nebula browser` tab — `ui_state_json` is only sent on `app.should_quit`, which is exactly why the old
one didn't stick. New `apply_config(app, &cfg)` + `set_show_workspaces(app, shown)` are shared by startup
and `apply_setting_at`, so the settings row and the hotkey are the same code path.

(2) **`/` is no longer workspace-scoped.** `build_palette_items` (`app.rs:~824`) now walks
`palette_workspace_order(tree)` — active workspace first, then tree order — emitting a
`PaletteTarget::Workspace` row per workspace plus every project/worktree/session/PR under it, each
pathed `workspace/project/branch/session`. `jump_to_target` switches workspace first via the new
`target_workspace()`; a workspace row gets the full `switch_workspace`, everything else gets
`switch_workspace_quietly` (new `switch_workspace_inner(.., restore: bool, ..)`).

629 tests green, fmt clean, no new clippy warnings. README + the `palette` keymap hint updated.

**Gotchas:**
- **A hotkey that writes `Config::save()` makes the test suite edit the dev's real settings file.**
  `shift_w_toggles_the_workspaces_column_and_parks_focus` had no path override, so the run wrote
  `show_workspaces: false` into a live `config.json` — and because an agent working inside `make dev`
  has `NEBULA_DATA_DIR=~/.nebula-dev` exported, it lands in the dev instance's config, not the one you'd
  think to check. It also correlated with `e2e_pty::workspace_scope_is_per_connection` failing 4 of 5
  full-suite runs (0 of 7 on a clean tree, and never when e2e_pty ran alone); the failure went away for
  good once the test was pinned. `Config::save()` now `assert!`s in `#[cfg(test)]` that
  `CONFIG_PATH_OVERRIDE` is set — wrap any test that presses such a key in `with_default_config`.
- **A cross-workspace jump attaches twice if you reuse `switch_workspace`.** It calls
  `restore_context` → `restore_session` → `attach`, so the destination's *remembered* session gets
  attached, then detached one request later when the jump lands on the row actually picked:
  `[Detach a1, Attach a8, OpenWorkspace, Detach a8, Attach a9]`. Hence `switch_workspace_quietly`.
  The test for this only discriminates if the remembered session **differs** from the jump target —
  with one agent in the destination workspace, `attach`'s already-attached early return hides the bug
  and the test passes either way.
- **A quiet switch means the branches' early-outs are wrong.** The cursor can already sit on the target
  row in the new workspace while the pane still shows the workspace you left, so `PaletteTarget::Project`
  needs `switched || changed` and `PaletteTarget::Worktree` needs `!switched && …` on its early return.
- **`build_palette_items` must not require `tree.workspaces` to be complete.** Grouping strictly by the
  workspace list emptied the palette for every `seed_tree`-only test (projects carry
  `workspace_id: Default::default()`, and nothing upserts the matching `Workspace`). A project whose
  workspace is unknown still gets its rows, just with no path prefix — vanishing from the
  find-anything tool is the worst failure it has.
- `TestBackend` renders the palette rows, so a failing palette assertion prints the whole modal — that
  dump is the fastest way to eyeball glyphs and paths (`◇ client` / `▫ client/secret`).

### Releases v0.8.0, v0.9.0 And v0.10.0 — A Clean Base Needs No Merge — 2026-08-25

**Asked:** "ok pull in latest from origin/main then and merge into this work them commit push and make
the next release"

**Did:** Nothing to pull — local `main` already tracked `origin/main` at `8beee5a` (the shared tree was
reconciled between releases), so no merge was needed and the release skill's plain recipe applied:
worktree cut from `origin/main`, the six dirty files copied in by content, `274ece8` (feature), `db718af`
(memory), `199011c` (bump to 0.8.0, tagged `v0.8.0`). 624 tests green, all 4 matrix targets built, notes
rewritten. Shipped the draggable Workspaces column, the ttyd `fontSize` refit, and the `make dev` /
`make browser` isolated dev instance — each already has its own entry above.

**Then v0.9.0**, asked as "commit and push and do another release" — same clean path, same recipe: base
read `0 0`, seven dirty files copied in, `2730e9f` (feature), `7ddee41` (memory), `12554d1` (bump to
0.9.0, tagged `v0.9.0`). 629 tests green, all 4 matrix targets built, notes rewritten. Shipped the
persistent Workspaces column and the cross-workspace `/` palette (entry above).

**Then v0.10.0** (2026-08-26), asked as "commit and push and release" — same recipe at a much larger
scale: base `0 0`, nineteen dirty files (~3.7k lines, six finished tasks bundled in one tree) copied in,
`78a1714` (feature), `249668e` (memory), `fd45d42` (bump to 0.10.0, tagged `v0.10.0`). 628 tests green,
all 4 matrix targets built, notes rewritten. Shipped `nebula worktree`, settings `R` reset, settings
tab focus, snapshot re-attach, divider removal, and the default-workspace splash gate (entries above).

**Gotchas:**
- Nothing bit us. Worth recording only as the contrast to [Release v0.7.0]: the copy-files-into-a-worktree
  recipe is safe **exactly when** `git rev-list --left-right --count HEAD...origin/main` reads `0 0`.
  Check that before choosing between copying and merging — it is the one-line test for which of the two
  release paths you are on.
- Two clean releases in a row now. The shared tree is left dirty-but-identical after each one (the
  release worktree does the committing, `main` never moves locally), so the reconcile the user needs is
  `git reset --hard origin/main`, not a merge — `origin/main` is a strict superset of what their working
  copy holds. Say that explicitly; a plain `git pull` on top of those identical-but-uncommitted files
  just stalls.
- With many files, verify the copy in one line instead of eyeballing hunks: `git diff --stat` in the
  shared tree and `git -C "$W" diff --stat` in the worktree must be byte-identical. Leave untracked junk
  (`random.txt`) behind — `git diff --name-only` never lists it, so the loop skips it on its own.
- `cargo clippy --workspace --all-targets` carried 8 warnings at v0.10.0 (unneeded `return`, items after a
  test module, `&` on an auto-deref, `len() == 1`, a complex-type lint). They are **not** a release
  blocker: the only workflow is `.github/workflows/release.yml` and it runs no clippy or fmt step. Report
  them, don't fix them under a "release" ask.

### `nebula browser` Terminal Stopped ~24 Columns Short — 2026-08-25

**Asked:** "when running nebula browser, there is a bunch of empty space in the right side of the terminal
panel... fix this. running nebula in iterm or ghostty doesn't have this extra space"

**Did:** One line of ttyd args: `ttyd_args` (`crates/nebula/src/browser.rs:87`) now passes
`-t fontSize=13`, plus a test `a_font_client_option_is_passed_so_ttyd_refits_after_the_renderer_swap`.
The TUI was never at fault — it filled every column it was given; the xterm.js **grid** was too narrow.
Measured before/after through the real `nebula browser` at a 1600px window: 201 cols / 1407px grid /
183px dead → 225 cols / 1575px grid / 15px. Rejected `-t rendererType=dom` (also fills the width, since
the DOM renderer never rounds — but it is the slow renderer for a TUI that redraws constantly).

**Gotchas:**
- **The cause is a measure/render split inside xterm, invisible to the server.** ttyd calls
  `fitAddon.fit()` right after `Terminal.open()`, while the **DOM** renderer is live and
  `dimensions.css.cell.width` is the raw measured advance (7.8267px at size 13, Menlo).
  `cols = floor(avail / cellWidth)` → 201. ttyd *then* swaps in WebGL/canvas, which **floors the cell to
  a whole pixel (7px)** and never re-fits. 201 × 7 = 1407px of grid in 1590px of page.
- **`-t fontSize=13` is ttyd's own default and looks like a no-op — it is load-bearing.** ttyd's
  `applyPreferences` loop ends in `t.options[r]=n, 0===r.indexOf("font") && i.fit()`: *any* client option
  **named** `font…` buys a second `fit()`, and that one runs after the renderer swap. `rendererType` is
  merged in ahead of the server's `-t` keys, so the ordering holds. The test guards the flag, the `font`
  prefix, and its position before `--`.
- **Rows never showed the bug** — the cell height was already an integer (15px), so flooring changed
  nothing vertically. A "why is only the width wrong" symptom is the tell for integer-rounding of a cell.
- **`ps` renders `-t fontSize=13` as `-t fontSize 13`.** ttyd's `strsep(&option, "=")` NULs the `=` in
  argv in place. That is ttyd having *parsed* it, not nebula having passed it wrong.
- **Scale depends on `devicePixelRatio`.** At dpr 1 (external monitor, headless) the floor is to a whole
  pixel → ~10% loss; on Retina it floors to a half pixel → ~4%. Don't conclude "not reproducing" from a
  Retina window alone.
- **`--virtual-time-budget` cannot screenshot ttyd** — it fast-forwards timers and tears the page down
  while the PTY bytes are still arriving in real time. Drive Chrome over CDP instead: launch
  `--headless=new --remote-debugging-port=9333 --user-data-dir=/tmp/cdp-profile --window-size=1600,1000`,
  then `Page.navigate` + a real `setTimeout` + `Page.captureScreenshot`. Node 22 has a global `WebSocket`,
  so the whole client is ~25 dependency-free lines.
- **ttyd exposes the live terminal as `window.term`** (no React/preact fiber to dig through — the
  container has no framework keys). `window.term._core._renderService.dimensions` and
  `_charSizeService.width` are what prove a measure/render mismatch; the `.xterm-helper-textarea`'s inline
  `width`/`height` are the rendered cell dimensions if you only need a quick read.
- A `make browser` run of your own leaves a `ttyd … ~/.cargo/bin/nebula` on 7681 that is **the user's**,
  not test residue. Match on the port you started before `pkill`.

### The Workspaces Column Drags To Resize — 2026-08-25

**Asked:** "also allow dragging the workspaces panel to resize like we do on the other panels"

**Did:** Reversed the "not draggable" decision from [A Workspaces Column Left Of Projects]. The column's
width moved out of the `WORKSPACES_PANEL_W` const into `App::workspaces_w`
(`crates/nebula-tui/src/app.rs:1894`), seeded from `DEFAULT_WORKSPACES_PANEL_W = 18` and persisted as its
own `UiState::workspaces_w: Option<u16>` field rather than as a fourth slot in `panel_widths` — that blob
stays `[u16; 3]`, so every saved layout still deserializes. **`HitTarget::Splitter(usize)` was reindexed:
0 is now the workspaces|projects boundary, and the three old splitters became 1/2/3.** New
`App::splitter_indices() -> Range<usize>` returns `0..4` or `1..4` depending on `show_workspaces`; both
`ui.rs` loops (grab-zone registration at `ui.rs:78` and `draw_splitter_grips`) iterate it instead of
`0..3`. `splitter_x` dropped its inclusive range (`panel_widths[..idx]`, not `[..=idx]`) so idx 0
naturally means "the column's right edge". Drag/hover/pointer-shape handling in `event_loop.rs` needed no
changes at all — it was already index-generic. 2 new tests, 4 updated; whole workspace suite green (440
tui unit + 21 e2e_pty + 6 e2e_tui + 133 daemon).

**Gotchas:**
- `set_splitter(0, …)` is *not* the same shape as the other three: the column starts at x=0, so the
  boundary x IS the width — no `offset + left` subtraction. Reusing the panel branch gives a column that
  drifts under the cursor.
- `normalize_panel_widths` had to clamp `workspaces_w` **before** computing the panel budget
  (`max = body_w - 3*MIN_PANEL_W - MIN_TERM_W`). Without it, a width dragged out on a wide screen
  survives into a narrow one, the budget goes to zero, all three panels floor at `MIN_PANEL_W` anyway,
  and the layout overflows the body.
- The grip for splitter 0 lands on the Workspaces panel's own `Borders::RIGHT` cell, which exists — but
  a body only 120 wide with the default panels caps the column at 26 (`120 - 74 - MIN_TERM_W`), so a test
  that drags it to 30 and asserts 30 fails at 26. Pick drag targets inside that headroom.
- `seed_splitters` in `event_loop.rs` tests still hides the column (its `x = 20, 42, 68` depend on it),
  so its loop is `app.splitter_indices()` = `1..4` — every assertion in the drag/hover tests that read
  `idx == 0` for the projects|worktrees boundary had to become `1`.

### `make dev` Showed v0.4.0 And No Projects — 2026-08-25

**Asked:** "still when I run make dev, it shows version v0.4.0 in the bottom left and now it seems like
all my projects and workspaces are done [gone]"

**Did:** Two unrelated causes. (1) The shared checkout was still at `026b64c` / `Cargo.toml` 0.4.0 while
`origin/main` was at v0.7.0 — every release since had been cut from a private worktree and never pulled
back, so `make dev` (which builds *this* checkout) faithfully reported 0.4.0. Synced it: `git stash -u`
(kept as `stash@{0}` for safety), `git pull --ff-only`, then restored only the `Makefile` from the stash.
(2) `make dev` runs with `NEBULA_DATA_DIR=~/.nebula-dev`, a deliberately separate DB, so it had zero
projects by design. Added `dev-seed` (Makefile) — on the first `make dev` it `sqlite3 .backup`s the real
DB into the dev dir, `DELETE`s `agents` and `terminals`, and copies `config.json`/`reviewed.json`; plus
`dev-reset` (wipe, so the next run re-seeds) and `make dev SEED=0` (start blank). Verified: dev DB got
7 projects / 3 workspaces / 9 worktrees / 0 agents, real DB untouched, and `nebula workspace list` against
the dev env booted a 0.7.0 dev daemon on it with a clean log.

**Gotchas:**
- **The real data dir is `~/Library/Application Support/dev.nebula.nebula/`** on macOS
  (`directories::ProjectDirs::from("dev","nebula","nebula")`, `nebula-core/src/paths.rs`);
  `$XDG_DATA_HOME/nebula` on Linux. Nothing prints it — the Makefile mirrors the rule by hand.
- Every dirty file in the shared tree except the `Makefile` was either byte-identical to `origin/main` or
  *older* than it (the pre-#12 `h`/`l` bindings and the macOS-only clipboard) — the same stale hunks the
  v0.6.0/v0.7.0 entries describe. `git diff origin/main -- <file> | grep -c ^@@` per file is the quick
  way to tell in-flight work from leftovers before discarding anything.
- Agent rows are only spawned lazily (`Registry::ensure_session`, `registry.rs:~1857`), so copying
  `agents` would not have launched anything at boot — they're dropped anyway so the dev instance can't
  `--resume` your live claude sessions.
- `sqlite3 ".backup"` reads the WAL, so the snapshot is consistent with the real daemon still running;
  a plain `cp nebula.db` would miss everything in `nebula.db-wal`.

### Release v0.7.0 — Merge, Don't Copy, When The Shared Tree Is Behind — 2026-08-25

**Asked:** "commit and push and make a next version release"

**Did:** Released the shared tree's uncommitted Workspaces column and the new `nebula browser`
(`crates/nebula/src/browser.rs`, ttyd on loopback) as `v0.7.0`. Commits `1e0c093` (feature), `71e2035`
(memory), `ead231d` (merge), `d9239d1` (focus-walk fix), `3306dc8` (bump). All 4 matrix targets built;
notes rewritten.

**Gotchas:**
- **The release skill's "copy files by content into a worktree cut from `origin/main`" is wrong when
  local `main` is behind.** It was 12 commits behind here, and the dirty files predate all of them, so
  copying would have silently reverted the `h`/`l` panel remap (#12), the Linux clipboard (#11), and the
  pill-corner fix. What works: `git worktree add -b release-vX.Y.Z "$W" <local HEAD>` — the dirty files
  share that base, so a straight `cp` reproduces `git diff HEAD` exactly — commit there, **then**
  `git merge origin/main`. Only `.claude/MEMORY.md` and `README.md` conflicted.
- **A textually clean merge is not a green one.** Every code file auto-merged, and
  `event_loop.rs::h_and_l_walk_panel_focus_like_the_arrows` still failed: it asserts `h` from Projects
  "stops at projects", written before the Workspaces column made `App::leftmost_focus()`
  (`crates/nebula-tui/src/app.rs:2701`) return `Focus::Workspaces`. The `Action::FocusLeft` hint in
  `keymap.rs` carried the same stale wording. Merge conflicts flag the text collisions, not the
  semantic ones — run the suite before you tag, not after.
- The `README.md` conflict is the same staleness in prose: keep our new rows, but take `origin/main`'s
  `Shift+H` / `Shift+L` wording. The `.claude/MEMORY.md` conflict is pure placement — both sides append
  at the top of `## Entries` and `origin/main`'s half of the conflict is empty, so strip the three
  markers and keep ours.
- `assets/screenshot.png` shows as staged-added in the shared tree but is byte-identical to
  `origin/main`'s (v0.5.0 added it). Not something to carry.

### The ⌂ Root Row Named After A Worktree — 2026-08-25

**Asked:** "for some reason, in a terminal of my project is says I'm on main, but that root row in
worktrees list shows a worktree name and under it there is a main row for a worktree, but when I click it
and open a terminal, it points to a worktree called gentle-narwahl-files. can you double check the logic
around worktrees and the root row to determine why my root row isn't matching my actual branch, and why
somehow a worktree row is labeled as main" — then "yes fix them".

**Did:** Three daemon fixes; the TUI was never at fault (label and id both come from the same
`visible_worktrees()` entry, so a row can't be mislabeled). `add_project`
(`crates/nebula-daemon/src/registry.rs:558`) now roots the project at `git worktree list`'s **first**
entry instead of `rev-parse --show-toplevel`, and derives `is_main` as `entry.path == repo_path`.
`worktree_probe_stamp` (`crates/nebula-daemon/src/lib.rs:221`) goes through the new `git_common_dir`,
which follows a `gitdir:` file and its `commondir` hop. `reconcile_project_worktrees`
(`registry.rs:~1010`) re-derives root-ness every pass from `entries.first()` via the new
`Store::set_worktree_main`, and its delete pass dropped the `w.is_main ||` reprieve. 6 new tests
(4 in `registry::tests`, 2 in the new `nebula_daemon::probe_tests`); workspace suite 621 green.

**Gotchas:**
- **`git rev-parse --show-toplevel` inside a linked worktree returns the worktree, not the repo.** So
  `nebula add .` from a worktree made the *worktree* the project: named `gentle-narwahl-files`,
  `repo_path` pointing at it, and a ⌂ root row for `…/repo` — a directory the project didn't own.
  `git worktree list --porcelain` always puts the main checkout first (verified: main first, then linked
  ones sorted by path), so it is the cheaper and more reliable root oracle, and it's already being called
  two lines later.
- **A probe that can't read anything is not a fingerprint.** `worktree_probe_stamp` did
  `repo_path.join(".git").join("HEAD")`; in a worktree `.git` is a *file*, so every stamp was `None`,
  `None == None`, and the project **never synced again after boot**. That alone is the "root row isn't
  matching my actual branch" half — confirmed live: root on `feature-x`, row still saying `main`
  indefinitely, while a normally-rooted control picked up its new branch within one 2s tick. The sync loop
  now refuses to cache a `None` stamp.
- **`is_main` was written once at insert and never updated** — no `UPDATE` of it existed anywhere. Nothing
  could repair a project seeded with the badge on the wrong row.
- **git will happily swap a root and a worktree's branches**, so this can also be *reality* faithfully
  reported, not a nebula bug: `git switch --ignore-other-worktrees <wt-branch>` in the root succeeds
  (plain `checkout` refuses), which frees `main` for the worktree to take. Check `git worktree list` before
  blaming the row.
- **The pre-fix breakage self-heals but only halfway.** Forged the old state in sqlite and restarted: the
  ⌂ root badge moves back onto the repo's checkout and the branch goes live again, but `repo_path` and the
  project `name` still point at the worktree (deliberately not migrated — it would silently repoint a
  project, and could collide with the repo added separately in the same workspace). Remove + re-add is the
  remedy for those two fields.
- `crates/nebula-tui/*` was already dirty with another session's in-flight work when this started; the
  whole change is confined to `nebula-daemon`. See [Shared Working Tree Is Raced By Other Sessions].

### A Workspaces Column Left Of Projects, Toggled With Shift+W — 2026-08-25

**Asked:** "add the ability to show a "workspaces" column to the left of projects which acts similar as
projects, basically we should be able to see from a top level which workspaces are running something, add a
hotkey of capital W shift + w to toggle that entire panel away or not. also clicking on the workspace in the
bottom bar should show the workspace select modal"

**Did:** New `Focus::Workspaces` (first variant) + `App::show_workspaces` (default shown; it was persisted
in `UiState.show_workspaces: Option<bool>` at the time — that field is **gone**, see [The Workspaces
Column Remembers Itself] below, which moved it to the `show_workspaces` config key). `ui.rs::draw_workspaces` renders every
`tree.workspaces` row as a 3-row project-style button with `app::workspace_rollup` (all unarchived agents
under the workspace's projects, folded by `rollup`) plus a warn-colored running count
(`workspace_running`); the open workspace is the selected row. The cursor IS the active workspace:
`move_selection` and left-click call `switch_workspace`, Enter steps into Projects, `n`/`r`/`d`/`m` map to
`PromptKind::NewWorkspace` / `RenameWorkspace` / `remove_workspace` / `workspace_menu` (three new
`MenuAction`s). `Action::ToggleWorkspaces` (`shift+w`, keymap.rs) flips the column and parks a cursor
in it on Projects. The column shipped fixed at 18 columns; `splitter_x` / `set_splitter` /
`normalize_panel_widths` all carry the offset via `App::workspaces_panel_w()`. (Superseded — it is
draggable now, see [The Workspaces Column Drags To Resize].) Footer: `draw_footer` is now a wrapper over
`draw_footer_bar(&App) -> Option<Rect>` that registers `HitTarget::FooterWorkspace` on the `◇ name`
span; left-click opens `open_workspace_picker`. 8 unit tests + e2e updated; README keymap rows added.

**Gotchas:**
- **Tab from the terminal pane now wraps to Workspaces, not Projects** (`App::leftmost_focus`). e2e
  `tui_projects_worktrees_agents_navigation` timed out on `FOOTER_PROJECTS` after the fourth Tab — the
  fix is a fifth stop (`FOOTER_WORKSPACES = "w: switcher"`) in the walk. Any future e2e that Tab-wraps
  needs the same.
- **Every 100-col draw test compresses when the column is shown**: budget = 100 − 18 − 20, so Sessions
  drops to 20 and the terminal pane truncates its own text. Six existing tests (`seed_splitters` and the
  five `TestBackend::new(100, 30)` draw tests that assert positions or pane text) now set
  `app.show_workspaces = false`; test the column at 140 cols.
- `render_button` lifts a `dim` span to `muted` on the selected row, so asserting a fresh dot's color on
  the active workspace row must expect `theme.muted`, not `theme.dim`.
- `seed_tree` points its project at `WorkspaceId::default()` but never upserts the 'default' Workspace
  entity — the footer's "default" is `active_workspace_name`'s fallback. A column test needs
  `seed_default_workspace` or the list shows only `seed_other_workspace`'s row with nothing selected.
- The auto-mode classifier blocked `git checkout origin/main -- <files>` to un-stale the shared tree's
  `event_loop.rs`/`keymap.rs`/`e2e_tui.rs` (they still carry the pre-#12 `h`/`l` hunks and the macOS-only
  clipboard), so this feature was built on the working tree as it stood. Expect those stale hunks to
  show up when rebasing onto `origin/main`; they are not part of this work.

### Release v0.6.0 — 2026-08-25

**Asked:** "commit and push everything, then do another release" (the auto-selected project below).

**Did:** Followed `.claude/skills/release/SKILL.md` in a private worktree: `1e58372` (feature), `d26ac07`
(memory), `c920b72` (bump to 0.6.0, tagged `v0.6.0`). All 4 matrix targets built, notes rewritten.

**Gotchas:**
- **"Everything" in the shared tree was mostly already on `origin/main`.** `ui.rs` and the screenshot were
  byte-identical to `origin/main` (released in v0.5.0), while `event_loop.rs` and `e2e_tui.rs` differed
  by 12 + 3 hunks that were *older* than origin — the pre-#12 `Shift+H`/`Shift+L` presses and a
  macOS-only clipboard — not anyone's in-flight work. Diff each file against the **worktree's** copy, not
  against local `main`, and only ever carry hunks you can name. A 30-line python hunk picker
  (split `diff -u` on `@@`, keep by index, `patch -p0`) plus the residual-hunk count check
  (18 total − 6 kept = 12 left) is the whole verification.
- ~~Local `main` is three releases behind; the cleanup is `git checkout -- .` then
  `git pull --ff-only`.~~ **Superseded 2026-08-25 (v0.8.0):** the shared tree has since been
  reconciled and local `main` tracks `origin/main` again. Never run `git checkout -- .` there on the
  strength of this note — check whether the dirty files are actually merged content first, because
  after a release they are usually the *next* release's work.

### A Freshly Added Project Selects Itself — 2026-08-25

**Asked:** "when I open / make a new project, it should auto focus it after creating"

**Did:** Both `ClientRequest::AddProject` sites in `crates/nebula-tui/src/event_loop.rs` (the prompt
submit and the `PendingAction::CreateProjectDir` confirm) now allocate
`PendingIntent::SelectCreatedProject` instead of `None`. The Ack arm calls the new
`select_created_project` (= `select_project_row_by_id` + `restore_context` + `Focus::Worktrees`, the
same landing a `/` palette project pick does), stashing into the new `App::select_project_when_seen`
when the upsert hasn't arrived, and the `EntityUpserted` arm drains that stash — the exact
`select_worktree_when_seen` idiom. Unit test `add_project_ack_selects_the_new_project` covers both
orderings; e2e `tui_projects_worktrees_agents_navigation` asserts `beta-proj` is the selected row after
adding it. Workspace suite 601 green.

**Gotchas:**
- **The e2e helpers assume panel stability, so an auto-focus change breaks tests that never mention
  focus.** Three `e2e_tui` tests timed out because the `\r` / `e` / `n` they press right after
  `add_project` now landed in the Worktrees panel. Fix was the `create_worktree` precedent: the
  `add_project` helper itself waits for `FOOTER_WORKTREES`, sends `←`, and waits for
  `FOOTER_PROJECTS`. Any future auto-focus needs the same hop in its helper.
- **A green e2e is not evidence after a focus change.** `tui_pull_request_row_leads_the_links_group`
  *passed* against the new behavior even though its `\r` + `wait_for_text(FOOTER_WORKTREES)` no longer
  matched the real flow — the wait was satisfied by the frame before the keypress landed. In a real
  (non-`cfg(test)`) build that stale-frame pass would have carried on to press Enter on a PR link row.
  Check every test that follows the changed step, not just the ones that went red.
- rustfmt reflows a `//` comment placed on the line directly after a trailing `// …` comment into a
  continuation of it (indents it to that column). Put a blank line between them.

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
  live, so walking into the OPEN PRS group and back never churns detach/attach. (It was modeled on the
  project-divider branch that sat above it until dividers were removed on 2026-08-25; it is now the only
  early return there.) Do not try to "clear" the terminal for this.
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
  `move_selection` and the left-click handler. The Sessions panel does go empty there; that's deliberate
  (it followed the project-divider behavior, since removed on 2026-08-25), not a bug.
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
- **`claude --cloud=<task>` creates and exits on this account** (verified 2026-08-26, claude 2.1.247): it
  prints `Created cloud session: … / View: https://claude.ai/code/session_… / Resume with: claude --teleport
  session_…` and returns, so the nebula PTY row goes dead. Both "stay attached after create" and
  `claude --cloud <session_id|url>` (live two-way attach) are gated in the binary on the server feature
  flag `tengu_remote_backend`; `claude --cloud session_…` here fails with
  `Error: Attaching to an existing cloud session is not enabled for your account.` Nothing nebula passes
  can unlock it — when the flag lands, the existing spawn becomes a live attached terminal unchanged.
- What works today without a browser: `claude --teleport session_…` (pulls transcript + branch into a
  local session — a snapshot/fork, not a live stream; it refuses a dirty tree with a "Stash changes and
  continue?" prompt, so run it in a fresh worktree) and `claude -p "msg" --cloud session_…` (queue a
  message, no reply). `claude agents --json --all` lists only local background/interactive sessions,
  never cloud ones, so there is no CLI poll for "cloud session finished". The reattach path built on this
  is [Cloud Rows Re-Enter Their Session On Restart]. Teleport was later confirmed **repeatable** — it
  re-pulls newer turns, is idempotent in the same worktree, and leaves the cloud session running — which
  is what [Cloud Rows Mirror Their Session Instead Of Dying At Create] is built on.

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
  notes, we should need all these sub menus"). Follow that pattern for any new modal — since notes were
  removed 2026-08-26 the surviving exemplar is the **hosts picker** (`ui.rs` `Overlay::Hosts`, ~1504,
  hint on `title_bottom` at ~1527).

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
- charmbracelet freeze wrecks the cell grid — use a small pillow grid renderer instead. (This bullet used
  to also say Chrome headless gets SIGKILLed on this Mac; that is **no longer true** — see
  [Browser Terminal Stopped ~24 Columns Short], which drives `/Applications/Google Chrome.app` with
  `--headless=new` over CDP without trouble.)

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
**Superseded 2026-08-25:** dividers were removed entirely (see [Project Dividers Removed From The Projects
Column]); Shift+J/K reordering stays, as a plain move within the workspace's list.

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
