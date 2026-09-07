# PR #32 — A quick prompt's new worktree and its session appear the moment Enter is pressed

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/32
- **Author:** @webdevcody
- **Merged:** 2026-09-07T02:24:28Z by @webdevcody (`6bcdcf71a548`)
- **Opened:** 2026-09-07T01:34:51Z
- **Branch:** `rustic-orbit-rambles` → `main`
- **Diff:** +1192 −123 across 16 file(s)

## Description

> With HIDE ROOT WORKTREE on, `p` on the WORKTREES PANEL showed nothing for the seconds git spent cutting the checkout and the CLI spent booting; now the new WORKTREE row and its SESSION row are on screen the moment Enter is pressed, and the DAEMON's answers slide in underneath them.
>
> **Contents:** [1. The problem](#user-content-1-the-problem) · [2. What changed](#user-content-2-what-changed) · [3. How it looks](#user-content-3-how-it-looks) · [4. How it works](#user-content-4-how-it-works) · [5. Risk](#user-content-5-risk) · [6. Technical overview](#user-content-6-technical-overview) · [7. Notes](#user-content-7-notes)
>
> ## 1. The problem <a id="1-the-problem"></a>
>
> A QUICK PROMPT into a WORKTREE that does not exist yet is two DAEMON round-trips: a `CreateWorktree` (a fetch of `origin/HEAD`, then `git worktree add`) and, once that is acked, a `CreateAgent` (a CLI spawn). For those seconds neither the WORKTREES PANEL nor the SESSIONS PANEL showed anything new, so Enter read as dropped — and then two rows appeared at once, already selected, with no sign of where they had come from.
>
> > "right now there is a few second delay before the worktree or session even shows up in the list, it would be nice if they showed up instantly and hooked into the real directories or sessions later after they actually get created. on failure, just undo the optimistic updates."
>
> ## 2. What changed <a id="2-what-changed"></a>
>
> - **Both rows appear on Enter.**
>   - The WORKTREE row carries the BRANCH NAME GENERATOR name the box offered.
>   - The SESSION row carries the default `agent-1` name and the launch's AGENT KIND.
>   - The cursor lands on them exactly as the real create leaves it; FOCUS stays on the panel `p` was pressed in.
>   - The TERMINAL PANE shows `starting session…` for the stand-in, as it does for any booting session.
> - **They read as pending.**
>   - A hollow `○` dot and no status sweep, on both rows.
>   - ` creating` where the worktree's ago label sits; ` starting` where the session's harness badge sits.
> - **The DAEMON's answers slide in underneath.**
>   - The `CreateWorktree` Ack turns the stand-in checkout into the real one and moves the session row under it.
>   - The `CreateAgent` Ack turns the stand-in session into the real one and attaches it.
>   - No row jumps in RECENCY ORDER: the stand-in carries the same stamp the DAEMON gives a fresh row.
> - **A refusal undoes it.**
>   - A refused checkout takes both rows down and puts the cursor back where it was before `p`.
>   - A refused session takes only its row down; the checkout is real and stays.
>   - The box comes back with the typed text either way — as before.
> - **Nothing reaches the DAEMON under a stand-in id.**
>   - Attaching, typing into the pane, the PREWARM POOL and a WORKTREE DELETE all stop at a stand-in.
>   - `n`, `p`, `t` and `d` on one flash `worktree is still being created`.
> - **Unchanged.**
>   - `p` on a selected checkout is one `CreateAgent`, as before.
>   - QUICK PROMPT FOCUS still decides on the Ack whether the pane is entered.
>
> ## 3. How it looks <a id="3-how-it-looks"></a>
>
> | The first seconds: stand-in rows, git still cutting the checkout | Settled: the DAEMON's rows in the same places |
> |---|---|
> | ![Right after Enter: a hollow-dot worktree row reading "sunny-s… creating" and a session row reading "agent-1 starting", both selected, the pane saying "starting session…"](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/rustic-orbit-rambles/quick-prompt-stand-in.png) | ![After the Acks: the same rows now real — "amber-w… just now" and "agent-1 just now claude" — selected in the same places](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/rustic-orbit-rambles/quick-prompt-landed.png) |
>
> ## 4. How it works <a id="4-how-it-works"></a>
>
> ```mermaid
> sequenceDiagram
>   participant U as User
>   participant T as TUI
>   participant D as DAEMON
>   U->>T: Enter in the QUICK PROMPT (a new WORKTREE)
>   Note over T: stage(): stand-in WORKTREE + SESSION rows,<br/>cursor on them, pane reads "starting…"
>   T->>D: CreateWorktree — its PENDING INTENT carries both ids
>   alt the checkout is cut
>     D-->>T: EntityUpserted(Worktree), then Ack
>     Note over T: resolve_worktree(): stand-in becomes the real row,<br/>session row re-homed under it
>     T->>D: CreateAgent — its intent carries the stand-in session
>     alt the CLI spawns
>       D-->>T: EntityUpserted(Agent), then Ack
>       Note over T: resolve_agent(): stand-in becomes the real row, Attach
>     else refused
>       D-->>T: Error
>       Note over T: discard_agent(): session row down, box back
>     end
>   else refused
>     D-->>T: Error
>     Note over T: discard(): both rows down, cursor back, box back
>   end
> ```
>
> ## 5. Risk <a id="5-risk"></a>
>
> **Verdict:** 🟢 Low risk — client-only rows, every DAEMON-facing path checks for a stand-in first, and the full suite is green.
>
> | | Level | Why |
> |---|---|---|
> | 🔒 **Security & production** | Low | No new `ClientRequest` and no PROTOCOL VERSION change. The stand-in ids are client-made ULIDs that never leave the TUI: attach, the pane's input, the prewarm, a launch, a terminal and a delete all refuse one. |
> | ⚡ **Performance** | Low | `is_placeholder_*` scans `app.pending` — a handful of in-flight intents — once per drawn row and once per keystroke into the pane. The event drain and the PTY byte path are untouched. |
> | 🧩 **Fit with the codebase** | Low | The same PENDING INTENT idiom WORKTREE DELETE's optimistic rollback uses, in its own module. One departure: the stand-in's mark is the intent itself, not a separate set to keep in sync. |
>
> **Rollback:** `git revert <merge>` undoes all of it — no PROTOCOL VERSION bump, no store migration; the images on `pr-assets` stay.
>
> ## 6. Technical overview <a id="6-technical-overview"></a>
>
> - **Mechanism.** `PlaceholderRows { worktree, agent }` rides `PendingIntent::LaunchInCreatedWorktree`, and the `CreateAgent`'s `AttachCreatedWithCloudRetry { placeholder }` carries the session id on (via `AgentLaunchDraft.placeholder`). `App::is_placeholder_worktree` / `is_placeholder_agent` / `is_placeholder_session` scan `app.pending`, so the Ack or Error that removes the intent is what un-marks the row. `stage` pushes real `Worktree` / `Agent` entries into `app.tree` — an empty `path`, `status_changed_at = now_ms()` to match the DAEMON's stamp — so every list, sort and count treats them as it would the real ones; `resolve_*` drop the stand-in when the upsert arrived first and rename it in place when the Ack did.
> - **Files.** `crates/nebula-tui/src/event_loop/placeholder.rs` — stage / resolve / discard, and six tests; `crates/nebula-tui/src/event_loop/quick_launch.rs` — stages before the `CreateWorktree`, resolves on its Ack; `crates/nebula-tui/src/event_loop.rs` — `attach_created`, `reopen_prompt_with`, and the guards in `send_attach`, `release_attachment`, `create_agent`, `create_terminal`, the prewarm and the three `Input` sites; `crates/nebula-tui/src/app.rs` — `PlaceholderRows`, the intent fields, the predicates; `crates/nebula-tui/src/ui.rs` — the pending look.
> - **Why not a separate placeholder set on `App`.** It would have to be cleared on exactly the Ack or Error that removes the intent — the intent already is that record, and a set would drift from it.
> - **Two ordering traps the tests caught.** The intent is allocated by `send_with` *after* `stage` runs and removed by the Ack arm *before* the intent runs, so in both windows a stand-in looks real: `stage` builds the pane's `AttachedTerm` by hand (an `attach_now` sent an `Attach` for a made-up id), and `create_agent` takes the name off the stand-in row (`default_session_name` counted it as taken and named the real create `agent-2`).
> - **Gate.** `make ci` green — fmt, clippy `-D warnings`, the memory / recall / terms checks, and 613 nebula-tui + 196 daemon + 29 e2e tests.
>
> ## 7. Notes <a id="7-notes"></a>
>
> - Level with `origin/main`; merges clean.
> - Screenshots from the SCREENSHOT HARNESS: the new `quick-prompt-stand-in` scene, run with `NEBULA_SHOT_SLOW_GIT_SECS=6` — a new env the harness's stand-in `git` reads so `worktree add` stays open long enough to capture — and `quick-prompt-landed`, the same keys settled.
> - Also lands three skill-audit edits: PROJECT TERMS' sightings grep now matches code spellings, NEBULA-MEMORY reads `retire:` tails only when enforcement was built, and PROMPT DADDY's worked examples are cut.
> - MEMORY LOG entry: `.claude/memory/entries/2026-09-06-quick-prompt-new-worktree-and-session-rows-show-up-at-once.md`.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_01ATGUFbYKgB5j7uvZWEa1Wh

## Changed files (16)

- `.claude/MEMORY.md` +1 −0
- `.claude/memory/entries/2026-09-06-quick-prompt-new-worktree-and-session-rows-show-up-at-once.md` +67 −0
- `.claude/memory/gotchas.md` +4 −4
- `.claude/skills/nebula-memory/SKILL.md` +3 −1
- `.claude/skills/project-terms/SKILL.md` +6 −1
- `.claude/skills/prompt-daddy/SKILL.md` +0 −34
- `TERMS.md` +5 −3
- `crates/nebula-tui/src/app.rs` +74 −1
- `crates/nebula-tui/src/event_loop.rs` +186 −61
- `crates/nebula-tui/src/event_loop/placeholder.rs` +708 −0
- `crates/nebula-tui/src/event_loop/quick_launch.rs` +38 −8
- `crates/nebula-tui/src/quick_prompt.rs` +6 −0
- `crates/nebula-tui/src/ui.rs` +47 −10
- `scripts/shot/bin/git` +18 −0
- `scripts/shot/scenes/quick-prompt-landed.keys` +14 −0
- `scripts/shot/scenes/quick-prompt-stand-in.keys` +15 −0

## Commits (3)

- `f61430740f0b` A quick prompt's new worktree and its session appear the moment Enter… — @webdevcody, @claude
- `da518c8711da` Screenshot harness: a slow-git stand-in and the two quick-prompt scen… — @webdevcody, @claude
- `bf8e11bf3ada` Record the slow-git screenshot trap in the MEMORY LOG entry — @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
