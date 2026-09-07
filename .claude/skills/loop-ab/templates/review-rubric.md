# Blinded code review rubric

You are reviewing two independent implementations of the same feature request in the same Rust
codebase (a daemon + TUI for multiplexing AI coding-agent sessions). They are labeled **X** and
**Y**. You do not know and must not guess how either was produced; judge only the code.

Score each arm 1–5 on every criterion, with one sentence of evidence per score that cites a
file and, where possible, a function or line. Then give an overall verdict.

| # | Criterion | What 5 looks like |
|---|---|---|
| 1 | Spec coverage | every numbered item of the request is implemented end to end (daemon aggregation, live updates over the protocol, restart persistence, row readout, worktree/project rollups, breakdown overlay via key + context menu, help overlay, docs, tests) |
| 2 | Transcript parsing correctness | robust to partial/garbage lines, dedupes by `message.id`, tolerates missing usage fields, reads incrementally rather than re-parsing the whole file on every tick |
| 3 | Cost model | pricing table keyed by model id, unknown model → tokens counted and cost reported as unknown, cache tokens priced separately from input |
| 4 | Architecture fit | follows the crate boundaries and existing idioms (protocol messages, store migrations, snapshot/diff pushes, overlay pattern, keymap), bumps the protocol version if the wire format changed, keeps modules small |
| 5 | Persistence & liveness | totals survive a daemon restart and stay live while a session runs, without a busy loop or unbounded re-reads |
| 6 | Tests | unit tests cover parsing, dedup and aggregation with realistic fixtures; existing tests untouched or updated for a reason |
| 7 | Code quality | clear names, no dead code, no unwraps on I/O, no duplicated logic, comments explain *why* |
| 8 | Diff economy | change is as small as it can be for the spec; no unrelated churn, no reformatting noise |

Output **only** this JSON (no prose outside it):

```json
{
  "scores": {"X": {"1": 0, "2": 0, "3": 0, "4": 0, "5": 0, "6": 0, "7": 0, "8": 0},
             "Y": {"1": 0, "2": 0, "3": 0, "4": 0, "5": 0, "6": 0, "7": 0, "8": 0}},
  "evidence": {"X": {"1": "…", "2": "…", "3": "…", "4": "…", "5": "…", "6": "…", "7": "…", "8": "…"},
               "Y": {"1": "…", "2": "…", "3": "…", "4": "…", "5": "…", "6": "…", "7": "…", "8": "…"}},
  "bugs": {"X": ["…"], "Y": ["…"]},
  "winner": "X" | "Y" | "tie",
  "verdict": "two or three sentences on why"
}
```
