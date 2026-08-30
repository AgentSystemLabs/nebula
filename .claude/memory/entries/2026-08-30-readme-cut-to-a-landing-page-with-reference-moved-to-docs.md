# README Cut To A Landing Page, Reference Moved Verbatim Into `docs/` — 2026-08-30

**Asked:** "analyze some of the best written open source tui tools and rewrite my readme with their
approach(s) you think works best to market to developers who use ai and want the tui experience"
→ refined: "Study the READMEs of the best-written open-source TUI tools (lazygit, zellij, atuin, yazi,
gitui, helix, k9s, jj, television) and rewrite `README.md` using the patterns that work best on
developers who run coding agents and want a terminal-native experience. Cut it to ~140 lines: hook,
demo, install, quickstart, STATUS DOT table, a feature grid, links. Move the full KEYMAP, the `nebula`
commands, CONFIG.JSON and *How it works* verbatim into `docs/`. Keep the ALL-CAPS TERMS. No code
changes." (asked: structure → landing page + `docs/` split; asked: voice in a public README → keep the
ALL-CAPS TERMS, against my recommendation of plain English)

**Did:** `README.md` 439 → 172 lines. New `docs/keys.md`, `docs/commands.md`, `docs/configuration.md`,
`docs/how-it-works.md`, `docs/sessions.md` (470 lines total), built by `sed`-extracting the old line
ranges so the reference prose moved verbatim. New README sections: a pain-point hook, a 10-row feature
table, a tightened 5-step Quickstart, the STATUS DOT table, a "where the status actually comes from"
section (MANAGED HOOKS + PROGRESS SCANNER), and a docs link table. No crate code touched; the one code
edit is `.claude/memory/terms_check.py::corpus_lines`, which now also reads `docs/*.md` (see Gotchas).
Gates: `make terms-check` ok (dead 0, stale 0, dangling 0), `make memory-check` ok, `make recall-eval` ok
(top-5 86%); link and code-fence checkers clean.

**Gotchas:**
- **A README rewrite is a silent-content-loss operation.** Diffing `git show HEAD:README.md` line-by-line
  against README + `docs/*` (normalized whitespace, lines ≥25 chars) caught four whole paragraphs that
  had fallen out rather than moved: the CLOUD MIRROR / CLOUD TELEPORT mechanics with
  `NEBULA_CLOUD_MIRROR_SECS`, AGENT PRESETS' `agent_presets.json`, the PROJECT OPEN PRS group's 15 s
  refresh and draft badging, and the Cursor `cursor_models.json` catalogue. They became `docs/sessions.md`
  — a fifth page nobody planned. Run that check *before* declaring the move lossless.
- Three single-clause facts also vanished and had to be re-placed by hand: `nebula upgrade` refusing to
  clobber a local `cargo build`, RECENCY ORDER's stamping rule (session → worktree → project), and "a turn
  that finishes in the pane you're already looking at never counts."
- **`README.md` is in the TERMS CHECK corpus and `docs/` was not**, so moving reference prose into new
  pages *shrank* the corpus and immediately reported `NEBULA_LOG` as `dead` — a live env var, documented,
  just no longer in a scanned file. Fixed by adding `docs/*.md` to
  `.claude/memory/terms_check.py::corpus_lines` (`dead` 1 → 0). Any future doc that moves prose out of
  `README.md`/`CLAUDE.md`/`AGENTS.md` has to land somewhere that function reads, or the gate goes blind.
- Extracting sections by `sed` line range duplicates any lead-in you also write yourself — `docs/keys.md`
  came out with "Defaults — every one of them is rebindable…" twice, and `## Mouse` followed by "Mouse:".
- What the best TUI READMEs actually converge on, from reading lazygit, atuin, yazi, zellij, television,
  k9s and jj: **≤70 words before the first image** (television ~40, atuin ~68); a 4-8 word concrete
  tagline; lazygit's pain-point rant as the section right after the hero; feature-with-GIF pairs (lazygit
  has 11 GIFs, nebula has 1 screenshot — the biggest remaining gap); install as one command up top with
  the long tail deferred; and reference material pushed to a docs site (yazi defers even installation).
  k9s is the counterexample — ~1000 lines with the keybindings and skinning inline.

**Corrections:** 0
