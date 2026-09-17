#!/usr/bin/env bash
# LATENCY HARNESS — `scripts/perf/run.sh [scenario]` (default: scripts/perf/scenario.steps).
#
# Drives the real TUI through a scripted session inside a private tmux, against an isolated daemon and a
# realistically sized checkout (a local clone of this repository: a few hundred files, a vendored crate,
# dirty files, three worktrees), with the INPUT LATENCY PROBE on (`NEBULA_PERF_LOG`, crates/nebula-tui/
# src/perf.rs). `report.py` then prints, per step, how long the key's handler held the loop, how long the
# key waited for its frame, and how long the screen took to settle. Never touches the real daemon.
#
#   BIN=target/release/nebula scripts/perf/run.sh      measure another build (default target/debug/nebula,
#                                                      the build `make dev` runs)
#   PERF_DUMP=1 scripts/perf/run.sh                    also save the screen after every step (debugging a
#                                                      scenario that went astray)
#   OUT=/some/dir scripts/perf/run.sh                  where the log, the report and the dumps go
#   PERF_REPO=~/src/big scripts/perf/run.sh            clone that repository as the checkout instead of
#                                                      this one (it is only read): what `g`, `f`, `b` and
#                                                      `F` cost where git has real work to do
#   python3 scripts/perf/report.py BEFORE AFTER        two runs' OUT dirs, side by side
#
# A scenario line is `label<TAB>key`: a tmux key name (`j`, `Enter`, `C-d`, `BTab`), or `text:…` typed
# literally in one go. Lines starting with `#` are comments. One line is one step; steps are STEP_SECS
# apart (default 0.7), which is what lets the report tell them apart.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SCENARIO="${1:-$HERE/scenario.steps}"
COLS="${COLS:-190}"; ROWS="${ROWS:-50}"
BIN="${BIN:-$REPO/target/debug/nebula}"; case "$BIN" in /*) ;; *) BIN="$REPO/$BIN";; esac
ID="$$"
RUNTIME="/tmp/nperf-$ID"                                    # short on purpose (SUN_LEN)
WORK="${TMPDIR:-/tmp}/nebula-perf/$ID"; mkdir -p "$WORK" "$RUNTIME"; chmod 700 "$RUNTIME"
# The results outlive the run; the checkout it was measured against does not. `target/` is git-ignored
# and `make clean` takes it, so that is where they go unless OUT says otherwise.
OUT="${OUT:-$REPO/target/perf/$(date +%Y%m%d-%H%M%S)}"; mkdir -p "$OUT"
TMUX="tmux -L nperf-$ID"
cleanup() {
  $TMUX kill-server 2>/dev/null || true
  if [ -f "$RUNTIME/daemon.pid" ]; then kill "$(cat "$RUNTIME/daemon.pid")" 2>/dev/null || true; fi
  # The clone, its worktrees and the daemon's data dir: hundreds of megabytes a run, and a dozen runs
  # of leaving them behind is a full disk.
  rm -rf "$RUNTIME" "$WORK"
}
trap cleanup EXIT
[ -x "$BIN" ] || { echo "perf: no binary at $BIN — build it first" >&2; exit 1; }
"$BIN" --version >/dev/null                                 # pay the cold-exec stall here

# --- a checkout big enough that git costs what it costs in real use ---
DEMO="$WORK/demo"
git clone -q --local "${PERF_REPO:-$REPO}" "$DEMO"
git -C "$DEMO" checkout -q -B main
git -C "$DEMO" worktree add -q -b feature-x "$WORK/demo-worktrees/feature-x" main
git -C "$DEMO" worktree add -q -b wheel-one-line "$WORK/demo-worktrees/wheel-one-line" main
# Work in progress in the main checkout: a dozen edited files and a few new ones.
n=0
for f in $(git -C "$DEMO" ls-files | grep -E '\.(rs|ts|tsx|js|jsx|py|go|md)$' | grep -v ' ' | head -12); do
  n=$((n + 1)); printf '\n// perf edit %d\n' "$n" >> "$DEMO/$f"
done
for i in 1 2 3; do printf 'scratch %d\n' "$i" > "$DEMO/scratch-$i.txt"; done

# --- stand-ins: `gh` answers from the shot fixtures, `open` opens nothing, agents print and idle ---
mkdir -p "$WORK/bin" "$WORK/data"
ln -s "$REPO/scripts/shot/bin/gh" "$WORK/bin/gh"
# What `open` costs on a Mac before it returns (LaunchServices round trip), without a browser tab
# per run.
printf '#!/bin/sh\nsleep 0.08\nexit 0\n' > "$WORK/bin/open"; chmod +x "$WORK/bin/open"
# An agent with a full ring: ~1 MB of colored output, then idle — the replay an attach has to parse.
cat > "$WORK/bin/agent" <<'AGENT'
#!/bin/sh
i=0
while [ "$i" -lt 12000 ]; do
  printf '\033[3%dm%05d\033[0m the quick brown fox jumps over the lazy dog, again and again and again\r\n' $((i % 7 + 1)) "$i"
  i=$((i + 1))
done
exec /bin/cat
AGENT
chmod +x "$WORK/bin/agent"
printf '{"prewarm_agents":false,"prewarm_sessions":false}\n' > "$WORK/data/config.json"

export NEBULA_RUNTIME_DIR="$RUNTIME" NEBULA_DATA_DIR="$WORK/data" NEBULA_AGENT_CMD="$WORK/bin/agent" \
       NEBULA_UPDATE_CHECK_SECS=0 NEBULA_GH_FIXTURES="$REPO/scripts/shot/fixtures" \
       PATH="$WORK/bin:$PATH" TERM=xterm-256color NEBULA_PERF_LOG="$OUT/perf.jsonl"
"$BIN" add "$DEMO" >/dev/null                                # registers the PROJECT (spawns the daemon)
# A second, small project, so the Projects panel has somewhere to go.
git init -q -b main "$WORK/tiny"
git -C "$WORK/tiny" -c user.name=perf -c user.email=perf@example.invalid commit -q --allow-empty -m tiny
"$BIN" add "$WORK/tiny" >/dev/null

now_us() { python3 -c 'import time; print(int(time.time() * 1e6))'; }
printf '%s\tlaunch\t\n' "$(now_us)" > "$OUT/steps.tsv"
$TMUX new-session -d -x "$COLS" -y "$ROWS" "$BIN"
sleep "${PERF_BOOT_SECS:-4}"                                 # first paint + the first GIT POLL answers

# Memory, twice a second for the whole run: the TUI's RSS and the daemon's (its sessions are the stand-in
# agent, so this is nebula's own footprint). A faster UI that holds more is not a win; the report prints
# the peak of each beside the latencies.
TUI_PID="$($TMUX display-message -p '#{pane_pid}')"
( while kill -0 "$TUI_PID" 2>/dev/null; do
    d="$(cat "$RUNTIME/daemon.pid" 2>/dev/null || true)"
    printf '%s\t%s\n' "$(ps -o rss= -p "$TUI_PID" | tr -d ' ')" "$( [ -n "$d" ] && ps -o rss= -p "$d" | tr -d ' ')" >> "$OUT/rss.tsv"
    sleep 0.5
  done ) &
step=0
while IFS=$'\t' read -r label key; do
  case "$label" in ''|'#'*) continue;; esac
  step=$((step + 1))
  printf '%s\t%s\t%s\n' "$(now_us)" "$label" "$key" >> "$OUT/steps.tsv"
  case "$key" in
    text:*) $TMUX send-keys -l "${key#text:}";;
    wait:*) sleep "${key#wait:}";;
    click:*) c="${key#click:}"                              # click:<col>,<row> — 1-based cells, SGR press + release
      $TMUX send-keys -l "$(printf '\033[<0;%d;%dM\033[<0;%d;%dm' "${c%,*}" "${c#*,}" "${c%,*}" "${c#*,}")";;
    wheel:*) c="${key#wheel:}"                              # wheel:<col>,<row> — one notch up (into the scrollback)
      $TMUX send-keys -l "$(printf '\033[<64;%d;%dM' "${c%,*}" "${c#*,}")";;
    *) $TMUX send-keys "$key";;
  esac
  sleep "${STEP_SECS:-0.7}"
  if [ -n "${PERF_DUMP:-}" ]; then $TMUX capture-pane -pN > "$OUT/$(printf '%02d' "$step")-${label// /_}.txt"; fi
done < "$SCENARIO"
printf '%s\tend\t\n' "$(now_us)" >> "$OUT/steps.tsv"
$TMUX send-keys C-q; sleep 0.3; $TMUX send-keys q; sleep 0.3; $TMUX send-keys y; sleep 0.5

python3 "$HERE/report.py" "$OUT" | tee "$OUT/report.txt"
echo "perf: $OUT"
