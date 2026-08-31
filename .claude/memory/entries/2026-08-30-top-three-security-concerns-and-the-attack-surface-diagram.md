# Top Three Security Concerns And The Attack Surface Walkthrough — 2026-08-30

**Asked:** "analyze the top 3 security concerns you can find with this app, diagram any attack
surfaces using a mermaid diagram into a .html walkthrough, then open the html in browser"
→ refined: Audit nebula for security weaknesses and report the top 3 you can actually find in the
code — read-only, change no code. Cover the real attack surfaces: the DAEMON SOCKET, the HOOK
RECEIVER and its BEARER TOKEN, AGENT ENV, NEBULA BROWSER (`--bind`/`--public`/`--credential`),
NEBULA TUNNEL, the SQLITE STORE and `install.sh`. Rank by severity with `file:line` evidence and a
concrete exploit path for each. Write it up as a self-contained `.html` walkthrough in the
scratchpad with a mermaid diagram of the attack surfaces, then open it in my browser with `open`.

**Did:** Read-only review of `main` @ `f001890`; no crate touched. Wrote the walkthrough to the
session scratchpad (`nebula-security-walkthrough.html`) and opened it. Ranked findings:
1. **DAEMON SOCKET is an unauthenticated control plane.** `server.rs:37` `handle_client` checks
   only PROTOCOL VERSION, then honours every `ClientRequest`; `server.rs:202`
   `ClientRequest::Input { session, data }` writes raw bytes into *any* PTY with no ownership test.
   Its whole boundary is `runtime_dir()` mode 0700, which on macOS is `/tmp/nebula-<uid>`
   (`paths.rs:6,19` — `XDG_RUNTIME_DIR` is unset there).
2. **One BEARER TOKEN for every AGENT, and `agentId` is never bound to it.** `hooks/mod.rs:205`
   mints one token per daemon boot; `registry.rs:2403-2408` puts that same value in every AGENT
   ENV; `hooks/mod.rs:254-269` authenticates the token and then trusts `query.agent_id` verbatim.
3. **No integrity check anywhere in the install chain.** `install.sh:47-51` curl → `tar -xzf` →
   `install -m 755` with no checksum/signature; `upgrade.rs:101-121` validates the fetched script
   only with `starts_with("#!")` then `upgrade.rs:81` runs `sh` on it; `env.rs:31`
   `NEBULA_INSTALL_URL` overrides the URL unrestricted, and `tunnel.rs:211` `remote_command` sends
   the same URL to be executed on the remote host.
Verified each claim against the cited lines. Verified the page renders by headless-Chrome
`--dump-dom` + `--screenshot` before opening it.

**Gotchas:**
- **`ensure_runtime_dir` (`lifecycle.rs:115`) is symlink- and owner-blind.** `dir.exists()`,
  `fs::metadata` and `fs::set_permissions` all follow symlinks, and nothing compares `st_uid` to
  `geteuid()`. The "same model as tmux" comment is about the *mode*; tmux additionally refuses a
  socket dir it does not own. On macOS the dir sits in world-writable, boot-cleared `/tmp`.
- **The HOOK RECEIVER's crypto is fine and its authorization is the hole** — 32 CSPRNG bytes with
  `subtle::ConstantTimeEq` is exactly right; the defect is that one token speaks for every agent.
  Do not "fix" this by hardening the comparison.
- **`validate_pr_url` (`registry.rs:2868`) accepts any host** — it checks http(s), no whitespace,
  <4 KiB and the substring `/pull/`, nothing else. That string is persisted and re-composed into
  `--append-system-prompt` on every spawn and RESUME (`claude_pr_system_prompt`,
  `registry.rs:2699`), so it is a restart-surviving prompt-injection channel (whitespace-free only).
- **`reparent_agent_to_cwd` (`registry.rs:1577`) is bounded to worktrees of the same project**, so
  forged-`cwd` hooks are a UI-integrity problem, not a path escape. Say so rather than overstating it.
- **`tunnel.rs` quoting is correct** — `shell_single_quote` (which lives in `ssh.rs:78`, not
  `tunnel.rs`) wraps both the install URL and the path. The remote-exec risk is the unverified URL,
  not an argv injection.
- **Mermaid's own HTML node wrapper has `class="label"`.** A page stylesheet with a `.label` rule
  silently restyles every node in the diagram — ours uppercased and shrank them all. Scope or
  rename any generic class on a page that renders mermaid with `htmlLabels: true`.
- **HTML entities inside `<pre class="mermaid">` decode before mermaid parses.** `&lt;uid&gt;`
  reaches mermaid as `<uid>` and, with `htmlLabels: true`, is swallowed as an unknown tag. Use
  literal `{uid}`/parens in labels.
- **`install_prelude!` (`ssh.rs`) pipes `curl -fsSL "$1" | sh` on the remote**, which is exactly what
  `upgrade.rs::stage_script` refuses to do locally (it stages to a file first, with a comment saying
  why a dropped connection must not half-execute). Both are reached from the same
  `upgrade::install_url()`, so `NEBULA_INSTALL_URL` set locally chooses what runs on every remote
  NEBULA SSH / NEBULA TUNNEL cold-starts. Do not "fix" one without the other.
- **SCROLLBACK is never persisted** — `store.rs` has no scrollback table (projects, worktrees, agents,
  terminals, ui_state, todos, workspaces, links, pr_seen only); the 1 MB SCROLLBACK RING is
  daemon-memory only. The BEARER TOKEN is never logged either. So a nebula box's on-disk footprint is
  metadata, CONFIG.JSON and the DAEMON LOG — the secrets on it are the *agents'* (`~/.claude`, git
  tokens, cloud creds), not nebula's.
- **Nebula never passes ssh `-A`** (`ssh.rs:57` `["-t", "--", host, &cmd]`, `tunnel.rs:185-188` `-tt`
  + `ExitOnForwardFailure=yes`) — agent forwarding on a NEBULA TUNNEL is the user's `~/.ssh/config`,
  not nebula.
- **On Linux `runtime_dir()` is `$XDG_RUNTIME_DIR/nebula`** (`paths.rs:15-18`), so a systemd login puts
  the DAEMON SOCKET in `/run/user/<uid>` — owned and 0700, better than the macOS `/tmp` fallback. But
  logind deletes `/run/user/<uid>` on last logout without `loginctl enable-linger`, while the
  DAEMON SETSID'd daemon keeps running: the socket goes away under a live daemon. Unverified on a real
  box — flagged from the code path, not observed.
- Load the mermaid **UMD** build (`dist/mermaid.min.js`) for a page opened over `file://`; verify
  with `--headless=new --dump-dom` and grep for `aria-roledescription="flowchart"` plus a zero
  `Syntax error` count before believing it rendered.
