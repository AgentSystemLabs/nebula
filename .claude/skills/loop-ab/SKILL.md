---
name: loop-ab
description: "Measure whether the SELF-IMPROVING LOOP (skills, hooks, CLAUDE.md, AGENTS.md, TERMS.md, MEMORY LOG) earns its tokens on this repo: two shallow clones of main, one as-is and one stripped bare, the same plain-language feature prompt run headless in each with claude -p, then tokens, cost, time, turns, the cargo gates, two blinded reviews with swapped labels, and an HTML report the user opens. Use when the user says \"a/b test the skills\", \"rerun the a/b\", \"loop a/b\", \"are the skills worth it\", \"do the skills help or just use tokens\", or asks to measure the memory or the skills against a bare run."
user-invocable: true
---

The first run of this harness (2026-09-06, MEMORY "A/B Test Of The SELF-IMPROVING LOOP…") priced the loop
at about 40% more cost and 20% more time for a modest quality edge, from one sample. Every rerun adds a
sample; two or three on features that touch areas the MEMORY LOG already covers is what settles it.

## What you produce

An A/B directory outside nebula's worktrees — `~/Workspace/AgentSystemLabs/nebula-ab-test-<date>/` —
holding `a-with-skills/`, `b-bare/`, `feature-prompt.md`, `out/` and `out/report.html`, opened for the
user with `open` and published as an Artifact. Nothing in this repo changes except the MEMORY LOG.

## Steps

1. **Pick the feature** (or take the user's). One larger, self-contained feature that spans the DAEMON,
   the protocol and the TUI, written in plain language with no TERMS in caps — arm B has no glossary.
   `templates/feature-prompt.example.md` is the shape: context the agent cannot guess, numbered wants,
   "work autonomously, leave the changes uncommitted, reply with a summary". Save it as
   `$AB/feature-prompt.md`. Never reuse a feature a previous run built.
2. **Clone.** `git clone --depth 1 --branch main file:///<repo> $AB/a-with-skills` and the same for
   `b-bare` — the `file://` form, or `--depth` is ignored. In `b-bare` delete `.claude/`, `CLAUDE.md`,
   `AGENTS.md`, `TERMS.md`, `.cursor/rules/`, drop `memory-check` / `recall-eval` / `terms-check` from the
   Makefile's `ci` target and `.PHONY`, and commit, so both trees start clean and the diff is the agent's.
3. **Pre-warm both**: `cargo build --workspace --all-targets` and `cargo clippy --workspace --all-targets`
   in each clone, output redirected to a log (the GUARD HOOK refuses a piped cargo). Neither arm pays a
   cold build.
4. **Probe once** that a headless run loads the loop: in `a-with-skills`,
   `claude -p "list the skills the Skill tool offers, and say whether a [nebula recall] block was
   injected" --model claude-haiku-4-5-20251001 --output-format json --permission-mode auto`. If the nine
   repo skills are missing, stop — the experiment is invalid.
5. **Run both arms**, detached and in parallel:
   `AB=$AB nohup scripts/run_arm.sh $AB/a-with-skills a > /dev/null 2>&1 &` and the same for `b`.
   Do not use `--dangerously-skip-permissions` (the classifier blocks it, and `--permission-mode auto`
   is what interactive sessions run under anyway). Wait on the `out/<label>.finished` markers with a
   background `until` loop; a Monitor or background task does not survive a session restart, the
   `nohup`'d runs do. A `pkill`ed first launch leaves stale markers — `rm` them.
6. **Measure.** `scripts/analyze_stream.py <label> out/<label>.stream.jsonl out/<label>.wall_seconds >
   out/<label>.summary.json` for each arm, then `scripts/phases.py $AB/out`. Per-call input and cache
   tokens in the stream are exact; output tokens exist only in the final `result` record.
7. **Gate.** `scripts/audit_arm.py <arm-dir> <label> $AB/out` for A then B, sequentially (the E2E PTY
   suite spawns daemons): it writes the code and full diffs, runs `cargo fmt --check`,
   `clippy -D warnings`, the build and `cargo test --workspace`, and reruns once on the cold-exec flake.
8. **Blind-review.** Copy the two code diffs under neutral names twice with the labels swapped
   (`r1-X` = A, `r1-Y` = B; `r2-X` = B, `r2-Y` = A) and spawn two `general-purpose` subagents, each given
   `templates/review-rubric.md`, its two diffs, the feature prompt and this checkout as the read-only
   base. They write `out/review/r1.json` and `r2.json`; assemble `out/review.json` as
   `{"reviews": [{…, "mapping": {"X": "A", "Y": "B"}}, {…, "mapping": {"X": "B", "Y": "A"}}]}`.
9. **Read both diffs yourself** for the design facts the reviewers will argue about (protocol bump,
   migration, poll interval, dedup, module split) and take each arm's real closing summary into
   `out/<label>.closing.txt` — in arm A the last `==== OVERVIEW ====` block, since the SKILL AUDIT
   HOOK's continuation is what lands in `result`.
10. **Report.** Write `out/narrative.json` (`title`, `date`, `tone` A|B|tie, `headline`, `lead`,
    `findings[]`, `recommendation`; HTML allowed) from the numbers, then
    `scripts/make_report.py $AB`, `open $AB/out/report.html`, and publish it as an Artifact. Say in the
    reply where the cost difference went, what the loop visibly changed in the code, what the bare arm
    did better, and that one run is one sample.

## Reading the result

- The scaffolding's per-call context is cheap (cache reads); the money is in extra *calls* — the
  closing chores and the prompt rewrite. Look at `phases.json`, not the raw token totals.
- KEEP MODULES SMALL is the rule the bare arm cannot know; expect the reviewers to dock it for that.
  Say so in the report's caveats rather than hiding it.
- Both arms lean on the same user-level skills (`claude-api` prices the models identically); only the
  repo layer is the variable.
