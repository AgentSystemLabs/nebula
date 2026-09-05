# The AGENTS TAB Groups Its Rows Under Quick Prompt / Claude / Codex / Cursor Headers — 2026-09-04

**Asked:** "reorgamize the agent settings panel by grouping based on related harness"
→ refined: On the SETTINGS OVERLAY's AGENTS TAB (the "agent settings panel"), group the rows per AGENT
KIND (your "harness") the way the HOTKEYS TAB already groups its rows: a dim header row per group with a
blank line between groups, headers not selectable, `j`/`k` and mouse clicks skipping them. Groups, top to
bottom: the two cross-kind QUICK PROMPT rows first (assuming under a "Quick prompt" header), then Claude,
Codex, Cursor, each holding its HARNESS TOGGLE, MODEL and EFFORT rows in that order. Since the header now
names the harness, shorten the row labels to `Enabled` / `Model` / `Effort` (assumption); keep every
hint, every SETTING key, the other four tabs, `locate`, and the SELECTION MEMORY exactly as they are.
(no questions asked)

**Did:** `crates/nebula-tui/src/config.rs`: `SettingSpec` gained `group: &'static str` (the twin of
`keymap::ActionSpec::group`; `""` = a bare row, and a tab whose rows all say `""` stays the flat list it
was), every row in `SETTINGS_TABS` names one, and the AGENTS TAB rows sit under `Quick prompt` (`Agent`,
`Focus`) and `Claude` / `Codex` / `Cursor` (`Enabled`, `Model`, `Effort`). `settings_rows` feeds both tab
bodies through one `grouped(groups, row)` layout: a `Header` when the group name changes, a `Blank` before
every header but the first, nothing for an empty group. Renderer, mouse hit-test, `j`/`k`, `locate`, the
hints and the CONFIG.JSON keys are untouched — `ui.rs` already drew `Header` / `Blank` rows for the
HOTKEYS TAB and `event_loop.rs`'s click path already skipped them. Tests: config
`agents_tab_groups_its_rows_per_harness` (headers and labels in order, each header names its rows' kind,
one blank per gap) and `tabs_cover_every_setting_once_and_rows_match` relaxed to "headers iff the rows
name groups"; event_loop `agents_tab_renders_its_harness_groups` (100×40 TestBackend screen walk, two `j`
from the first row land on `ClaudeEnabled`, a click on the `Codex` header changes nothing). `docs/keys.md`
(`p` row) and `docs/configuration.md` name the new labels; TERMS AGENTS TAB / QUICK PROMPT FOCUS rows
updated. Gate: the SHARED CHECKOUT went red mid-task on another session's PROTOCOL VERSION 35
(`CreatePrAgent { kind }` missing at `event_loop.rs:5522`), so the gate ran in a scratch copy —
`git archive HEAD | tar -x` into the scratchpad, `config.rs` copied whole (its whole diff was mine), the
new test spliced into HEAD's `event_loop.rs` by regex, `CARGO_TARGET_DIR` inside the scratchpad so the
SHARED CHECKOUT's `target/` was never touched: nebula-tui 572 passed, clippy and `cargo fmt -p
nebula-tui --check` clean (`--all --check` fails on the other session's `e2e_pty.rs` / `attach.rs`). The
shared tree had passed 576 + clippy with everything but the event_loop test just before it broke. Not
committed.

**Gotchas:**
- The AGENTS TAB is 18 screen rows now (4 headers, 3 blanks, 11 settings): a 30-row TestBackend fits it
  exactly (`height = min(rows + 8, screen − 2)`, body = height − 8) while a stock 24-row terminal shows
  14, so the Cursor section scrolls in under the same follow-window the HOTKEYS TAB uses — draw tests
  over this tab at ≥30 rows or the bottom section is simply off-screen.
- `text.contains("Agent")` also matches the `Agents` title in the tab strip above the body; walk the
  needles in order from `"Quick prompt"` (`text[pos..].find`) rather than independent `contains` checks.
- A SETTING's `label` is quoted verbatim in `docs/keys.md` and in TERMS.md rows ("labelled `Quick prompt
  focus`"), and no gate notices a rename — grep the old label across `docs/` and `TERMS.md` whenever one
  is shortened or reworded.
