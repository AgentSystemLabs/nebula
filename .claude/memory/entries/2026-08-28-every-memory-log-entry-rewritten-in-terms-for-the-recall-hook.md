# Every MEMORY LOG Entry Rewritten In TERMS So The RECALL HOOK Reaches The Old Ones — 2026-08-28

**Asked:** "go through all memories and try to rewrite as minimally as possible to use our list of terms so
recall can more successfully use older memories"
→ refined: Go through all 128 MEMORY LOG entry files (and their index lines in `.claude/MEMORY.md`, plus
the bold **TERM** keys in `.claude/memory/gotchas.md`) and rewrite them as minimally as possible in the
TERMS from `TERMS.md` — swap an alias for its ALL-CAPS TERM where the entry means that thing, put the TERMS
the entry is about in each index line's `TERMS:` cell, and leave everything else alone (assuming: the
**Asked** line stays verbatim, code identifiers and quoted error text are untouched, titles only get an
existing name capitalised, no entry is merged or deleted). Measure retrieval before and after; don't commit.

**Did:**
- **Bodies.** Four subagents, 32 entries each, swapped aliases for TERMS in titles, Did and Gotchas: 561
  edits across 115 of 127 files (the 2026-08-14 agent-kinds entry was skipped — another session had it
  open). Entries with zero caps TERMS in the body: 90 → 2. No `**Asked:**` / `→ refined:` line changed
  (checked with `git diff -U0 | grep '^[-+]\*\*Asked'`), every title still parses for `index_line.py`.
- **Index cells.** Each agent named up to six TERMS the entry is *about* (most specific first); those
  replaced the `TERMS:` cells (126 lines), then each cell was unioned with the TERMS the entry's own
  **Asked** prompt maps to via the Alias index (rarest first, real TERM rows only, multi-word or ≥8-char
  aliases only), and for the nine entries that still lost rank, the generic TERMS their prompt hits were
  restored from the old cell. Cap of six kept.
- **Measurement** (scratchpad `selfrecall.py`: run `recall.py` on every entry's own Asked prompt, is the
  entry in the top 5?): before 59 top-1 / 85 top-5 / 43 unretrieved; after 60 / 96 / 32 — 12 gained, 1
  lost. 14 of the 32 unretrieved are the 16 release entries competing for five slots on "commit push
  release", which no cell change fixes.
- **Standing gotchas** untouched: every bold key was already a TERM (two are keyed by `ui.rs`, which the
  RECALL HOOK matches as a path). Gate: `make memory-check` ok.

**Gotchas:**
- `recall.py` and `index_line.py` share a `TERM_ROW` regex (`[A-Z0-9][A-Z0-9 ./+'-]*?`) that skips any
  TERM with an underscore or a backtick — NEBULA_AGENT_CMD, NEBULA_RUNTIME_DIR / NEBULA_DATA_DIR and
  OLD `h` / `l` / `o` / `t` BINDINGS are never loaded, so they can neither match a prompt nor sit in a cell
  (the agents proposed them; the applier had to drop them).
- The RECALL HOOK runs on the **raw** prompt, before PROMPT DADDY — a cell holding only specific TERMS
  (IPC CODEC, HARDWIRED UNLOCK) loses the prompts typed in generic words; the first specific-only pass
  dropped top-1 from 59 to 52. Cells need the alias-reachable TERMS too, and alias coverage in TERMS.md
  is the real lever.
- `load_terms` takes the Alias index's target cell verbatim, so annotated targets come back as pseudo-TERMS
  ("FOCUS RIGHT (one action each — the arrows follow)", "NEW LINK (retired)", "WORKTREE OPEN PRS GROUP —
  settle by …"), and one-word aliases (new, fresh, prompt, feedback) match ordinary prose: deriving cells
  from prompt words without a real-row + multi-word filter overfits (a junk pass reached 113 top-5).
- The RECALL HOOK fires on background-task notifications too: each subagent's completion report, full of
  TERM names, drew an 8 KB `[nebula recall]` injection listing ~100 TERMS — four times in this task.
