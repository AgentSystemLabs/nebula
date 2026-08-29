# Prototype: The Workspaces Column Became A Top Tab Bar — 2026-08-26

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
