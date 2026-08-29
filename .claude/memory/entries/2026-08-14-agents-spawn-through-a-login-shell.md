# AGENTS Spawn Through A Login Shell — 2026-08-14

**Asked:** "it seems like new sessions don't use my ~/.zshrc, verify the do on load"

**Did:** `1344cd6` — AGENTS and TERMINAL SESSIONS spawn through a login shell (the LOGIN SHELL WRAP).

**Gotchas:**
- The LOGIN SHELL WRAP is why `NEBULA_AGENT_CMD` also has to *skip* it: without that, `~/.zprofile` resets PATH and
  the **real** `claude` CLI launches instead of a test STUB AGENT.
