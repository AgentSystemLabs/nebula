# Dev helpers for working from a checkout. (End users install via install.sh.)
#
# Ways to run code you just wrote:
#   make dev      isolated instance — own daemon, own data; your real sessions untouched
#                 (first run copies your real projects, worktrees, workspaces and
#                 settings in, so it looks like yours — `make dev-reset` re-copies)
#   make browser  the same isolated instance, served into a browser tab via ttyd
#
# Each checkout gets its own instance, keyed to its path, so the main clone and
# every worktree can run at once without sharing a daemon, a DB, or a port.
# `make dev-ls` shows them all.
#   make install  put it in ~/.cargo/bin for real use (then `make kill` to cut over)
#   make cycle    install + kill + dev in one go — the re-runnable full cutover

PREFIX      ?= $(HOME)/.cargo/bin
RELEASE_BIN := target/release/nebula
DEBUG_BIN   := target/debug/nebula

# The dev instance is a second, complete nebula: its own socket, DB, and
# settings — and one *per checkout*, so the main clone and every worktree can
# run at the same time. Sharing them is worse than a port clash: two checkouts
# on one runtime dir means the second TUI silently attaches to the first's
# daemon and you drive the other checkout's binary, while `dev-prep` below
# SIGTERMs whichever daemon got there first.
#
# The slot is the checkout's directory name plus a hash of its absolute path,
# so two worktrees with the same name in different repos still separate. The
# runtime dir takes the hash alone — it holds a unix socket, and SUN_LEN (104
# bytes on macOS) is not a budget a long worktree name should be spending.
DEV_SLOT    := $(shell printf '%s' '$(CURDIR)' | shasum | cut -c1-8)
DEV_RUNTIME := /tmp/nebula-dev-$(DEV_SLOT)
DEV_DATA    := $(HOME)/.nebula-dev/$(notdir $(CURDIR))-$(DEV_SLOT)
# `make dev SEED=0` skips the first-run copy and starts the dev instance empty.
SEED ?= 1
# `make dev AGENT=/bin/cat` stubs agents out, so nothing spawns a real claude —
# including the warm-slot prewarm, which launches one before you create any
# agent at all. Unset (the default) means real agents, exactly like production.
AGENT ?=
# Left empty on purpose: `nebula browser` then takes 7681 when it is free and
# a free port otherwise, printing which — so a `make browser` per checkout all
# serve at once. `make browser PORT=8080` pins it (and fails if 8080 is taken,
# which is what you want when you have an ssh tunnel pointed at it).
PORT ?=

# Every dev-instance run goes through this: its own socket dir and its own DB,
# so nothing here can touch the real daemon's state.
DEV_ENV = NEBULA_RUNTIME_DIR=$(DEV_RUNTIME) NEBULA_DATA_DIR=$(DEV_DATA) \
	$(if $(AGENT),NEBULA_AGENT_CMD=$(AGENT))

.DEFAULT_GOAL := help
.PHONY: help dev browser dev-prep dev-seed dev-reset dev-ls dev-stop build install kill cycle check fmt lint test ci clean

help: ## Show this help
	@grep -hE '^[a-z][a-z-]*:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-11s\033[0m %s\n", $$1, $$2}'

# --- running your changes ----------------------------------------------------

dev: dev-prep ## Run the latest code in an isolated instance (own daemon + data)
	@echo "dev instance [$(notdir $(CURDIR))] → runtime $(DEV_RUNTIME), data $(DEV_DATA)"
	-@$(DEV_ENV) $(DEBUG_BIN)
	@$(MAKE) --no-print-directory dev-stop

# `nebula browser` shells out to ttyd and serves *this* binary
# (`current_exe`, not whatever `nebula` is on PATH), so the tab gets the build
# below rather than the installed release. ttyd hands its environment to the
# command it runs, so $(DEV_ENV) reaches the TUI in the tab and the browser
# instance stays as isolated as `make dev`. Needs ttyd on PATH — the binary
# says how to install it if it is missing. Ctrl+C here stops ttyd.
browser: dev-prep ## Serve the latest code into a browser tab via ttyd (PORT= to pin)
	@echo "dev instance → runtime $(DEV_RUNTIME), data $(DEV_DATA)"
	-@$(DEV_ENV) $(DEBUG_BIN) browser $(if $(PORT),--port $(PORT))
	@$(MAKE) --no-print-directory dev-stop

# Build, clear the way, and seed — everything `dev` and `browser` both need
# before they can hand the terminal over.
dev-prep:
	cargo build
	@# Load-bearing: a dev daemon from a previous run detached and outlived
	@# its TUI, and it is still executing the OLD code. Connecting to it is
	@# precisely how "I rebuilt and my change isn't there" happens — so stop
	@# it, and let this run spawn a fresh daemon from the binary above.
	@$(MAKE) --no-print-directory dev-stop
	@# Also load-bearing: on macOS the first exec of a freshly relinked binary
	@# pays for signature validation and can stall for seconds. Paying it here
	@# keeps the daemon spawn inside the TUI's 3s connect deadline
	@# (nebula-tui/src/ipc.rs) instead of failing with "daemon did not come up".
	@$(DEBUG_BIN) --version >/dev/null
	@$(if $(filter 0,$(SEED)),true,$(MAKE) --no-print-directory dev-seed)

