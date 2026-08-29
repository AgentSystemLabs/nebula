# `make dev` Showed v0.4.0 And No Projects — 2026-08-25

**Asked:** "still when I run make dev, it shows version v0.4.0 in the bottom left and now it seems like
all my projects and workspaces are done [gone]"

**Did:** Two unrelated causes. (1) The shared checkout was still at `026b64c` / `Cargo.toml` 0.4.0 while
`origin/main` was at v0.7.0 — every release since had been cut from a private worktree and never pulled
back, so `make dev` (which builds *this* checkout) faithfully reported 0.4.0. Synced it: `git stash -u`
(kept as `stash@{0}` for safety), `git pull --ff-only`, then restored only the `Makefile` from the stash.
(2) `make dev` runs with `NEBULA_DATA_DIR=~/.nebula-dev`, a deliberately separate DB, so it had zero
projects by design. Added `dev-seed` (Makefile) — on the first `make dev` it `sqlite3 .backup`s the real
DB into the dev dir, `DELETE`s `agents` and `terminals`, and copies `config.json`/`reviewed.json`; plus
`dev-reset` (wipe, so the next run re-seeds) and `make dev SEED=0` (start blank). Verified: dev DB got
7 projects / 3 workspaces / 9 worktrees / 0 agents, real DB untouched, and `nebula workspace list` against
the dev env booted a 0.7.0 dev daemon on it with a clean log.

**Gotchas:**
- **The real data dir is `~/Library/Application Support/dev.nebula.nebula/`** on macOS
  (`directories::ProjectDirs::from("dev","nebula","nebula")`, `nebula-core/src/paths.rs`);
  `$XDG_DATA_HOME/nebula` on Linux. Nothing prints it — the Makefile mirrors the rule by hand.
- Every dirty file in the shared tree except the `Makefile` was either byte-identical to `origin/main` or
  *older* than it (the pre-#12 `h`/`l` bindings and the macOS-only clipboard) — the same stale hunks the
  v0.6.0/v0.7.0 entries describe. `git diff origin/main -- <file> | grep -c ^@@` per file is the quick
  way to tell in-flight work from leftovers before discarding anything.
- Agent rows are only spawned lazily (`Registry::ensure_session`, `registry.rs:~1857`), so copying
  `agents` would not have launched anything at boot — they're dropped anyway so the dev instance can't
  `--resume` your live claude sessions.
- `sqlite3 ".backup"` reads the WAL, so the snapshot is consistent with the real daemon still running;
  a plain `cp nebula.db` would miss everything in `nebula.db-wal`.
