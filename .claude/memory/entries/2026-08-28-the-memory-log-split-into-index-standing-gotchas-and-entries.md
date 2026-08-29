# The MEMORY LOG Split Into Index, Standing Gotchas And Entries — 2026-08-28

**Asked:** "is our custom shared memory system seem useful? is it possible to verify it's improving my ai
harnesses output?" → "what is a better approach to this memory system?" → "implement this"
→ refined: Given the assessment — a 69k-token MEMORY LOG every SESSION reads but cannot reach the tail of
(Read caps at 2,000 lines), merge conflicts on one shared file, four skill loads on a one-line question,
no measurement — build the recommended shape: split the MEMORY LOG into an index, standing gotchas and
per-entry files; inject the relevant entries per prompt with a `UserPromptSubmit` hook; enforce the size
caps in `make ci`; turn the recurring blockable gotchas into a `PreToolUse` hook; give the NEBULA-MEMORY
SKILL the re-hit and corrections counters. Keep TERMS.md, PROMPT DADDY and OUTPUT DOCTOR as they are
except for the pure-question short form.

**Did:**
- **Layout.** `.claude/MEMORY.md` is the index: one line per entry (`- DATE · [Title](memory/entries/…)
  · TERMS: … · files: … · gotchas: N`), newest first. `.claude/memory/entries/<date>-<slug>.md` hold the
  116 entries verbatim — 113 split out of the monolith by a scratchpad script plus two a concurrent
  session wrote mid-migration and this one. `.claude/memory/gotchas.md` holds the standing gotchas:
  three subagents condensed the monolith's 428 bullets a slice each (12 dropped as already enforced by a
  named test), merged with the nine lines a concurrent session had started, regrouped by TERMS.md
  section with the TERM inline (96 per-TERM groups cost as many header lines as gotchas), three
  cross-slice twins pruned. `.claude/memory/archive.md` receives index lines past the cap.
- **RECALL HOOK** `.claude/hooks/recall.py`, registered in the new `.claude/settings.json`: maps the
  prompt's words onto TERMS (TERM names + the Alias index) and file / symbol tokens, scores index lines
  with rarity weighting (`2 / log(2 + df)` per TERM hit, +2 per file hit, +1 per title hit), prints the
  top 5 entries' Gotchas (≤1,600 chars each) and ≤15 matching standing-gotcha lines under
  `[nebula recall]`, ≤7,000 chars; silent for prompts under 12 chars or starting with `/`; any error
  exits 0.
- **GUARD HOOK** `.claude/hooks/guard.py` (`PreToolUse`, matcher `Bash`): blocks a backtick inside a
  double-quoted `git commit -m` message, `cargo install --path`, and `cp` / `>` onto
  `~/.cargo/bin/nebula`; heredoc bodies are stripped before matching; exit 2 with the reason on stderr.
  12/12 cases pass (`python3 - <<'PY' …` harness in the transcript).
- **MEMORY CHECK** `.claude/memory/check.py` → `make memory-check`, first step of `make ci`: index ≤200
  lines, gotchas ≤300, every index link resolves, every entry file is indexed, no entry body in the
  index. `.claude/memory/index_line.py <entry>` prints a correctly shaped index line (TERMS by caps
  count, aliases as fallback; files by mention count; gotcha count).
- **Skills and rules.** NEBULA-MEMORY SKILL rewritten for the three layers (entry file → index line →
  standing-gotcha promotion, `· re-hit ×N date`, `· retire: <test or hook>`, `**Corrections:** N`);
  PROMPT DADDY skips a pure question; OUTPUT DOCTOR gained the two-section question form; `CLAUDE.md`,
  `AGENTS.md`, `.cursor/rules/nebula-memory.mdc`, project-terms and the RELEASE SKILL point at the new
  files. `TERMS.md`: MEMORY LOG row rewritten; RECALL HOOK, GUARD HOOK, STANDING GOTCHAS, MEMORY CHECK
  ledgered as candidates.
- **Not done:** the replay ablation from the assessment (headless `claude -p` over past entries with and
  without the memory) — it was recommended as the measurement, not part of "implement this"; and no
  gotcha was turned into a regression test, only into GUARD HOOK rules. Nothing committed: the SHARED
  CHECKOUT carries other sessions' hunks.
- **Gate:** `make memory-check` ok; guard 12/12; RECALL HOOK checked on four prompts (STOP GATE,
  WORKSPACES BAR, `e2e_pty.rs`, "yes do it" → silent). No Rust changed, so no cargo run.

**Gotchas:**
- A project `.claude/settings.json` hook is live in every running session the moment the file exists —
  no restart, no reload. The GUARD HOOK's first version blocked the very heredoc that was editing it
  (it scanned the whole command for `git commit -m` plus a backtick anywhere); rules must match the
  actual argument and strip heredoc bodies, or any file that *mentions* the pattern is unwritable.
- A rewritten SKILL.md is live for concurrent sessions just as fast: two sessions wrote entry files and a
  `gotchas.md` in the new format before the index existed. `ls` the target of a migration before
  writing generated output, and fold in what is there.
- The monolith kept growing under the migration: an entry prepended between the raw gotcha extraction
  and the split shifted every `[i]` by one, so the standing-gotcha pointers named the wrong entries until
  remapped through the extraction file's own headers. Snapshot the source once and derive everything
  from the snapshot.
- Read caps at 2,000 lines — the 3,529-line monolith was never fully readable by an agent; and
  `grep -c '^- \*\*'` said 153 gotchas where there were 428 (only the bold-led bullets).
- 51 of the 112 backfilled entries name zero TERMS in caps (they predate the glossary); the index
  derives their TERMS cell from Alias-index phrases in the body so the RECALL HOOK still finds them.
- A session still holding the old skill will prepend a `###` entry to `.claude/MEMORY.md`;
  `make memory-check` flags it ("holds an entry body") — move it into an entry file.
