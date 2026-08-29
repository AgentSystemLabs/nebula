# Claude Cloud Launch From The Session Picker — 2026-08-24

**Asked:** "ok add in an option so a user can press tab when hovered over the claude option in the new
session harness selection modal, and when they press tab, it should toggle claude cloud which will mean
now when they press enter, it'll show 1 more dialog prompt so a user can type their prompt and then invoke
claude using the --cloud argument, then that will launch claude with --cloud and their prompt, make sure
the prompt word wraps and allows a user to read multiple lines when they are prompting."

**Did:** `ContextMenu::toggle_hovered_claude_cloud` in `crates/nebula-tui/src/app.rs` and the menu key
path in `event_loop.rs` make `Tab` toggle `Claude · cloud`; Cloud bypasses the bare-Claude prewarm and
opens `PromptKind::ClaudeCloudTask`, a wrapped multi-row editor rendered by
`ui.rs::multiline_input_lines` (`Shift+Enter` or legacy-terminal-safe `Ctrl+J` inserts a hard line).
`ClientRequest::CreateAgent` carries
a request-only `cloud_prompt` across protocol v24; `registry.rs::claude_cloud_spawn_command` launches a
fresh `claude --cloud=<task>` and validates the task before persistence. Failed requests reopen the
populated editor, failed synchronous spawns roll back the agent row, and server logs include request and
launch metadata without the task. README documents the flow. Full workspace suite: 563 tests green.

**Gotchas:**
- A Cloud create must never adopt or refill the normal Claude warm slot: that PTY already started bare
  and cannot retroactively receive `--cloud` plus the task.
- Claude 2.1.241 declares `--cloud [description|session_id|url]` as an optional-value flag. Bind the task
  as one `--cloud=<task>` argv item; passing a dash-prefixed task as the next item can turn it into a
  different Claude option.
- The login-shell wrapper puts the task in both its `-c` string and Claude's argv. TUI and daemon both
  reject NUL and tasks over 16 KiB before inserting a row, and shell quoting prevents injection, but the
  task can still be visible to local process inspection — do not put secrets in it.
- The Cloud task is intentionally request-only, not persisted on `Agent`: a later restart follows the
  established local Claude resume/fresh path. Only a synchronous create error retains the in-memory
  draft and reopens it for retry.
- **`claude --cloud=<task>` creates and exits on this account** (verified 2026-08-26, claude 2.1.247): it
  prints `Created cloud session: … / View: https://claude.ai/code/session_… / Resume with: claude --teleport
  session_…` and returns, so the nebula PTY row goes dead. Both "stay attached after create" and
  `claude --cloud <session_id|url>` (live two-way attach) are gated in the binary on the server feature
  flag `tengu_remote_backend`; `claude --cloud session_…` here fails with
  `Error: Attaching to an existing cloud session is not enabled for your account.` Nothing nebula passes
  can unlock it — when the flag lands, the existing spawn becomes a live attached terminal unchanged.
- What works today without a browser: `claude --teleport session_…` (pulls transcript + branch into a
  local session — a snapshot/fork, not a live stream; it refuses a dirty tree with a "Stash changes and
  continue?" prompt, so run it in a fresh worktree) and `claude -p "msg" --cloud session_…` (queue a
  message, no reply). `claude agents --json --all` lists only local background/interactive sessions,
  never cloud ones, so there is no CLI poll for "cloud session finished". The reattach path built on this
  is [Cloud Rows Re-Enter Their Session On Restart]. Teleport was later confirmed **repeatable** — it
  re-pulls newer turns, is idempotent in the same worktree, and leaves the cloud session running — which
  is what [Cloud Rows Mirror Their Session Instead Of Dying At Create] is built on.
