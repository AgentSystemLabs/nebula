# PR #33 — The loop's ceremony goes opt-in after an A/B priced it, and a loop-ab skill reruns the test

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/33
- **Author:** @webdevcody
- **Merged:** 2026-09-07T02:42:59Z by @webdevcody (`638c160b35de`)
- **Opened:** 2026-09-07T02:31:57Z
- **Branch:** `dapper-panda-escapes` → `main`
- **Diff:** +1109 −43 across 17 file(s)

## Description

> An A/B of the SELF-IMPROVING LOOP priced its per-task ceremony at about 40 % of a feature's cost for a modest quality edge, so PROMPT DADDY, the NEBULA-MEMORY SKILL and PROJECT TERMS now run only when the task calls for them, the SKILL AUDIT HOOK fires weekly, and the harness that measured it ships as the `loop-ab` skill.
>
> **Contents:** [📊 Scorecard](#user-content-scorecard) · [✨ What changed](#user-content-what-changed) · [📸 Screenshots](#user-content-screenshots) · [🧭 Where the money goes](#user-content-where-the-money-goes) · [⚠️ Risk](#user-content-risk) · [🔧 Technical overview](#user-content-technical-overview) · [📝 Notes](#user-content-notes)
>
> ## 📊 Scorecard <a id="scorecard"></a>
>
> One feature (per-SESSION token usage and cost off the CLAUDE TRANSCRIPT, DAEMON to TUI), two shallow clones of `main`, one headless Fable 5.1 run each: arm A with the whole loop, arm B with `.claude/`, CLAUDE.md, AGENTS.md and TERMS.md deleted.
>
> | Metric | A · with the loop | B · bare | Δ (B vs A) | Measured with |
> |---|---|---|---|---|
> | Cost | $21.28 | $15.16 | **−29 %** | `claude -p … --output-format stream-json`, the `result` record |
> | Wall time · API time | 29m 50s · 26m 25s | 24m 11s · 22m 26s | **−19 % · −15 %** | the same record's `duration_ms` / `duration_api_ms` |
> | API calls | 57 | 31 | −26 | assistant records deduped by `message.id` |
> | Closing chores (memory, terms, reply shape, audit) | ≈ $3.2 · 13 calls | — | | `phases.py`, input-side cost by phase |
> | Gates | fmt, clippy, 938 tests green | fmt, clippy, 929 tests green | | `cargo test --workspace` in each clone |
> | Blinded review, two reviewers, labels swapped | 37 · 37 / 40 | 33 · 37 / 40 | one pick for A, one tie | `review-rubric.md`, eight criteria |
> | `prompt-daddy` body | 219 lines | 188 lines | −31 | `wc -l` (the worked examples) |
> | Skill audit cadence | every closed task | weekly | | `skill_audit.py::DEFAULT_COOLDOWN_MIN` |
>
> ## ✨ What changed <a id="what-changed"></a>
>
> - **PROMPT DADDY runs only on a prompt the RECALL HOOK could not settle.**
>   - a word that maps to two TERMS, a spec hanging on one word ("done", "new", "move", "fix it"), a bug with no evidence, a visual ask with no target, a change that never says what stays
>   - a prompt that already names what to change, what to keep and why is worked from as written
>   - the A/B priced a rewrite of a clear prompt at about $2, asking nothing
> - **The NEBULA-MEMORY SKILL writes only when something surfaced.**
>   - a gotcha, a diagnosed cause, a decision not to relitigate, a non-obvious fact about the repo, the DAEMON, the TUI or the hook dialects
>   - a surprise-free code change, however large, gets no entry: the diff and `git log` already record it
> - **PROJECT TERMS runs only when vocabulary surfaced.**
>   - a word the user typed that no TERM row lists, a new name, a rename or a removal
>   - most tasks surface none and skip it
> - **The SKILL AUDIT HOOK fires at most weekly.**
>   - `NEBULA_SKILL_AUDIT_COOLDOWN_MIN` defaults to a week, `0` restores the per-task audit
>   - the clock lives in `/tmp`, so a reboot restarts it
> - **A `loop-ab` skill reruns the experiment.**
>   - "a/b test the skills", "rerun the a/b", "are the skills worth it" clone, strip, pre-warm, run both arms headless, gate, blind-review and render the report
>   - its scripts take the A/B directory as an argument, so nothing is hard-wired to today's run
> - **Unchanged.** OUTPUT DOCTOR, the RECALL HOOK, the GUARD HOOK, KEEP MODULES SMALL and the MAKE CI gates stay exactly as they were: they cost nothing per call, and the rules file was the loop's most visible effect on the code the A/B produced.
>
> ## 📸 Screenshots <a id="screenshots"></a>
>
> Nothing on the TUI changed, so there is no SCREENSHOT HARNESS frame; the picture is the report this PR acts on.
>
> | The A/B report's verdict and scorecard (`nebula-ab-test/out/report.html`, also published as an Artifact) |
> |---|
> | ![The report's verdict banner, the KPI tiles for cost, wall time, API time, API calls, tokens sent and generated, peak context, tool calls and code changed, and the input-side cost by phase chart, arm A in blue and arm B in orange](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/dapper-panda-escapes/ab-report.png) |
>
> ```
> memory-check: ok (index 200/200 lines, gotchas 300/300 lines, 180 entries indexed)
> terms-check: ok
> recall-eval: ok
> ```
>
> ## 🧭 Where the money goes <a id="where-the-money-goes"></a>
>
> ```mermaid
> flowchart LR
>   P([prompt]) --> R["RECALL HOOK"]
>   R -->|"words unsettled?"| PD["PROMPT DADDY · ≈ $2 when it runs"]
>   R -->|"clear prompt"| W
>   PD --> W["the work · $6.36 vs $5.92 input-side"]
>   W -->|"gotcha, cause or decision?"| NM["NEBULA-MEMORY SKILL"]
>   W -->|"nothing surfaced"| OD
>   NM -->|"vocabulary surfaced?"| PT["PROJECT TERMS"]
>   NM --> OD
>   PT --> OD["OUTPUT DOCTOR"]
>   OD --> A["SKILL AUDIT HOOK · weekly"]
>   A --> F([reply])
>   classDef gated fill:#fef3c7,stroke:#92400e,color:#111
>   classDef kept fill:#bbf7d0,stroke:#166534,color:#111
>   class PD,NM,PT,A gated
>   class R,OD kept
> ```
>
> The closing chores were ≈ $3.2 of arm A's $21.28 and 13 of its 57 calls; the prompt rewrite and the glossary and memory reading before the first edit ≈ $2.2; only ≈ $0.7 was A doing more building. The extra context the scaffolding adds to every call cost ≈ $0.13 in total as cache reads.
>
> ## ⚠️ Risk <a id="risk"></a>
>
> **Verdict:** 🟢 Low risk — prose, one Python default and a new on-demand skill; no crate changed.
>
> | | Level | Why |
> |---|---|---|
> | 🔒 **Security & production** | Low | No new surface in the DAEMON or TUI. The `loop-ab` scripts run only when the skill is invoked, act on throwaway clones outside the repo, and launch `claude -p` under `--permission-mode auto`, never `--dangerously-skip-permissions`. |
> | ⚡ **Performance** | Low | Off every hot path; a task now loads fewer skill bodies, and the audit hook returns early for a week. |
> | 🧩 **Fit with the codebase** | Low | The trigger table keeps its shape and gains skip conditions; `loop-ab` follows the `pr-description` layout (`SKILL.md` + `scripts/` + `templates/`); the skills' frontmatter when-clauses match the table, and the SELF-IMPROVING LOOP row in TERMS.md says the same thing. |
>
> **Rollback:** `git revert` of the merge restores per-task ceremony and the old audit cadence; it does not remove the MEMORY LOG entry (its index line stays), the A/B clones and report on disk, or the `pr-assets` image.
>
> ## 🔧 Technical overview <a id="technical-overview"></a>
>
> - **Mechanism.** The gate is the trigger table in `CLAUDE.md`; `AGENTS.md` carries the same protocol in its harness-neutral register (steps 4, and the two after-task steps). Each gated skill's frontmatter description now states the same when-clause, so the Skill tool's listing agrees with the table, and its body's "When to run it" points back at the table. `skill_audit.py::DEFAULT_COOLDOWN_MIN` goes from `0` to `7 * 24 * 60`; the state files and env overrides are unchanged. The `loop-ab` scripts are today's harness with the A/B directory parameterised: `make_report.py <ab-dir>`, `phases.py <out-dir>`, `AB=<dir> run_arm.sh <arm> <label>`; `analyze_stream.py` and `audit_arm.py` already took their paths as arguments.
> - **Files.** `CLAUDE.md` — three trigger rows and the audit paragraph; `AGENTS.md` — the protocol's three steps; `.claude/skills/prompt-daddy/SKILL.md` — frontmatter when-clause and the opening of "When to run it"; `.claude/skills/project-terms/SKILL.md` — frontmatter, the run/skip paragraph, and a rule that a thing outside the repo gets no Candidates row; `.claude/hooks/skill_audit.py` — the default cooldown and its docstring; `.claude/skills/loop-ab/` — `SKILL.md`, five scripts, the review rubric and an example feature prompt; `TERMS.md` — the SELF-IMPROVING LOOP, PROMPT DADDY, PROJECT TERMS and SKILL AUDIT HOOK rows, a LOOP A/B candidate; the MEMORY LOG entry, index line and two standing gotchas on measuring the loop headless.
> - **How the numbers were taken.** Both clones at `ec7c200`, pre-warmed (`cargo build --workspace --all-targets`, clippy), then `claude -p --model claude-fable-5-1 --output-format stream-json --verbose --permission-mode auto --permission-prompts none` on the same prompt, `nohup`'d and run in parallel on one machine, so wall time carries some CPU contention. Per-call input and cache tokens from the stream reproduce Claude Code's reported cost to the cent once output tokens are added. One run per arm: n = 1.
> - **Rejected.** Dropping OUTPUT DOCTOR or the RECALL HOOK: the reply shape is the user's preference and the hook is a cheap push that reached every prompt. Deleting the memory layer outright: a one-task A/B cannot score cross-task recall, which is the layer's whole bet; the follow-up is a rerun on a task the log already covers, which `loop-ab` makes one invocation.
> - **Gate.** `make memory-check`, `make recall-eval` and `make terms-check` green after the merge with `origin/main` (index 200/200, gotchas 300/300). No crate changed on this branch, so `cargo fmt`, clippy and the test suite were not rerun here; the crates are byte-identical to `origin/main`, which CI passed.
>
> ## 📝 Notes <a id="notes"></a>
>
> - `origin/main` (PR #32) was merged into the branch before opening: conflicts in `.claude/MEMORY.md` (both sides' index lines kept) and `TERMS.md` (upstream had promoted the HIDE ROOT WORKTREE candidate; only the LOOP A/B row was kept).
> - Another session cut `prompt-daddy`'s worked examples on `main` the same day, from its own skill audit; the merge converged on the same cut without a conflict.
> - The full experiment stays on disk at `~/Workspace/AgentSystemLabs/nebula-ab-test/` (both clones with their uncommitted feature trees, streams, diffs, reviewer JSON, the report).
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_019dd1iTVfVVssmb6c5B8Mkn

## Changed files (17)

- `.claude/MEMORY.md` +1 −0
- `.claude/hooks/skill_audit.py` +4 −4
- `.claude/memory/entries/2026-09-06-a-b-test-of-the-loop-two-shallow-clones-one-feature-headless.md` +16 −0
- `.claude/memory/gotchas.md` +4 −4
- `.claude/skills/loop-ab/SKILL.md` +69 −0
- `.claude/skills/loop-ab/scripts/analyze_stream.py` +208 −0
- `.claude/skills/loop-ab/scripts/audit_arm.py` +98 −0
- `.claude/skills/loop-ab/scripts/make_report.py` +548 −0
- `.claude/skills/loop-ab/scripts/phases.py` +48 −0
- `.claude/skills/loop-ab/scripts/run_arm.sh` +20 −0
- `.claude/skills/loop-ab/templates/feature-prompt.example.md` +12 −0
- `.claude/skills/loop-ab/templates/review-rubric.md` +33 −0
- `.claude/skills/project-terms/SKILL.md` +8 −5
- `.claude/skills/prompt-daddy/SKILL.md` +5 −2
- `AGENTS.md` +17 −12
- `CLAUDE.md` +11 −11
- `TERMS.md` +7 −5

## Commits (2)

- `d940ad70dda5` The loop's ceremony goes opt-in after an A/B priced it, and a loop-ab… — @webdevcody, @claude
- `f47bf74077a8` Merge remote-tracking branch 'origin/main' into dapper-panda-escapes — @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
