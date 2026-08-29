# Shared Working Tree Is Raced By Other SESSIONS — 2026-08-23

**Asked:** (no prompt — surfaced mid-task) A `git stash push -m hotkey-wip` + pop cycle from **another**
Claude session reverted and then restored every uncommitted file mid-edit, and the pop left three
duplicated `activity:` fields in `event_loop.rs` test fixtures.

**Did:** Nothing to commit — recorded as a working rule.

**Gotchas:**
- The user runs nebula's own agents against this repo, so the SHARED CHECKOUT is routinely mid-refactor from
  someone else. A `cargo check`/`cargo test` failure often has nothing to do with your change — check
  whether the failing symbols belong to unrelated in-flight work before blaming your own edit.
- Re-verify your edits are still on disk after any unexplained state change. Never `git stash pop` or
  `git checkout` the SHARED CHECKOUT on your own judgment.
- A self-contained new module can be checked in isolation with `rustc --test --edition 2021 <file>` when
  the crate as a whole won't build.
