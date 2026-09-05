---
name: transcript-lookup
description: "Pull a past tool call and what it printed out of the CLAUDE TRANSCRIPT (`~/.claude/projects/<slug>/<session>.jsonl`) by a text needle or regex, across this checkout and its worktrees, with `.claude/memory/transcript.py`. Use when the user asks why a Claude session did something, what a truncated Bash call really ran, whether a GUARD HOOK block was a false positive, or to check a MEMORY LOG gotcha against the output the blamed call actually produced. Also use when the user says \"check the transcript\", \"what did that session run\", \"look at the session log\", or \"find that command\"."
user-invocable: true
---

Claude Code shows a long Bash call as its first lines plus `…`, and a GUARD HOOK block names the rule,
not the fragment it matched — so what a SESSION *ran* and what came *back* are questions the screen
cannot answer. The CLAUDE TRANSCRIPT can: every `tool_use` sits next to its `tool_result`.

## Run it

```bash
python3 .claude/memory/transcript.py '<text from the call as displayed>'   # substring, this checkout + worktrees
python3 .claude/memory/transcript.py --regex '<pattern>'                    # e.g. a GUARD HOOK rule's regex
python3 .claude/memory/transcript.py --chars 0 '<needle>'                   # whole results (default 1,200 chars)
```

Each hit prints the session id, timestamp and tool, the full call text, then the result. `--slug` reads
another project's transcripts (a WORKTREE session lives under this checkout's slug plus the worktree
path, which the default prefix match already covers); `--session <id prefix>` narrows to one session.

## Read it

- Pick a needle from the *displayed* head of the call (`echo "--- remote tag 22?"`), not from what you
  guess the tail holds; the hit shows the tail.
- To test a block, feed the printed command back through the hook:
  `echo '{"tool_name":"Bash","tool_input":{"command":"…"}}' | python3 .claude/hooks/guard.py` — or run the
  rule's regex over it with `--regex`. The 2026-09-05 audit found a "false positive" that was a real
  `for b in $(…)` past the `…`, and a rule premise three transcripts contradicted.
- A result that is *silent* is evidence too: check whether the call's failure branch would have printed
  before recording that it "silently did nothing".
- Quote the session id and timestamp in the reply and in the MEMORY LOG entry so the next agent can
  re-run the same lookup.
