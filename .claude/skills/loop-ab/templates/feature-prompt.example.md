Add per-session token usage and cost tracking for Claude sessions.

Context: Claude Code writes a JSONL transcript for every session. The daemon already learns each Claude session's transcript path from the hook payloads it receives (the `transcript_path` field; see how the session title code uses it). Every `assistant` record in that file carries `message.model` and a `message.usage` object with `input_tokens`, `output_tokens`, `cache_read_input_tokens` and `cache_creation_input_tokens`. Note that one API response is written as several records that share the same `message.id`, each repeating that response's usage, so a message id must only be counted once.

What I want:

1. The daemon aggregates, per Claude session, the total input, output, cache-read and cache-creation tokens and an estimated USD cost, computed from a small pricing table keyed by model id (put the current public per-million-token prices for the Claude models in it; an unknown model still counts tokens and reports its cost as unknown rather than guessing).
2. The totals stay live while a session runs, reach the TUI through the existing daemon-to-client protocol, and survive a daemon restart.
3. In the TUI, the sessions panel shows a compact readout on each Claude session's row, for example `$0.42 · 12.3k`, and the worktree and project rows show the rolled-up totals of the sessions under them. A new overlay, opened with a key on the focused session and from the session's context menu, shows the breakdown: the four token counts, the cost, and the model. Add the key to the help overlay and to the docs.
4. Unit tests for the transcript parsing, the deduplication and the aggregation; the existing test suite, clippy and rustfmt all stay green.

Work autonomously and do not ask questions. When you are done, leave the changes uncommitted in the working tree and reply with a short summary of what you built and what you verified.
