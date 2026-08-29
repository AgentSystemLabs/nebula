# One Nebula Per Checkout: Auto-Port For `browser`, Per-Path Dev Slots — 2026-08-26

**Asked:** "merge latest from origin main into this and verify it works, find a way to be able to run nebula
without port conflicts as i run in various worktrees"

**Did:** Surveyed what actually binds anything first — the answer is *one* thing. The daemon uses a unix
socket (`paths::socket_path()`), and the hook receiver already binds `127.0.0.1:0`
(`nebula-daemon/src/hooks/mod.rs:156`), so neither can ever clash. The only fixed port in the tree is
ttyd's, so that plus the Makefile is the whole fix.

(1) **`nebula browser` chooses its port.** New `resolve_port` / `probe` / `free_port`
(`crates/nebula/src/browser.rs`); `run_browser` takes `Option<u16>` and `main.rs`'s `--port` lost its
`default_value_t`. No `--port` → 7681 when free, else a kernel-chosen one with
`nebula browser: 7681 is busy — serving on N instead` on stdout. `--port 0` → any free one (it used to
`bail!("needs a fixed port")`). `--port N` → that port or an error, deliberately: silently moving would
break an `ssh -L N:localhost:N` aimed at it. `probe` binds and immediately drops a `TcpListener` — a
listener that never accepted doesn't enter TIME_WAIT, so ttyd rebinds cleanly a moment later.

(2) **`make dev` / `make browser` are keyed to the checkout.** `DEV_SLOT := $(shell printf '%s' '$(CURDIR)'
| shasum | cut -c1-8)`, `DEV_RUNTIME := /tmp/nebula-dev-$(DEV_SLOT)`,
`DEV_DATA := $(HOME)/.nebula-dev/$(notdir $(CURDIR))-$(DEV_SLOT)`, and `PORT ?=` (empty → no `--port`,
so (1) picks). New `make dev-ls` lists every slot and whether its daemon is up.

Verified for real, not just by test: two `nebula browser` processes at once took 7681 and 49293, both
answering `200`; and the merge of `origin/main` (v0.10.0) into the prototype branch builds and passes
629 tests. Merge conflicts were two and both trivial (main added `reset_settings`/`reopen_settings`
directly above `set_show_workspaces`; both sides prepended a memory entry).

**Gotchas:**
- **Sharing `DEV_RUNTIME` across checkouts is worse than a port clash, and silent.** Both worktrees'
  `make dev` pointed at `/tmp/nebula-dev`, so the second TUI just *connected to the first's daemon* and you
  drove the other checkout's binary — the exact "I rebuilt and my change isn't there" the Makefile warns
  about elsewhere. Worse, `dev-prep` calls `dev-stop`, so starting one worktree SIGTERMed the other's
  daemon out from under it. Nothing reports either; it looks like your build didn't take.
- **The runtime dir gets the hash alone, not the worktree name.** It holds the unix socket and SUN_LEN is
  104 bytes on macOS; `/tmp/nebula-dev-<8hex>/daemon.sock` is 36 and safe for any checkout name. The
  readable name goes in `DEV_DATA`, which has no such limit.
- **The old flat `~/.nebula-dev/{nebula.db,config.json,state/}` is now orphaned** beside the new
  `<name>-<slot>/` dirs. `dev-seed` re-copies from the real DB per slot, so nothing is lost — but the old
  files are dead weight and can be deleted.
- **`~/.nebula-dev/config.json` has `show_workspaces: false`.** A `make dev` in a fresh slot inherits it
  through `dev-seed`, so the Workspaces bar starts hidden and looks broken. `Shift+W`.
- **A merge that compiles can still be semantically stale.** `cargo build` passed; `cargo test` then failed
  on `Project has no field named divider_after` — main had removed the divider fields
  (see [Project Dividers Removed]) and a *test helper* I'd added still set them. Build the tests, not just
  the lib, before calling a merge verified.
- **Killing a `nebula browser` parent leaves its ttyd running.** Ctrl+C works because the two share a
  process group; a bare `kill <pid>` does not reach the child, and the orphan keeps the port. Kill the
  ttyd pid too, or you'll be hunting a phantom "port in use" later. Related: [Orphan e2e daemons].
- `nebula browser` really does `open` the URL — running it twice to test opened two tabs in the desktop
  browser. There is no `--no-open`.
