# TERMS.md: One ALL-CAPS Name Per Thing, Kept By `project-terms` — 2026-08-27

**Asked:** "analyze my projetc and also previous chat history in memory.md, I want you to create a TERMS.md
which has a descriptive name for various features, routes, short cuts, naming to help describe the project
better between tean mates. update claude.md to call a project-terms skill which will update that TERMS.md
with useful new terms after every prompt like we do for nebula-memory. update the claude.md to load and
always speak in those terms. update the prompt-daddy to use those terms when rewrighting your prompt"
→ prompt-daddy: no option picked; the note said *"make sure terms are always all caps"*.

**Did:** New `TERMS.md` at the repo root — 254 TERMS in 13 sections (the tree, plumbing, layout, status,
focus/keys, overlays, agents/hooks, daemon mechanisms, cloud, CLI, config/env, dev workflow, retired) plus
an **Alias index** mapping the user's words to TERMS; every row is `TERM | meaning | "aliases" | where`,
with the aliases quoted from the **Asked:** lines of this log and the *where* symbols verified by grep.
Built from two parallel sweeps (this log lines 705–2425 for vocabulary; the code for keymap actions,
overlays, CLI, routes, env vars, Makefile) plus README/ARCHITECTURE. New
`.claude/skills/project-terms/SKILL.md` (runs after `nebula-memory` on every task; edits only for a new,
renamed, retired, or newly-aliased TERM — *superseded 2026-08-28*: new names now wait in the Candidates
ledger until a separate task uses them again, see "Project Terms: Detect Every Session, Promote Only
What Recurred"). `CLAUDE.md` rewritten: read `TERMS.md` with the memory log, a
"Speak in the project's terms" section (ALL CAPS in replies, `AskUserQuestion` options, commits, memory
entries; alias → TERM on first use; never rename code to match), and the `project-terms` call after
`nebula-memory`. `prompt-daddy` gained a TERMS row in its failure table, a TERMS lookup in the grounding
step, an "in the project's TERMS" rewrite rule, and a worked example in caps. `AGENTS.md` mirrors the
protocol for codex/cursor; the `release` skill's scaffolding-commit list now includes `TERMS.md`.

**Gotchas:**
- **The memory-mining subagent stopped without reporting once and had to be resumed with `SendMessage`**;
  its result then arrived normally. Don't assume a "stopped" notification means lost work.
- Two sweeps disagreed on facts — VENDORED VT100 is **0.16.2** (`vendor/vt100/Cargo.toml`), not 0.15.2 as
  an old entry says; there is no `resolve_editor` fn (it is `Config::editor` + `NEBULA_EDITOR`). Grep every
  *where* cell before writing it; the log's own symbols drift.
- The Retired table has three columns, not four — a row-regex that assumes the four-column shape silently
  skips it. Anchor on the `| **TERM** |` prefix when editing rows programmatically.
- `nameplate` meant two things in README/log: the `nebula vX.Y.Z` (VERSION NAMEPLATE) and the `◇ workspace`
  chip (WORKSPACE NAMEPLATE). Likewise "locked layer" ≠ LOCKED PANE (it is the WALK EDGE) and "done" splits
  UNSEEN vs FINISHED — all three are listed under both TERMS in the Alias index on purpose.
- An unquoted note inside an alias cell (`"sub agents" (mistaken)`) breaks a `", "`-split merge; keep
  notes inside the quotes.
