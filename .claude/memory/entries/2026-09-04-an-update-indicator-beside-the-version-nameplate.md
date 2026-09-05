# A `⇡ vX.Y.Z` Update Indicator Beside The VERSION NAMEPLATE When GitHub Has A Newer Release — 2026-09-04

**Asked:** "add some indicator that a new upgate is available for nebula bottom left when one is published to github"
→ refined: In the FOOTER (bottom left), right after the VERSION NAMEPLATE, show an update indicator when a newer
NEBULA release than the running binary has been published on GitHub — e.g. `nebula v0.21.0 ⇡ v0.22.0`, the
`⇡ v0.22.0` part in an attention color (assuming that shape). Resolve "latest" the way INSTALL.SH does (the GitHub
`releases/latest` redirect — no `gh` token, no API quota), checked once at TUI start and hourly after, off the render
path; any failure shows nothing. Show it only when the published semver is strictly newer than the running one. Keep
the nameplate's FLASH-only yield and everything else in the FOOTER as it is; no auto-upgrade, no new key — NEBULA
UPGRADE stays the way to update.

**Did:** New module `crates/nebula-tui/src/update_check.rs`: `curl -sS -I -o /dev/null -w '%{redirect_url}'` against
`https://github.com/AgentSystemLabs/nebula/releases/latest`, `tag_from_redirect` → `newer_than(tag, CARGO_PKG_VERSION)`
(three-number semver only, so a pre-release tag never compares), and `spawn(tx)` sends only a *newer* version, so a
re-check that can't ask never clears a lit indicator. `event_loop.rs::main_loop` arms it with a
`sleep_until(next_update_check)` select arm at start and every `update_check::interval()` — `NEBULA_UPDATE_CHECK_SECS`
(new in `nebula-core::env`), default 3600, `0` = off. `App::update_available: Option<String>`;
`ui.rs::draw_footer_bar` renders ` ⇡ v{v}` as a third plate span in `th.warn` bold, counted in `plate_w` so the
FLASH-only yield drops plate and indicator together, with `workspace_idx += plate_spans.len()` keeping the WORKSPACE
NAMEPLATE hit target aligned. The E2E TUI harness sets the env to `0`; the DEV INSTANCE keeps the check on. Docs:
`docs/keys.md` FOOTER table, `docs/commands.md`, `docs/configuration.md` env table. Rejected `gh release view` (spends
the token the GIT POLL already rations for the Claude SESSIONS) and the REST API (60/hr unauthenticated). Gate:
nebula-tui 575 passed (four new unit tests + `footer_flags_a_newer_release_beside_the_nameplate`), e2e_tui 7 passed,
clippy clean; `cargo fmt --check` was red only in `e2e_pty.rs`, another session's in-flight work.

**Gotchas:**
- **GitHub's `releases/latest` page is the token-free "newest release" lookup.** A HEAD answers `302` with
  `Location: …/releases/tag/vX.Y.Z` (drafts and pre-releases excluded — the same resolution INSTALL.SH's
  `releases/latest/download/` relies on) and `-w '%{redirect_url}'` reads it without following. No API call, no
  quota, no `gh`; a repo with no releases redirects to `/releases`, which `tag_from_redirect` answers `None` for.
- **Anything that spawns the real TUI in a test must set `NEBULA_UPDATE_CHECK_SECS=0`** or the FOOTER's text
  depends on what GitHub has published relative to the built version: a test binary at 0.21.0 after v0.22.0 ships
  draws `⇡ v0.22.0` and shifts every hint ten columns right — a failure that appears in CI only after a release and
  never before it. `e2e_tui.rs::TuiHarness` sets it.
- **The nameplate's yield is a width sum and its hit target is a span count.** Adding to the plate means adding to
  `plate_w` (or a FLASH that just fit now clips) *and* to `workspace_idx` (or a click on `◇ workspace` lands on the
  indicator); the new test asserts both, the hit rect's `x` included.
- The channel carries `String`, not `Option<String>`: a release does not un-publish, so a later check that fails
  (offline, no curl) must not clear the indicator, and "nothing arrives" is the way to say so.
- `⇡` (U+21E1) is width 1 for unicode-width and the TESTBACKEND — no emoji padding, so an adjacency assertion like
  `"{stamp} ⇡ v9.9.9  ·  ◇ "` holds.
