#!/usr/bin/env bash
# SCREENSHOT HARNESS — `make shot [SCENE=open-prs] [KEYS="Tab j"]`.
#
# Runs the debug binary in an isolated nebula (own runtime dir, own data dir, a stub agent command so no
# real CLI ever launches, a stand-in `gh` on PATH answering from scripts/shot/fixtures) against a demo
# repository with two worktrees, drives it inside a private tmux server, and captures the screen as
# design-screenshots/<scene>.{txt,ansi,png}. Never touches the real daemon or the real data dir.
#
# Traps this encodes (MEMORY LOG, 2026-08-20 / 2026-08-21): NEBULA_RUNTIME_DIR must be short (the unix
# socket path caps at ~104 chars); NEBULA_AGENT_CMD must be set even with no agent (the PREWARM POOL
# launches a real claude otherwise); the first exec of a fresh binary can stall on macOS signature
# validation (warm it before the TUI's connect deadline); capture with `-epN` or trailing styled cells
# vanish; the daemon detaches and outlives tmux — kill it by pidfile.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SCENE="${1:-${SCENE:-open-prs}}"
COLS="${COLS:-190}"; ROWS="${ROWS:-50}"
BIN="$REPO/target/debug/nebula"
OUT="$REPO/design-screenshots"; mkdir -p "$OUT"
ID="$$"
RUNTIME="/tmp/nshot-$ID"                                   # short on purpose
WORK="${TMPDIR:-/tmp}/nebula-shot/$ID"; mkdir -p "$WORK" "$RUNTIME"; chmod 700 "$RUNTIME"
VENV="${NEBULA_SHOT_VENV:-${TMPDIR:-/tmp}/nebula-shot/venv}"
TMUX="tmux -L nshot-$ID"
cleanup() {
  $TMUX kill-server 2>/dev/null || true
  if [ -f "$RUNTIME/daemon.pid" ]; then kill "$(cat "$RUNTIME/daemon.pid")" 2>/dev/null || true; fi
  rm -rf "$RUNTIME"
}
trap cleanup EXIT

echo "shot: building"; (cd "$REPO" && cargo build -q)
"$BIN" --version >/dev/null                                 # pay the cold-exec stall here

# --- a demo repository with a main checkout and one worktree on a branch the fixtures know ---
DEMO="$WORK/demo"; mkdir -p "$DEMO"
git -C "$DEMO" init -q -b main
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q --allow-empty -m "demo"
git -C "$DEMO" worktree add -q -b feature-x "$WORK/demo-worktrees/feature-x" main
git -C "$DEMO" worktree add -q -b wheel-one-line "$WORK/demo-worktrees/wheel-one-line" main

export NEBULA_RUNTIME_DIR="$RUNTIME" NEBULA_DATA_DIR="$WORK/data" NEBULA_AGENT_CMD=/bin/cat \
       NEBULA_UPDATE_CHECK_SECS=0 NEBULA_GH_FIXTURES="${NEBULA_GH_FIXTURES:-$HERE/fixtures}" \
       PATH="$HERE/bin:$PATH" TERM=xterm-256color
"$BIN" add "$DEMO" >/dev/null                                # registers the PROJECT (spawns the demo daemon)

# --- drive it ---
$TMUX new-session -d -x "$COLS" -y "$ROWS" "$BIN"
sleep "${SHOT_BOOT_SECS:-4}"                                 # first paint + the first GIT POLL answers
send() { $TMUX send-keys "$1"; sleep "${SHOT_KEY_SECS:-0.6}"; }
if [ -f "$HERE/scenes/$SCENE.keys" ]; then
  while IFS= read -r key; do case "$key" in ''|'#'*) continue;; esac; send "$key"; done < "$HERE/scenes/$SCENE.keys"
fi
for key in ${KEYS:-}; do send "$key"; done
sleep 1
$TMUX capture-pane -epN > "$OUT/$SCENE.ansi"
$TMUX capture-pane -pN  > "$OUT/$SCENE.txt"
send C-q; send q; send y                                      # let the TUI restore the terminal
echo "shot: $OUT/$SCENE.txt  ($(wc -l < "$OUT/$SCENE.txt") rows)"

# --- render, in a venv that has Pillow ---
if [ ! -x "$VENV/bin/python" ]; then
  echo "shot: creating venv with Pillow at $VENV"
  python3 -m venv "$VENV" && "$VENV/bin/python" -m pip -q install pillow || { echo "shot: no Pillow — PNG skipped, .ansi/.txt are there"; exit 0; }
fi
"$VENV/bin/python" "$HERE/render.py" "$OUT/$SCENE.ansi" "$OUT/$SCENE.png" --cols "$COLS" --rows "$ROWS"
