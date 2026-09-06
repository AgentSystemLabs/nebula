# "Make Pr" Shipped A Hand-Written Body: The LAND SKILL Deferred The PR DESCRIPTION SKILL — 2026-09-05

**Asked:** "did you use the pr description skill when you made the pr?" — then "do it and figure out
why when I told you to make a pr you didn't use that full pr description.  update claude.md so it's
obvious to invoke that skill anytime a pr is created"

**Did:**
- Why: `.claude/skills/land/SKILL.md` §4 read "The PR DESCRIPTION SKILL only when the user asks for a
  description", and `CLAUDE.md`'s `land` row carried the same clause as its skip condition — both
  written the day before (`aed589f`) to keep landing chores short after the 2026-09-04 "17 minutes"
  complaint, a trade never put to the user. `pr-description`'s own triggers include "open a PR" while
  `land`'s include "make pr" / "open a PR and merge it"; `CLAUDE.md` routes "make pr" to `land`, so the
  LAND SKILL won the phrase and its body's skip list dropped the other skill. PR #30 therefore opened
  with a short hand-written body.
- Fix: `CLAUDE.md` gained its own row — a pull request about to be created or its body rewritten,
  inside `land` or on any other path → `Skill(skill: "pr-description")`, skip never; the `land` row's
  skip cell lost the description clause; `land` §4 now invokes the PR DESCRIPTION SKILL and says a
  hand-written body is not a shape it has; `AGENTS.md` step 2 says the same for non-Claude agents.
- PR #30's body rewritten with the PR DESCRIPTION SKILL (template `01-benefit-groups`): two SCREENSHOT
  HARNESS scenes (`scripts/shot/scenes/settings-experimental.keys`,
  `scripts/shot/scenes/quick-prompt-new-worktree.keys`), the PNGs pushed to
  `pr-assets/hide-root-auto-worktree/`, a `sequenceDiagram`, the risk read, the technical overview,
  `gh pr edit 30 --body-file`. The PR read CLEAN after CLAUDE REVIEW.
- Mid-task the user reported the TOC links did nothing on PR #30. The rendered `body_html` showed the
  cause: anchors come back as `<a id="user-content-screenshots">` while the links stayed
  `href="#screenshots"`, and the PR conversation page has no hash handler bridging the prefix. Every
  TOC link now names `#user-content-<id>` — the PR DESCRIPTION SKILL's *GitHub anchors* section, its
  checker (which now also flags an unprefixed link and a prefixed anchor), all ten templates, and PR
  #30's body (`gh pr edit`, then `body_html` re-read to confirm the hrefs). The 2026-09-04 entry and its
  standing gotcha were corrected in place.
- Gate: `make memory-check recall-eval terms-check` (no crate changed).

**Gotchas:**
- Two skills can claim one phrase: `pr-description` lists "open a PR", `land` lists "make pr" /
  "open a PR and merge it". The skill `CLAUDE.md`'s table names wins, and the winner's body decides
  whether the other runs at all — a skill that must always run *inside* another has to be invoked from
  the outer skill's body and get its own `CLAUDE.md` row with "skip: never"; its own trigger list
  guarantees nothing.
- `make shot` builds into the repo's own `target/` (`$REPO/target/debug/nebula` is hard-coded), so
  `CARGO_TARGET_DIR` must not be exported for it — a worktree that gated with a private `vtarget` pays a
  second full debug build before its first shot; start `cargo build` in the background early.
- The SCREENSHOT HARNESS starts every run from a fresh data dir, so a scene that needs a SETTING on
  toggles it through the SETTINGS OVERLAY's own keys (`s`, `Tab`×4 to Experimental, `j`, `Enter`, then
  `Escape` `Escape`); the TUI boots with FOCUS on the PROJECTS PANEL, so one `Tab` reaches the
  WORKTREES PANEL — and the QUICK PROMPT's title (`new worktree …` or not) is the oracle for which
  panel had FOCUS.
- In a 190×50 capture the SPLASH's text shows beside the SETTINGS OVERLAY's box ("ven when you leave",
  the config path line) — the modal is narrower than the splash behind it. Pre-existing, not this PR's.

- A `<a id="x"></a>` in a PR body renders as `id="user-content-x"`; a `#x` link is dead on the PR page
  (the file-view JS that bridges the prefix is not there), so the link must say `#user-content-x` and the
  anchor must not — GitHub would double the prefix. `body_html` from the pulls API is the only honest
  check short of clicking; `gh api /markdown` in any mode shows the source, not the page.

**Corrections:** 2 — the user asked why "make pr" had produced no full description, then reported the
TOC links did not navigate.
