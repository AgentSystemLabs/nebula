# The VERSION NAMEPLATE In The FOOTER's Left Edge — 2026-08-24

**Asked:** "display the version number of nebula in the bottom bar somewhere, I think bottom left should
say nebula vx.y.z"

**Did:** `draw_footer` (`crates/nebula-tui/src/ui.rs`) now splices `nebula v{env!("CARGO_PKG_VERSION")}`
+ `"  ·  "` in at span index 1 (after the leading pad, ahead of the WORKSPACE NAMEPLATE `◇ workspace`), styled `th.dim`. The
splice happens **after** `left` is computed, not where the workspace span is pushed, because the decision
needs the width the usage readout left behind: it is skipped when `app.flash.is_some()` **and** the spans
already measure wider than `left.width`. New unit test
`footer_shows_the_nebula_version_but_never_truncates_a_flash` covers both branches. `nebula --version`
(clap, `crates/nebula/src/main.rs:10`) reads the same workspace version, so the two agree by construction.

**Gotchas:**
- **The FOOTER's left edge is a fixed column budget and it was already full.** The VERSION NAMEPLATE costs 18
  columns (`nebula v0.2.0` + separator) and everything downstream of it — workspace, hostname, conn,
  breadcrumb, hints/flash — just shifts right and clips off the end. Anything else added here pays the
  same toll.
- **A clipped FLASH is a broken feature, a clipped hint list is not.** The e2e
  `tui_pull_request_row_leads_the_links_group` caught this: at `COLS = 120` the bar rendered
  `… #7 Attach links    the pull request link c` — the flash lost `an't be deleted`. Hint lists are
  ordered by importance and truncate harmlessly; flashes are sentences. Hence the flash-only yield rather
  than a blanket "only if it all fits", which on a 120-col terminal would have hidden the version almost
  always.
- That failure looked like a flake at first — it passed alone and `tui_link_crud_in_sessions_panel` failed
  alongside it once, then didn't. Only `tui_pull_request_row_leads_the_links_group` was real. The panic
  message's `--- screen ---` dump is what identifies it: read the rendered footer line, don't rerun blind.
- `splash_footer_lists_only_keys_that_work` (`event_loop.rs`) asserts `m: menu` reaches the bar and was
  sized at exactly `TestBackend::new(140, 30)` — 18 columns of nameplate pushed `m: menu  ?: help` off.
  Bumped to 160. Any future footer addition trips this test first; it is a width canary, not a key bug.
- This tree's `Cargo.toml` is still `0.2.0` while `origin/main` is `0.4.0` (see the v0.4.0 entry below), so
  a binary built here reads **nebula v0.2.0**. That is the built version, not a bug in the readout.
