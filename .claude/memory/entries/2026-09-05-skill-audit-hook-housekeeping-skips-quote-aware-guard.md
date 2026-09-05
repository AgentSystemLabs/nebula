# A SKILL AUDIT HOOK On Stop, Housekeeping Skips For PROMPT DADDY And NEBULA-MEMORY, A Quote-Aware GUARD HOOK — 2026-09-05

**Asked:** "ok fix up the skills based on your recommendation, but also look at the last 10 conversations
to try and improve the ones that need help. then add a hook that runs at the very end which will self
improve all of my skills based on where it thinks my skills are too verbose and not adding real value. at
the end you can suggest to users woth AskUserQuestion tool to create a new skill or make tweaks based on
your findings"
→ refined: Three things, in order. (1) Apply the skill tweaks recommended earlier in this session: PROMPT
DADDY gets an explicit rule for unattended sessions (settle an alias with one `gh` or grep before asking;
fill with a stated assumption when no user can answer) and a firm cap on the REFINED PROMPT length; the
NEBULA-MEMORY SKILL's "prune before you add" gets a concrete recipe; run the STANDING GOTCHAS pruning pass
now (retire enforced lines, merge duplicates); fix `recall.py`'s `maxsplit` warning. (2) Read the ten most
recent Claude transcripts for this project (excluding this session), measure each skill (invocations,
corrections it drew, reply length, sections followed) and tighten the skills that measure badly. (3) Add
a `Stop` hook, `.claude/hooks/skill_audit.py` registered in `.claude/settings.json`, that once per closed
task (a NEBULA-MEMORY SKILL invocation since the last audit, plus a per-project cooldown) blocks the stop
with instructions for the agent to audit the skills it used for verbose, no-value sections and propose
cuts or a new skill through one `AskUserQuestion`, applying only what I pick, and writing a report file
instead when no user is present. (Assuming "my skills" means the seven under `.claude/skills/`, and "at
the very end" means the end of each task's turn, since a `SessionEnd` hook cannot act.) Finish by asking
me with `AskUserQuestion` which findings to turn into new skills or tweaks. (no questions asked)

**Did:** *Measured ten transcripts* (`~/.claude/projects/<project>/*.jsonl`, a scratchpad `analyze.py`:
typed prompts are `type=="user"` + `promptSource=="typed"` + `origin.kind=="human"`, skill use is a
`tool_use` named `Skill`, a shaped reply contains `==== OVERVIEW ====`). Findings: OUTPUT DOCTOR replies
ran 550–1,770 words; PROMPT DADDY asked zero `AskUserQuestion`s in ten sessions; five of the ten were
landing chores ("fix conflicts on the pr then fix comments, baby sit until all passing then merge" ×3,
"commit push and merge", "commit and push to main, skip all skills"), and the three-word one spent more
than half a 17-minute turn in the loop and drew "why did this take so long to do...?"; the GUARD HOOK
fired nine times (piped-cargo 5, bare-equals 3, for-in 1). *Skills:* PROMPT DADDY
(`.claude/skills/prompt-daddy/SKILL.md`) — a housekeeping skip ("commit push and merge" is taken
literally), the unattended rule extended to a harness that says the user is not watching, a "one check
beats one question" step, and a counted 100-word cap; NEBULA-MEMORY SKILL — the same housekeeping skip
and a four-step prune recipe (enforced → twins → migration-era → change-specific detail); OUTPUT DOCTOR —
a *Budget* section (task reply < 350 words, question < 150, OVERVIEW < 120, DETAILS ≤ 5 bullets);
`CLAUDE.md` / `AGENTS.md` carry the skips and the third hook. *SKILL AUDIT HOOK:*
`.claude/hooks/skill_audit.py` on `Stop` (`.claude/settings.json`): fires when the transcript holds more
`nebula-memory` invocations than its per-session state recorded, outside a per-project cooldown
(`NEBULA_SKILL_AUDIT_COOLDOWN_MIN`, default 0 — the user chose every closed task), never while `stop_hook_active`; returns
`{"decision":"block","reason":<brief>}` listing the skills invoked with their line/word counts and the
audit questions, capped at three proposals put through one `AskUserQuestion` (unattended: a report under
`.claude/memory/skill-audit/`); state in `/tmp/nebula-skill-audit-<uid>/`; `NEBULA_SKILL_AUDIT=off`;
`--dry-run <transcript>`; nine stdin probes cover every gate. *GUARD HOOK:* the four command-phrase rules
(`QUOTE_BLIND`) now match with quoted strings blanked, the path rules keep them
(`"$HOME/.cargo/bin/nebula"` still blocks); `probe_guard.py` (scratchpad) runs 15 trap/remedy pairs green;
this retires the `CARGO_INSTALL_PATH` gotcha. `recall.py::resolve_targets` passes `maxsplit=1`. *STANDING
GOTCHAS* 300 → 294 + 2: deleted the enforced `CARGO_INSTALL_PATH` line and the migration-era "session
still holding the old skill" line, merged MAKE CYCLE's two lines, RELEASE SKILL's two `CARGO_TARGET_DIR`
lines and SHARED CHECKOUT's two "other sessions' changes" lines, and retired the TUI `Terminal::clear`
line with `the_loop_never_calls_terminal_clear` (`crates/nebula-tui/src/event_loop/host_terminal.rs`,
`include_str!` over the loop's three files; nebula-tui host_terminal tests green, fmt and clippy clean).
*Left to the other live session* (`184f2d12`, whose third prompt is "the terms are only supposed to be
related to product terms…"): the PROJECT TERMS scope change — `TERMS.md` was dirty under its hands the
whole time. *Then, from the `AskUserQuestion` (all four picked, cadence "every closed task"):* the hook's
`DEFAULT_COOLDOWN_MIN` is 0; a **`land` skill** (`.claude/skills/land/SKILL.md`, ~90 lines) encodes the
landing recipe from the PR #28/#29 entries — three shapes (dirty SHARED CHECKOUT on `main`; a branch
with an open PR to babysit; "make pr"), MAKE CI first, explicit-path `git add`, `git commit -F`, `gh pr
create --body-file`, `git merge-tree --write-tree --name-only` before touching a tree, the squash-pre-landed
resolution, inline-comment replies via `…/comments/<id>/replies`, `gh pr checks --watch --interval 10`,
`gh pr merge --merge`, `git merge --ff-only origin/main` — and stands in for PROMPT DADDY, NEBULA-MEMORY
and PROJECT TERMS on that prompt (`CLAUDE.md` table row, `AGENTS.md`, PROMPT DADDY's skip list, a new
skip paragraph at the top of PROJECT TERMS); **OUTPUT DOCTOR's worked examples cut 8 → 3** (kept the pure
question, the bug fix, the blocked task; 527 → 321 lines — reverses the 2026-09-04 "five examples"
decision on the user's pick); the **SCREENSHOT HARNESS is committed**: `make shot [SCENE=] [KEYS=]` →
`scripts/shot/shot.sh` builds, makes a demo repo with two worktrees, exports the isolation env, puts
`scripts/shot/bin/gh` (a Python stand-in answering `pr list` / `pr view [n]` / `pr diff n` / `api user`
from `scripts/shot/fixtures/`) first on PATH, `nebula add`s the repo (which spawns the demo daemon from
`current_exe`), drives a private tmux, captures `-epN`, quits the TUI, kills the daemon by pidfile, and
`scripts/shot/render.py` (Pillow in a venv under `$TMPDIR/nebula-shot/venv`, made on first run) turns
the ANSI into `design-screenshots/<scene>.png`. First run: `KEYS="Tab j"` → `OPEN PRS · 3` with #42, #37,
then `#39 Still … draft` dimmed and last, and the SESSIONS PANEL's `↗ #39 … 2 new` PR ROW for the
selected worktree — the visual check the previous task could not make.

**Gotchas:**
- A GUARD HOOK block aborts the *entire* Bash call: a heredoc that rewrote `guard.py` earlier in the
  same call never ran because a probe string later in it tripped a rule, and the next command assumed
  the edit had landed. Keep probes in a script file, and `git diff --stat` before building on an edit
  that shared a call with anything a rule might match.
- Two read-only greps were blocked because their *patterns* named `git stash` and `cargo install
  --path` — the bare-phrase trap the standing gotcha predicted. Fixed with `QUOTE_BLIND`; a rule that
  keys on a *path* must not be quote-blind or `cp x "$HOME/.cargo/bin/nebula"` slips through.
- A source-grep test is part of the source it greps: `include_str!("host_terminal.rs")` matched the
  needle inside the test's own `///` comment. Build needles with `concat!` and keep the literal out of
  the doc comment as well.
- A project `Stop` hook is live in the session that writes `settings.json`, that same turn: with
  `nebula-memory` already run earlier, the SKILL AUDIT HOOK would have fired at the end of the turn that
  created it and doubled this task's own `AskUserQuestion`. Its per-session state was pre-seeded
  (`session-<id>.json` → `{"closes": N}`); test any Stop hook with `stop_hook_active` both ways before
  registering it mid-task.
- `Refined prompt:` blocks are barely greppable in transcripts (one of ten sessions printed the `> `
  quote shape the skill specifies); the OUTPUT DOCTOR's YOU ASKED section is where a refined prompt can
  be read back reliably. A measurement of PROMPT DADDY should key on that.
- `grep` on this machine is `ugrep`: a bounded wide repeat (`-o -E '.{0,40}#39.{0,60}'`) dies with
  "exceeds complexity limits"; use `cut -c` or a python slice over the SCREENSHOT HARNESS's `.ansi`.
- The harness's first screen selects the ROOT WORKTREE, whose branch has no fixture, so the PR ROW is
  empty until `KEYS="Tab j"` moves onto a worktree the `gh` stand-in knows (`pr-view-<dir basename>.json`).