# A blank dev instance is useless for eyeballing a change — you'd re-add every
# project by hand first. So the first `make dev` snapshots the real DB and
# settings, minus `agents` and `terminals`: those rows are the live sessions
# the real daemon owns, and the dev daemon must not resume them. `.backup`
# reads the WAL, so the copy is consistent even with the real daemon running.
# The real dir is where `directories::ProjectDirs::from("dev","nebula","nebula")`
# puts it (nebula-core/src/paths.rs); keep the two in step.
dev-seed: ## Copy real projects/workspaces/settings into the dev instance (only if it has no DB yet)
	@[ ! -e $(DEV_DATA)/nebula.db ] || exit 0; \
	case "$$(uname -s)" in \
		Darwin) real="$$HOME/Library/Application Support/dev.nebula.nebula";; \
		*)      real="$${XDG_DATA_HOME:-$$HOME/.local/share}/nebula";; \
	esac; \
	if [ ! -f "$$real/nebula.db" ]; then \
		echo "no real nebula data at $$real — dev instance starts empty"; exit 0; fi; \
	if ! command -v sqlite3 >/dev/null 2>&1; then \
		echo "sqlite3 not on PATH — dev instance starts empty"; exit 0; fi; \
	mkdir -p $(DEV_DATA); \
	sqlite3 "$$real/nebula.db" ".backup '$(DEV_DATA)/nebula.db'"; \
	sqlite3 $(DEV_DATA)/nebula.db "DELETE FROM agents; DELETE FROM terminals;"; \
	for f in config.json reviewed.json; do \
		if [ -f "$$real/$$f" ]; then cp "$$real/$$f" $(DEV_DATA)/; fi; \
	done; \
	echo "seeded dev instance from $$real (projects, worktrees, workspaces, settings — no sessions)"

dev-reset: dev-stop ## Wipe this checkout's dev data; the next `make dev` re-seeds it
	rm -rf $(DEV_DATA)

# Slots accumulate: a worktree you deleted leaves its DB behind under
# ~/.nebula-dev. This lists every one with its daemon's state, so you can see
# what is still running and `rm -rf` what is not.
dev-ls: ## List every checkout's dev instance and whether its daemon is up
	@for d in $(HOME)/.nebula-dev/*-*/; do \
		[ -d "$$d" ] || continue; \
		slot=$${d%/}; slot=$${slot##*-}; \
		pidfile=/tmp/nebula-dev-$$slot/daemon.pid; \
		state=stopped; \
		if [ -f "$$pidfile" ] && ps -p "$$(cat $$pidfile 2>/dev/null)" -o command= 2>/dev/null \
			| grep -q 'nebula daemon'; then state=running; fi; \
		printf '  %-8s %-40s %s\n' "$$state" "$$(basename $$d)" "$$d"; \
	done

# The pidfile outlives the process it names, so confirm the pid is still a
# nebula daemon before signalling it — otherwise a recycled pid means killing
# some unrelated process of the user's. SIGTERM (not KILL) so the daemon runs
# its normal shutdown and takes its PTY children with it.
dev-stop: ## Stop the dev daemon (it detaches, so quitting the TUI leaves it running)
	@pidfile=$(DEV_RUNTIME)/daemon.pid; \
	[ -f $$pidfile ] || exit 0; \
	pid=$$(cat $$pidfile 2>/dev/null); \
	case "$$pid" in ''|*[!0-9]*) exit 0;; esac; \
	if ps -p $$pid -o command= 2>/dev/null | grep -q 'nebula daemon'; then \
		kill $$pid 2>/dev/null || true; \
	fi

# --- installing for real use -------------------------------------------------

build: ## Release build
	cargo build --release

# The cp+mv two-step is load-bearing on macOS: overwriting the installed
# binary in place reuses its inode, and the kernel's cached code signature
# for that inode no longer matches the new contents — every exec then dies
# with SIGKILL (exit 137). A fresh inode forces signature re-validation.
install: build ## Install to $(PREFIX) — warns if the live daemon is now stale
	cp $(RELEASE_BIN) $(PREFIX)/nebula.new
	mv $(PREFIX)/nebula.new $(PREFIX)/nebula
	@$(PREFIX)/nebula --version
	@$(PREFIX)/nebula _stale-daemon-note

kill: ## Stop every session and the daemon — the cutover step after `make install`
	$(PREFIX)/nebula kill

# The whole cutover as one command, safe to re-run as often as you like:
# install first, so a build that fails stops here with every session still
# alive; then kill the real daemon (it is now running the old binary — the
# STALE DAEMON NOTE `install` just printed says as much); then `dev`, which
# builds the debug binary and hands the terminal to the isolated dev instance.
# `nebula kill` exits 0 and says "no nebula daemon running" when there is
# nothing to stop, so the first run on a cold machine goes through too. The
# kill stops every real session — run this from a terminal outside nebula,
# not from a session it would take down with it. Recipe lines rather than
# prerequisites so `make -j` cannot reorder the three.
cycle: ## Install, kill the real daemon, run the dev instance — re-run whenever
	@$(MAKE) --no-print-directory install
	@$(MAKE) --no-print-directory kill
	@$(MAKE) --no-print-directory dev

# --- checks ------------------------------------------------------------------

check: ## Typecheck the workspace (fastest feedback)
	cargo check --workspace --all-targets

fmt: ## Format the workspace
	cargo fmt --all

# Not `-D warnings` by default: the workspace does not currently clear that
# bar (pre-existing lints in config.rs, ui.rs, and hooks/mod.rs), and CI runs
# no clippy at all. `make lint STRICT=1` opts into the stricter gate.
lint: ## Clippy over the workspace (STRICT=1 to fail on warnings)
	cargo clippy --workspace --all-targets $(if $(STRICT),-- -D warnings)

test: ## Full test suite (e2e_pty spawns real daemons — slow)
	cargo test --workspace

ci: ## The whole gate: fmt check, clippy, tests
	cargo fmt --all -- --check
	@$(MAKE) --no-print-directory lint
	@$(MAKE) --no-print-directory test

clean: ## Remove build artifacts
	cargo clean
