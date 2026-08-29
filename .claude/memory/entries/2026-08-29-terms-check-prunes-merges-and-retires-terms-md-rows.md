# `terms_check.py` Reports, Merges, Retires And Prunes TERMS.md Rows; PROJECT TERMS Acts On Its Report — 2026-08-29

**Asked:** "is there something that helps prune down or merge terms in the TERMS.md fil?" → "yeah build … with both retire and merge and tell the terms skill to auto prune or merge when it decides it needs to"

**Did:**
- **`.claude/memory/terms_check.py`** (`make terms-check`, third MAKE CI step). Report: `dead` (TERMS never
  written in caps outside `TERMS.md` — corpus is the MEMORY LOG, `CLAUDE.md`, README, `AGENTS.md`, the
  skills and `//` comments under `crates/`), `once`, `merge?` (A only ever on lines that also say B),
  `collide` (alias on 3+ TERMS), `stale` (`Where` pointer whose file or symbol no longer greps), `dangling`
  (Alias-index target that names no row, duplicate rows), `overdue` (candidate past 30 days),
  `unmentioned` (Retired row nothing under `.claude/memory/` says). CI fails only on `stale` / `dangling`.
  Actions: `--merge A --into B`, `--retire A [--into B]`, `--prune`, `--dry-run`, `--note` — rewrite the
  row (→ Retired with date, "Merged into B", the old `Where`), B's Also-called (A's name + aliases), the
  Alias index, other rows' cross-references (B's own row gets A in lowercase), the MEMORY LOG index cells
  and `gotchas.md` keys; then RECALL EVAL, exit 1 on a drop. Entries are never rewritten.
- **`recall.resolve_targets()`** — one reading of an Alias-index target cell for `recall.py`,
  `index_line.py` and `terms_check.py`: known row names scanned longest-first over the cell's head (before
  ` — `, ` (`, `; `, `: `), so `FOCUS LEFT / FOCUS RIGHT` and `MODEL / EFFORT` resolve to their one row and
  an annotated target ("WALK EDGE — *not* LOCKED PANE") to the TERM it leads with.
- Applied on this run: `--prune` (SECOND PRESS), `--merge METRICS SNAPSHOT --into MEMORY MODAL`,
  `--merge PENDING ACTION --into CONFIRM DIALOG`, three `Where` cells repointed (AGENT ENV →
  `nebula-core/src/env.rs::AGENT_ID`, NEBULA RENAME / NEBULA WORKTREE → `nebula-tui/src/lib.rs`). 263 → 261
  TERMS; RECALL EVAL unchanged (70 / 92, 19/19).
- **PROJECT TERMS skill**: new "Prune and merge with `terms_check.py`" section (a rule per report line),
  step 4 runs it and acts, Size discipline and the retirement bullet point at the tool, description says so.
- Then, at the user's word ("yes merge and retire them"): the other 5 `merge?` pairs merged (GREP VIEW, OPTION
  CLICK and VIM MODAL → FILE FINDER; HANDSHAKE → VERSION SKEW; CHORD → KEYMAP — meanings folded into the
  target rows by hand) and all 23 `dead` TERMS retired `--into` the row that covers them (ACTION ID / KEY
  SCOPE → KEYMAP, DISCONNECTED / TERMINATED / BOOT SWEEP → AGENT STATUS, ARCHIVE → SESSIONS PANEL, ZOOM →
  LOCKED PANE, QUIT → TUI, …; RAW ATTACH plain), so their aliases live on. 263 → 233 TERMS, 45 retired;
  RECALL EVAL unchanged. Gates: MEMORY CHECK, RECALL EVAL, TERMS CHECK ok.

**Gotchas:**
- The Alias index splits targets on ` / `, but nine row names *contain* ` / ` (`FOCUS LEFT / FOCUS RIGHT`,
  `DELETE / DELETE ALL`, the env-var pairs): `recall.py` and `index_line.py` had been attaching those
  aliases to pseudo-TERMS that never sit in an index cell, so they matched nothing. Resolve targets by
  scanning for known names, never by splitting.
- Scanning a whole annotated target cell over-assigns: "`n` in the WORKSPACE SWITCHER or the WORKSPACES BAR;
  both create, and the new one opens with FOCUS on the first visible PANEL" made "new workspace" an alias
  of FOCUS and PANEL. The names the row means come first; scan the head, fall back to the whole cell.
- A `Where` symbol checked against the *first* file of that basename reads as stale when the symbol is in
  the second (`config.rs` exists in three crates, `hooks/mod.rs` in two): check every same-basename file.
- A merge's cross-reference rewrite must skip B's own row or it cites itself ("a yes/no gate before a
  CONFIRM DIALOG"); there the merged name becomes plain lowercase words.
- The example row `**THE NAME**` in the "How to read a row" table parses as a TERM unless rows before
  `## 1.` are ignored — it showed up as a dead TERM in the first report.
