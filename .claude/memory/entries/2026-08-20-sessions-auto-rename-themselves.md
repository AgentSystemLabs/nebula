# Sessions Auto-Rename Themselves — 2026-08-20

**Asked:** "add some type of hook into nebula and ability for claude to automatically rename the session,
update the system prompt to use the skill to tell nebula to rename the session after the initial prompt
was submitted, we should be able to creat a title between 3-4 words that describe the ask of the promp…"

**Did:** A `UserPromptSubmit` hook injects an instruction telling the agent to run `nebula rename <title>`.
Later extended to codex ("it doesn't seem lke when I send a prompt to codex it updates the session title…
look into how we do it for claude code and replicate that behavior").

**Gotchas:**
- This is why every session in this repo issues a `nebula rename` before doing anything. It is injected
  context, not something the user typed — don't mistake it for part of the request.
