# The RECALL HOOK Gets An Eval, Text Search Over Entries, Ranked STANDING GOTCHAS And A PROMPT DADDY Re-Run — 2026-08-29

**Asked:** "how might you recomend improving this memory term recall setup?" → "do all start with eval if you think we need it"
→ refined: Do all five RECALL HOOK improvements you recommended, eval first: (1) check in a recall eval — a
fixture of real (prompt → expected entry) pairs plus a `make recall-eval` target wired beside MEMORY CHECK
in MAKE CI — so every later change is measured; (2) rank the STANDING GOTCHAS by relevance, weight aliases
by specificity, and cap lines per TERM; (3) search the entries' own `Asked`/`Did` text, not only the index
cells; (4) have PROMPT DADDY re-run `recall.py` on the refined prompt; (5) skip task-notification prompts,
widen `TERM_ROW` (and `index_line.py`), strip annotated Alias-index targets, and prune `gotchas.md` under
its cap (assuming: no new deps, python3 stdlib only, MAKE CI stays green; the eval fails CI only on a
regression against a checked-in baseline).

**Did:**
- **RECALL EVAL** (`.claude/memory/recall_eval.py`, `make recall-eval`, first step of MAKE CI after MEMORY
  CHECK): self-recall — every entry's verbatim `**Asked:**` prompt through `recall.rank()` with that entry's
  own Asked text masked (leave-one-out, so text search cannot just find the quote), rank top-1 / top-5 /
  missed — plus `recall_eval.json`: 14 curated (prompt → entry glob) pairs and 5 chatter prompts that must
  inject nothing. Gates top-5 and curated against `recall_baseline.json`; `--update` after a deliberate
  change, `--verbose` lists misses, `--entry <file>` ranks one fresh entry (now a step in the NEBULA-MEMORY
  SKILL), `--hook <script>` scores an old copy by stdout. Old hook: top-1 50 / top-5 77 of 103. New: 69 / 91,
  curated 19/19.
- **`recall.py`** is now a module: `rank(prompt, root, mask_asked)` → (hit, paths, scored, standing) and
  `render()`; the hook, `recall.py "<prompt>"` and `recall.py --diff "<raw>" "<refined>"` (prints only what
  the refined prompt adds) share it. Scoring: TERM-name hit 2, alias hit = specificity (multi-word or ≥8
  chars 1, else 0.5) ÷ TERMS sharing it; text overlap = Σ idf² of prompt words in the entry's Asked+Did
  (`BODY_WEIGHT` 0.12); entries below `MIN_ENTRY_SCORE` 2.0 dropped; STANDING GOTCHAS scored the same way,
  best first, `MAX_PER_TERM` 4 of the 15, `MIN_STANDING_SCORE` 1.0. `usable()` skips prompts starting `<`
  (`<task-notification>`, `<command-name>`, `<local-command-*>`). `TERM_ROW` is `[A-Z0-9][^|*]*?` in both
  `recall.py` and `index_line.py` (underscore / backtick TERMS load, 291 names); `clean_term` cuts an
  Alias-index target at ` (` / ` —`; `ALIAS_STOP` drops "this"/"that"-type aliases.
- **PROMPT DADDY** step 5 runs `recall.py --diff` on the rewrite; **gotchas.md** 300 → 291: eight
  RELEASE SKILL / SHARED CHECKOUT near-duplicates merged (sources kept), the RECALL HOOK `TERM_ROW` line
  retired (its `retire:` was this change). TERMS.md: "gotchyas" aliased to STANDING GOTCHAS.
- Rejected: weighting short TERM names (NEW, PIN, TUI) below 2 — no change on the eval, so not added.

**Gotchas:**
- The self-recall metric leaks unless the entry's own Asked text is masked: with text search on, an
  unmasked eval scores ~100% top-1 because the prompt *is* in the file. `rank(mask_asked=path)` exists
  only for this.
- Chatter is the cost of text search: "sounds good, go ahead with the second option" scores the DONE
  SOUND entry 3.0 on the word "sounds" (idf 4.2), "nice work" hits an entry on "nice". `MIN_ENTRY_SCORE`
  and a conversational `STOP` list hold it at 0 on the five fixture prompts; a plain-idf overlap scored
  worse (top-5 91 vs 93) and idf² without the floor let 3 entries through.
- The old hook read its prompt only from stdin, so a subprocess eval that passes argv scores it 0/103;
  and a zsh `echo '…\n…'` expands `\n` inside the JSON so the hook falls back to raw text starting `{` —
  generate test JSON with `python3 -c 'import json…'`.
- `share` counts matter more than weights: "green" names six status TERMS, "done" three — a raw alias hit
  on any of them was worth as much as the TERM's own name before.
- Importing `recall.py` from another script drops `__pycache__/` beside the hook; `__pycache__` is not in
  `.gitignore`, so `sys.dont_write_bytecode = True` before the import.
