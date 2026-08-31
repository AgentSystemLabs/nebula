# A Ten-Agent Docs Audit Found Eight False Claims In README And `docs/` — 2026-08-30

**Asked:** "use 10 sub agents to explore various parts of our code base and then inspect the docs to
verify they are all up to date and perfectly document the feature sets and arguments we allow" — then,
on the findings: "do all tiers, usew sub agents to update the files"
→ refined: "Run 10 read-only subagents in parallel over `crates/` — the CLI surface (`cli.rs`,
`help_cli.rs`), the KEYMAP, the CONFIG.JSON schema, the AGENT PRESETS, the DAEMON, NEBULA BROWSER / SSH /
TUNNEL, NEBULA WORKSPACE, the hook routes, the panels and OVERLAYS — and check `README.md` and the five
pages under `docs/` against what the code actually does. Report every command, flag, key, config field,
env var and feature that is missing, wrong, or documented but gone, ranked by severity, in TERMS."
(asked: doc scope → README + `docs/` only; asked: fix or report → report, then fix on approval)

**Did:** Two waves of subagents, one lens each, then one file each so territories stayed disjoint —
10 read-only auditors, then 7 writers (`README.md`, the five `docs/` pages, `cli.rs`). I verified every
Tier-1/Tier-2 finding in the code myself before it went in the report; **one auditor finding was wrong
and rejected** (see Gotchas). Net: 309 insertions / 56 deletions across 10 files.
- **False claims corrected:** `q` opens a CONFIRM DIALOG, it does not quit outright
  (`event_loop.rs:1410`) — was wrong in `docs/keys.md:59` *and* `README.md:99`; `nebula daemon
  --foreground` logs to **stdout**, not stderr (`crates/nebula/src/main.rs:145-148` — no
  `.with_writer`, and `tracing_subscriber`'s `SubscriberBuilder` defaults `W = fn() -> io::Stdout`);
  the HOSTS PICKER is `Shift+H`, not `h` (`keymap.rs:445`); `Shift+Tab` steps into the WORKSPACES BAR
  in one press (`focus_walk.rs:77-80`); click-outside is *not* `Esc` for the staged-filter OVERLAYS
  (`overlay_close.rs:70-79`); OPEN PRS is 15 s only for a PROJECT with PRs, else 30 s→10 min backoff
  (`event_loop.rs:152-154`).
- **`docs/configuration.md` 33 → 140 lines**, now covering all 26 CONFIG.JSON fields with a table
  marking which the SETTINGS OVERLAY can edit. `prewarm_agents` / `prewarm_sessions` /
  `git_init_on_create` were documented nowhere and have no overlay row at all
  (`nebula-daemon/src/config.rs:10-40`).
- **`TERMS.md` corrected on four rows I own:** DEFAULT WORKSPACE (the "can never delete" claim is
  false), PROTOCOL VERSION (33 → 34), NEBULA UPGRADE (idle daemon auto-shutdown), FOOTER
  (`⏻ connected` is built at `ui.rs:3736` but only pushed when *dis*connected, `ui.rs:3979`).
- **Stale `h` picker wording** also fixed in three code comments: `ssh.rs:50`, `tunnel.rs:115`,
  `hosts.rs:1`.
- Gate: `cargo check --workspace` exit 0, 0 warnings; `make terms-check` ok (dead 0, stale 0,
  dangling 0); `make memory-check` ok.

**Gotchas:**
- **A subagent's finding is a hypothesis, not a result.** One auditor reported `Ctrl+y` copy is
  "silently disabled" over `nebula ssh` because of `!app.is_remote && copy_to_clipboard(...)`. Reading
  four more lines shows `copy_and_flash` (`event_loop.rs:5692-5705`) falls through to an OSC 52
  terminal copy and flashes `(via terminal)` — copying works fine remotely. It had stopped at the
  first `if`. Verify every finding at the call site before it reaches the user.
- **The docs were wrong in the places nobody re-reads, not the places that rot.** Every command, flag,
  default, model name and resume form still matched, and there was **no** documentation of a removed
  feature anywhere. What was false was behavior that changed *under* a line nobody thought to revisit
  (`q` got its confirm on 2026-08-29; `docs/keys.md:59` was last touched before that). Grep the
  behavior, not the feature list.
- **`TERMS.md` went stale in the same direction as the docs and is not covered by TERMS CHECK.**
  `terms_check.py` validates pointers, aliases and dead/once TERMS — it never checks whether a row's
  *prose* is still true, so "can never delete" and "(33)" both sat green. Four of my corrections were
  facts a gate cannot catch.
- **`cargo check --workspace | tail` is blocked by the GUARD HOOK** and it is right: a pipeline returns
  `tail`'s exit code. Redirect to a log and `echo $?`.
- **Standing gotchas sits exactly at its 300-line cap**, so a new trap costs a prune. Nothing here was
  prunable (the skill's rule is "delete what a test or GUARD HOOK now enforces", and documenting a trap
  in DOCS PAGES is not enforcement), so this task's trap was **folded into the existing PROJECT TERMS
  corpus line** instead of taking a row. Extending an adjacent line is the cheap move at the cap.

**Corrections:** 0
