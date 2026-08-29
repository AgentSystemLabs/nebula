# NEBULA TUNNEL Reuses A ttyd Already On The Remote Port — 2026-08-28

**Asked:** "when I run nebula tunnel my console says port is already in use... figure out how to refactor
so if nebula is already running, maybe skip certain tasks so it'll just work" (asked: which line →
"port N is not free", i.e. the remote NEBULA BROWSER's clash) → refined: if a ttyd/NEBULA BROWSER already
answers on the remote's 127.0.0.1:N, skip starting a second one and reuse it, keep the session open for
the forward, open the local URL as usual; fresh-start path, loopback-only forward and Ctrl+C/hang-up
teardown unchanged.

**Did:** `crates/nebula/src/tunnel.rs`: new `reuse_existing_ttyd!()` shell fragment spliced into
`REMOTE_SCRIPT` between the install prelude and the `--no-open` version gate: `curl -sI --max-time 2
http://127.0.0.1:$2/ | grep -qi "^server: ttyd"` → print "a nebula browser is already serving on this
host at port $2; reusing it" and `exec sleep 2147483647` so ssh's `-L` has something to reach and
Ctrl+C / hang-up still ends the session. Tests: script-order assertions plus two that run `REMOTE_SCRIPT`
under `sh -c` against a fake ttyd listener (`fake_ttyd`, answers 200 and 401 with ttyd's `server:`
header) and against a free port (falls through to the gate, which a stub `nebula` fails). 17 unit +
3 `tunnel_cli` tests green; also checked by hand against a real `ttyd -c u:p` (401 → reuse). Docs:
ARCHITECTURE.md tunnel paragraph, README `nebula tunnel` block. Rejected: a new `nebula browser` flag
for the probe (would re-trip the version gate on every remote that has today's nebula); auto-picking a
different remote port (the `-L` remote end is fixed before the remote can choose, and a second ttyd
was the thing to avoid).

**Gotchas:**
- ttyd sends `server: ttyd/1.7.7 (libwebsockets/…)` on every response, 401 included, so the header is
  the identity check; `curl -sI` (HEAD) is enough and needs no body.
- With `-tt` the remote's stderr rides the pty into ssh's *stdout*, not the stderr pipe
  `forward_ssh_stderr` reads — the "reusing it" line reaches the console either way, but do not
  expect it in that thread.
- The behavioral tests must set `HOME` to a temp dir: the prelude prepends `$HOME/.local/bin`, and a
  real nebula found there would pass the gate and `exec nebula browser` for real from inside a test.
- `sleep infinity` is GNU-only; `sleep 2147483647` works on macOS, GNU and busybox.
- The installed `~/.cargo/bin/nebula` (0.13.0) still has the old script; the user must reinstall to get
  the reuse (and the shared tree has other sessions' uncommitted protocol changes — see "Shared tree
  races"), so the install was left to them.
