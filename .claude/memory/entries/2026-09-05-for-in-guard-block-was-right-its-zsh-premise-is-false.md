# The `for … in $(…)` GUARD HOOK Block Was Right, But Its zsh Premise Is False — 2026-09-05

**Asked:** "⏺ Bash(echo "--- remote tag 22?"; git ls-remote --tags origin 2>&1 | grep -i '22' ; … gh release view
v0.22.0 2>&1 | hea…) ⎿ Error: PreToolUse:Bash hook error: [python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/guard.py"]:
[nebula guard:for-in-unquoted-command-substitution] Blocked: the harness shell is zsh, which does not word-split
an unquoted … debug why i saw this error on a claude prompt"
→ refined: Find out why the GUARD HOOK (`.claude/hooks/guard.py`) blocked a Bash call in a Claude SESSION
with its `for-in-unquoted-command-substitution` rule when the visible part of the command has no
`for … in $(…)` in it. Recover the full command from that session's transcript, run the rule against it,
and tell me which text matched. If the rule matched something that is not a `for … in $(…)` loop, tighten
it so this command passes while the real pattern still blocks (with a regression case in the hook's
tests, if it has any); if the truncated tail really held the loop, say so and change nothing.

**Did:** The prompt came under a pasted Claude Code Bash call, collapsed to `echo "--- remote tag 22?"; git
ls-remote --tags origin … gh release view v0.22.0 2>&1 | hea…`, with the GUARD HOOK's
`for-in-unquoted-command-substitution` block beneath it. Diagnosis only — `guard.py` untouched. The other Claude SESSION's CLAUDE TRANSCRIPT
(`~/.claude/projects/-Users-webdevcody-Workspace-AgentSystemLabs-nebula/a8ecd569-….jsonl`, 05:18:29Z,
`tool_use.input.command`) holds the whole command: past the `…` it ran `for b in $(git for-each-ref
--format='%(refname:short)' refs/heads refs/remotes); do v=$(git show "$b:Cargo.toml" …); done`. Fed back
through the hook, the full command exits 2 with match `for b in $(`, the visible head alone exits 0, and
the same session's rewrite five seconds later (`git for-each-ref … | while IFS= read -r b; do …`) exits
0 — the block was correct and the display hid the fragment. Then checked the rule's premise in the harness
shell itself (`/bin/zsh` 5.9, `[[ -o shwordsplit ]]` → no): a backtick or `$(…)` loop over
`printf "a b\nc\n"` prints `[a] [b] [c]`, only `x=$(…); for f in $x` yields one word. Pulled the
tool_result of each loop the rule cites: 2026-08-25 `for f in $(git diff --name-only)` printed 13 per-file
`N hunks  path` lines; 2026-08-26 `for f in $(git diff --name-only); do cp "$f" "$W/$f"` printed
`IDENTICAL diff stat` and 19 files in the RELEASE WORKTREE; 2026-08-28 `for f in $(…); do cmp -s "$f"
"$W/$f" || echo "DIFFERS: $f"` printed only `checked` — 37 matches, not 37 silent failures. Corrected the
gotcha bullets of the 08-26 and 08-28 release entries in place, folded the truth into the RELEASE SKILL
`$VAR` standing gotcha, and left the decision to retire or reword the rule to the user.

**Gotchas:**
- Claude Code shows a long Bash call as its first line plus `…`, and the GUARD HOOK's block names the
  rule, not the fragment it matched — so a correct block on a `for b in $(…)` at the tail reads as a false
  positive. Recover the command from the CLAUDE TRANSCRIPT (`grep -l '<head text>' ~/.claude/projects/<slug>/*.jsonl`,
  then the `tool_use` block's `input.command`) or run the rule's regex over it before touching the rule.
- zsh 5.9 word-splits an unquoted `$(…)` and backtick substitution on `IFS` exactly like bash; only
  parameter expansion `$VAR` stays one word while `SH_WORD_SPLIT` is off. The rule's message, its source
  comment and the two release gotchas conflated the two. The residual reason to prefer
  `| while IFS= read -r f` is a path with a space or glob character, not zsh.
- A `cmp -s a b || echo DIFFERS` loop that prints nothing has *passed*: a glued or missing path makes `cmp`
  exit 2 and the `||` branch prints. The 08-28 re-hit read silence as failure; before recording that a loop
  "silently did nothing", check that its failure branch would have said so.
- Past gotchas are checkable: the CLAUDE TRANSCRIPT (`~/.claude/projects/<project-slug>/<session>.jsonl`, a
  WORKTREE session under its own slug) holds the `tool_result` of the very call an entry blames next to its `tool_use`.
  Three cited failures, three recorded successes.
