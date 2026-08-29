# KEEP MODULES SMALL: A Split-What-You-Touch Rule In CLAUDE.md And AGENTS.md — 2026-08-28

**Asked:** "update the claude and agents to instruct it to try and split up larger files, classes,
functions, into smaller modules when it makes sense. too long of files is a refactoring smell"
→ refined: Add a section to `CLAUDE.md` and `AGENTS.md` telling agents to keep modules small: when a
file, `impl` block/struct, or function has grown long, split it into smaller modules, types or functions
when that makes sense (assuming: applies to code the task already touches, behavior-preserving and tested
first, no numeric line limit).

**Did:** Appended a `## Keep modules small` section to both `CLAUDE.md` and `AGENTS.md` (identical text):
split what you touch, no line limit (judgment + the smell), extraction is a refactor so test first and
keep it behavior-preserving, and stay in your lane — no drive-by moves of files the task does not touch,
because the SHARED CHECKOUT has other sessions mid-edit. Cited the current sizes as the motivation:
`event_loop.rs` 20k lines, `ui.rs` / `registry.rs` ~4.6k–4.9k (`find crates -name '*.rs' | xargs wc -l`).
`.cursor/rules/*.mdc` were left alone — they mirror only the memory-log and title rules, not the
whole protocol.
