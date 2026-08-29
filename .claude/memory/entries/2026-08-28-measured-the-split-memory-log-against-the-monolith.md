# Measured The Split MEMORY LOG Against The Monolith Across 65 Transcripts — 2026-08-28

**Asked:** "do an analysis on the memory system over the past couple of prompts to compate against the old
memory system approach befeore i refactored how nebula memory works and see if it's "better""
→ refined: Analyse how the refactored MEMORY LOG (index + standing gotchas + entry files, the RECALL HOOK,
the GUARD HOOK, `make memory-check` — commit `bad8ea4`) has actually behaved in the SESSIONS since it
landed, and compare that against the monolithic `.claude/MEMORY.md` the sessions before it used. Ground it
in the transcripts and `git`, not the design intent (assuming "better" means: context cost per prompt,
whether the relevant entries and gotchas reached the agent, gotcha re-hits and GUARD HOOK blocks, merge/CI
friction, maintenance overhead). Change nothing.

**Did:** No code changed. On the follow-up ("remove the mission control related skills regardless") the Mission
Control files left the repo: `git rm` of `.claude/skills/{recall,diagram}`, `.agents/skills/{recall,diagram}`,
`.cursor/skills/{recall,diagram}` and `.mcp.json` (its only server was MC's `recall`); the four `_mcManaged`
groups stripped from `.cursor/hooks.json` (the `_nebulaManaged` ones stay); the `.gitignore` comment reworded.
Kept: the `_mcManaged` coexistence code and tests in `crates/nebula-daemon/src/hooks/installer.rs` (nebula must
not clobber MC groups in a user's global hook files) and the untracked `.claude/settings.local.json`, which is
MC's per-machine config and still names a `recall` server `.mcp.json` no longer defines. A scratchpad script (`metrics.py`, session-specific) walked every
transcript under `~/.claude/projects/-Users-webdevcody-Workspace-AgentSystemLabs-nebula/`, split them into
OLD (52 sessions, 101 prompts, 2026-08-27 → cutover, monolith + the same skill suite) and NEW (13 sessions,
17 prompts, every one with a `[nebula recall]` injection), and measured per session: chars read from
each memory layer, entries opened, first-turn and peak context tokens, minutes to the first work tool,
turns spent between `Skill(nebula-memory)` and `Skill(project-terms)`, RECALL HOOK injections, GUARD HOOK
blocks (`is_error` tool results carrying "hook error"), `make memory-check` failures. Results:
- **Reach.** OLD: 52/52 sessions touched `.claude/MEMORY.md` but read a median 16.8 KB of its 258 KB
  (`head` ×124, `sed -n` ×68, `grep` ×48, `Read` ×8 — never the whole file). NEW: the RECALL HOOK landed on
  20/20 prompts (median 6.9 KB ≈ 1.7k tokens, never in a subagent sidechain); 9/13 sessions then opened an
  entry file (mean 1.7). `gotchas.md` read in 10/13 NEW sessions vs 3/52 OLD; nobody in either era read
  TERMS.md (103 KB) or the index in full — "read in full" in CLAUDE.md is not what happens.
- **Cost.** First-turn context 40.6k → 45.9k tokens (the injection plus CLAUDE.md growth); peak context
  120.8k vs 118.5k (unchanged); minutes to first work tool 0.2 vs 0.3; the NEBULA-MEMORY SKILL write phase is
  4 → 5 turns median, 3.6k → 7.5k output tokens (an entry file + index line + gotcha promotion).
- **Relevance of the 13 injected prompts (hand-scored):** 6 clear hits (subagents → STOP GATE entry; spawn →
  `nebula worktree` entry; AGENT PRESETS; this one), 3 partial, 2 misses where memory held the answer
  ("commit and push and release" got 0 entries; "did the auto name break?" got PR ROW entries instead of the
  08-26 protocol-skew entry), 2 no-entry-exists. Replaying `recall.py` today returns the right entry first for
  both misses: the missing sessions wrote the VERSION SKEW entry and the "still just named agent-1 agent-2"
  alias, and the RELEASE SKILL entries gained TERMS cells — the SELF-IMPROVING LOOP closed both gaps inside a
  day. No standing gotcha carries `re-hit` and no entry carries `Corrections:` yet, so the loop's own
  counters have never fired.
- **GUARD HOOK.** 5 real blocks ever: 3 in the migration session writing the hook, 1 true positive
  (`for f in $(find …)` in a concurrent session, 12 minutes after `af8b032` added the rule), 1 false positive
  (this session's grep whose *pattern* quoted `cargo install --path`).
- **Not Mission Control.** "RECALL HOOK" above is nebula's `.claude/hooks/recall.py`; Mission Control's Recall
  (`mcp__recall__*`, `.claude/skills/recall`) had zero calls in both eras — its last use in this project was
  2026-08-18 — so nothing in the comparison comes from it. The user read "recall" as Mission Control's.
- **Friction.** OLD: 6 entries record `.claude/MEMORY.md` as the file that conflicted on every merge
  (39 modifying commits, 5 merges). NEW: 2 release entries still record index conflicts, both against a
  branch cut before the split; the index is still one prepend-at-top shared file. `memory-check: FAIL`
  seen in 3 sessions, all mid-migration (monolith still 3,551 lines, gotchas at 441); 0 since.

**Gotchas:**
- The GUARD HOOK's `cargo-install-rewrites-in-place` rule (`CARGO_INSTALL_PATH`) matches the phrase anywhere
  outside a heredoc — a `grep 'cargo install --path'` or a `for pat in '…'` list is blocked as if it were the
  install. Rules that key on a bare phrase need a quote-aware match like `COMMIT_MSG_WITH_BACKTICK` has.
- In the transcripts the RECALL HOOK's text is an `attachment` record (`type=="hook_success"`,
  `hookEvent=="UserPromptSubmit"`, `content` starts `[nebula recall]`), and a GUARD HOOK block is a
  `tool_result` with `is_error` and "PreToolUse:Bash hook error" — grepping for `nebula guard:<rule>` also
  matches tool *output* that printed the rule names (my own grep did), so count only `is_error` results.
- Neither era reads the always-loaded layers in full: index reads are `head -80`/grep (median 10.5 KB of
  35.8 KB), TERMS.md is grepped by TERM name. Only the push-based RECALL HOOK reaches every prompt; anything
  that must reach the agent belongs in the injection or the prompt, not in a "read this in full" rule.
- The `re-hit ×N` and `Corrections: N` counters the NEBULA-MEMORY SKILL defines have zero uses after 13
  sessions; the measurement the split was meant to enable has not started.
