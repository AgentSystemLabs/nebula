# `make prune` Drops Stale target/ Hashes — 41GB Of Object Files From Release Bumps × Build Variants — 2026-08-29

**Asked:** "based on recent sessions, you helped me clean up disk space before, try to determine where I
can free up space again" then "update the make cycle to automatically clean up old stuff if most of this
was coming from me rebuilding nebula"
→ refined: Find where the disk filled again since the 2026-08-19 cleanup (139MB → 67GB free then; 135MB
free now), attribute the regrowth, and — since most of it is nebula's own `target/` — add a TARGET PRUNE
to MAKE CYCLE that removes stale build artifacts automatically while keeping the warm builds warm; leave
Docker, Steam, Downloads and the rest as a report, not an action.

**Did:** `scripts/prune-target.py` (new) + `Makefile::prune` (`make prune`, `KEEP ?= 3`), wired into
MAKE CYCLE as install → kill → **prune** → dev. The pruner groups every entry of `target/<profile>/{deps,
.fingerprint,build,incremental}` by `<stem>-<hash>`, keeps the newest `KEEP` hashes per stem (newest
mtime among the hash's files) and deletes the rest, holding cargo's own `target/<profile>/.cargo-lock`
via `flock` so it waits for a running build and no build starts under it; `--dry-run` reports only. First
run: 41.6GB freed in 63s (`target/` 41GB → 2.8GB), after which `cargo check --workspace --all-targets`,
`cargo build`, `cargo test --workspace --no-run` and `cargo build --release` were all 0-crate no-ops and
the live daemon (running from `target/debug/nebula`) was untouched. Rejected `cargo sweep --time/--maxsize`:
it evicts by mtime, so still-current registry crates compiled a week ago go first and every cycle would
recompile the dep graph. Rest of the survey (not acted on): Docker.raw 20GB with ~4.7GB reclaimable in-VM,
Desktop 11GB, Steam 10GB, `Library/pnpm` 7.5GB, Claude Desktop 6.9GB, Downloads 6.6GB, `.cursor` 5.8GB,
`/private/tmp/claude-501` session scratchpads 16GB (RELEASE WORKTREE target dirs).

**Gotchas:**
- **`target/debug/deps` held 372,771 `.o` files (52GB apparent) — macOS's dev-profile default
  `split-debuginfo=unpacked` leaves every build's object files beside the binary for the debugger, hard-
  linked into `incremental/`, and nothing ever removes the old ones.** ~200MB per build of `nebula_tui`.
- **Each release bump re-hashes every workspace crate, × each command variant** (`cargo build` / `test` /
  `check` / `clippy` resolve features differently): 202 hashes of `nebula_tui` and 163 of `nebula_daemon` in
  ten days while registry crates had 8–12. The fingerprint json (`rustc`, `profile`, `features`,
  `rustflags`, `path`) is identical across the hashes — only the package version differs.
- Keep-N-per-stem is the right policy, not age: a `KEEP=3` prune left all four build variants as no-ops,
  while "drop groups untouched > 2 days" would have reclaimed less (47 of 63GB) *and* forced dep rebuilds.
- `du` on `deps/` double-counts: the `.o` files are hard links shared with `incremental/`, so 63GB
  "on-disk" was 41GB real — report reclaimed space from `statvfs` before/after, not from summed sizes.
- `timeout` is not on this box's PATH (no coreutils); `perl -e 'alarm N; exec @ARGV' <cmd>` is the stand-in.
