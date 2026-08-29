# Rebindable Hotkeys And Settings Tabs — 2026-08-24

**Asked:** "in the settings add a top tabs which a user can use arrows or tabs to navigate though.
challenge my prompt, pick the best user experience. make good tab categories for where to put settings.
now I need you to add in a setting for hotkeys, allow a user to customize ANY HOTKEY in the application…"

**Did:** New `crates/nebula-tui/src/keymap.rs` holds the KEYMAP (the rebindable key table); the SETTINGS OVERLAY grew
tabs. Landed in `87d2b24` alongside the cancel-status fix.

**Gotchas:**
- The user explicitly invited pushback ("challenge my prompt") — this is a standing preference on UX
  asks, not a one-off.
