# Finishing The Codex Session's OPEN PRS → Claude SESSION Launch — 2026-08-28

**Asked:** "there is a codex session id of 01a046d2-b160-7800-9af2-1c403d000114 that never finished because I
ran out of credits, please try to load that session and finish up the work on it" (headless — `prompt-daddy`
skipped). The codex session's own prompt: "add the ability for a user to create sessions off of the open PRS
rows, so they can create claude sessions which would already have a system prompt defining all work must be
done on that PR already injected and include pr url".

**Did:** *(Superseded 2026-09-04: the PR SESSION picker offers every AGENT KIND, `CreatePrAgent` carries a `kind`, and `claude_pr_system_prompt` is `pr_scope::rule` — see that entry.)* The transcript is `~/.codex/sessions/<Y>/<M>/<D>/rollout-<stamp>-<session id>.jsonl` (JSONL;
`payload.type` is `message` / `function_call` / `custom_tool_call_output` / `agent_message`; the final
`task_complete` line carried `usage_limit_exceeded`). The feature was already built and green in the SHARED
CHECKOUT when it died: `ClientRequest::CreatePrAgent` (`protocol.rs`, PROTOCOL VERSION 28 → 29), MIGRATION 22
`agents.pr_url`, `store.rs::insert_agent_with_launch_context` / `agent_pr_url`, `registry.rs::
claude_pr_system_prompt` + `validate_pr_url` composed into the one `--append-system-prompt` on every cold spawn
and RESUME, PREWARM POOL adoption bypassed when `pr_url` is set, `server.rs` routing; TUI `n` / `m` /
right-click on a PROJECT OPEN PRS GROUP row → `event_loop.rs::open_pr_agent_picker` (Claude only, in the ROOT
WORKTREE) through the normal MODEL / EFFORT + name flow; README and ARCHITECTURE patched. Left undone and
finished here: `cargo fmt --all`; the simplify review (its three sub-agents died on the limit — re-read the
hunks by hand, nothing to change); the FOOTER and HELP OVERLAY hint for `n` on the PR row
(`ui.rs::draw_footer_bar`, `ui.rs` WORKTREES help group); clippy — `agent_spawn_command_with` grew to 8 args
(`#[allow(clippy::too_many_arguments)]`), plus five pre-existing rustc 1.92 lints in `e2e_tui.rs`,
`e2e_pty.rs`, `config.rs` cleared on the way. `cargo test --workspace`: 705 green. **Not committed** — the
SHARED CHECKOUT also holds other sessions' uncommitted work (LINK removal, CLICK OUTSIDE dismiss, settings memory, the
new skills).

**Gotchas:**
- Codex sub-agent prompts are stored encrypted in the transcript (`spawn_agent`'s `message` is opaque) and
  their results arrive as `agent_message` rows; here all three were just the usage-limit error, so the
  review had to be redone rather than recovered.
- `cargo clippy … | tail -40` hides earlier crates' warnings: the daemon's two only showed on a per-crate
  run. Grep `^warning:` over the full output instead.
- Two pre-existing clippy warnings were left alone: `hooks/mod.rs:194` items-after-test-module and the
  cfg-gated `return copy_via("pbcopy", &[])` in `event_loop.rs` (fixing it means restructuring the
  non-macOS branch).
