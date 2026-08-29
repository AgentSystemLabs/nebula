# NEW SESSION PICKER Drops Its Terminal Row — 2026-08-28

**Asked:** "remove terminal as n option in the new session modal because we already have hotkey t to
open a terminal" → refined: "Drop the **Terminal (shell)** row from the NEW SESSION PICKER, leaving it
Claude / Codex / Cursor, because NEW TERMINAL (`t`) already opens a TERMINAL SESSION. Keep NEW TERMINAL
and the CONTEXT MENU's **New terminal** item exactly as they are."

**Did:** `open_new_agent_picker` (`crates/nebula-tui/src/event_loop.rs`, ~2525) builds three
`kind_row`s and no `MenuAction::NewTerminal` item; doc comment says why. Decision: the picker is for
AGENT kinds only — a shell has `t` and the CONTEXT MENU's **New terminal** (`MenuAction::NewTerminal`
still exists for those). README step 4 and the TERMS.md row updated. 489 nebula-tui unit tests green.

**Gotchas:**
- Two tests hard-coded the row: `n_in_sessions_opens_agent_type_picker_then_prompt` (len 4, `items[3]`
  label) and `picker_right_drills_into_model_then_effort_submenus` (`items[3].action.submenu()`), which
  panicked with `index out of bounds: the len is 3`. The other `items[3]` asserts in that file are on
  the MODEL / EFFORT submenus, not the picker — leave them.
