# Workspaces And The o/t/e Hotkey Remap — 2026-08-21

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
