# Codex Hooks Moved To ~/.codex — 2026-08-22

**Asked:** (follow-on from the Aug 14 codex work — codex sessions still weren't reporting status)

**Did:** `22f1b24` moved codex's MANAGED HOOKS (the CODEX HOOK DIALECT) to `$CODEX_HOME/hooks.json` and started trusting the IDLE PROMPT `idle_prompt`.

**Gotchas:**
- Codex gates hooks behind a trust modal keyed by the **hook file's absolute path**, recorded in
  `~/.codex/config.toml` under `[hooks.state."<abs path>:<snake_case event>:<group idx>:<hook idx>"]` as
  `trusted_hash = "sha256:…"` — **not** a plain sha256 of the command string, so don't try to precompute
  it. A project-local `.codex/hooks.json` therefore re-prompts in every new worktree, and an unanswered
  prompt means the hooks never run at all. `$CODEX_HOME/hooks.json` is a stable path → one approval
  covers everything.
- Codex discards raw stdout from hooks. Context injection only works through
  `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"…"}}`. Claude Code
  accepts that same envelope, so one response body serves both.
- `codex exec` **does** run hooks once trusted, so it's a fast harness — but it can't answer the trust
  modal, so grant trust first with one interactive run.
