# The SESSIONS PANEL Shows OPEN PRS And Cannot Create LINKS — 2026-08-28

**Asked:** "instead of it saying \"Links\" in the sessions list, just have it says OPEN PRS, and remove
the ability for a user to even add links manually for now"

**Did:** `crates/nebula-tui/src/ui.rs::draw_sessions` now titles the selected WORKTREE's group `OPEN
PRS`. Removed the TUI's full NEW LINK creation chain: `Action::NewLink` / HOTKEYS TAB row, HELP OVERLAY
and FOOTER hints, keyboard and mouse CONTEXT MENU rows, `MenuAction::NewLink`, `PromptKind::NewLink`,
submit dispatch, and the select-created-LINK PENDING INTENT/state. The DAEMON protocol/store and
existing LINK rows stay intact; old rows still open/edit/delete, while the PR ROW only opens and its
FOOTER no longer advertises edit/delete. README, ARCHITECTURE and focused unit/E2E assertions updated.
`cargo check -p nebula-tui --lib`, all 471 `nebula-tui` unit tests, and both focused E2E TUI tests
(`tui_manual_link_add_is_unavailable`, `tui_pull_request_row_leads_the_open_prs_group`) passed.

**Gotchas:**
- Two surfaces now render `OPEN PRS`: the repo-wide group in the WORKTREES PANEL and the branch-local
  group in the SESSIONS PANEL. TERMS calls them PROJECT OPEN PRS GROUP and WORKTREE OPEN PRS GROUP.
- An E2E absence check can pass before a key is processed. The NEW LINK regression sends `Shift+L`,
  then `?`, and waits for HELP OVERLAY's unique `NAVIGATE & SEARCH` heading; if the prompt still opened,
  `?` would type into it and the test would time out.
- The SHARED CHECKOUT moved during the first verification pass and temporarily produced unrelated
  PR-agent compile errors (`pr_url` / `CreatePrAgent` / `validate_pr_url` mismatches). That work settled
  and the final suites passed; do not misattribute the transient errors to the LINK removal if they recur.
