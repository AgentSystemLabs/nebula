---
name: output-doctor
description: "Shape every reply to the user into one fixed layout of up to four sections, each opened by a horizontal rule and a markdown heading with its own emoji so a human can tell them apart at a glance — 💬 ==== YOU ASKED ==== (present if and only if a REFINED PROMPT was logged: that prompt, verbatim), 📋 ==== OVERVIEW ==== (always: what happened, a few plain sentences), 🔧 ==== DETAILS ==== (present only when there is a mechanism, a file:line or a gotcha the overview could not carry), 👉 ==== NEXT STEPS ==== (always present, always last: blocking steps first, each marked 🚧 **Blocking:** with the exact command, then the hand-off — commit, PR, a question, a decision — or the single line \"Nothing — this is done.\") — so a reply can be read top-down and stopped at any line. Use before writing the reply that answers or closes any request: features, bug fixes, questions, refactors, releases, recommendations. Also use when the user says \"output doctor\", \"format this\", \"use the output format\", or \"rewrite this in the format\"."
user-invocable: true
---

The reply is the part of the task the user actually reads. Everything else — the grep, the build, the
test run — is scaffolding they see a few lines of. A reply that opens with a file path, or with a
paragraph that mixes the headline with the mechanism, makes the user do the sorting. This skill puts
the sorting on you: what they asked (when it was rewritten), what happened, how (when it matters), then
what is left for them with the blocking steps first — in that order, under fixed headers that look
different from the text around them, so they can stop reading the moment they have what they need.

## When to run it

Right before you write the reply that answers or closes a request — after the work is done, after
`nebula-memory` and `project-terms` have run (their one-line results fold into DETAILS). Every kind of
request: a feature, a bug fix, a question, a refactor, a release, a recommendation, a diagnosis that
changed nothing.

It does **not** govern:

- the one-line "I'll start by…" preamble and the brief progress notes you post mid-task
- an `AskUserQuestion` — its options and previews keep their own shape
- text that lives in files: commits, PR bodies, memory entries, `TERMS.md`, code comments
- a reply that is itself only a question back to the user, or a one-word acknowledgement of a
  mid-task correction you are about to act on

When in doubt, use the format. A reply that is short is fine; a reply that is shapeless is not.

## The format

Four sections in a fixed order. Two are always present (`OVERVIEW`, `NEXT STEPS`); two are conditional
(`YOU ASKED`, `DETAILS`) and are **absent**, not empty, when their condition fails. `NEXT STEPS` is
always last:

```
---

## 💬 ==== YOU ASKED ====

"<the REFINED PROMPT prompt-daddy logged, verbatim — only when one exists>"

---

## 📋 ==== OVERVIEW ====

<what happened, at a glance>

---

## 🔧 ==== DETAILS ====

<the mechanism, the file:line, the gotcha to know now — only when there is one>

---

## 👉 ==== NEXT STEPS ====

1. 🚧 **Blocking:** <a step only the user can take, with the exact command — first, only when there is one>
2. <the hand-off: commit, PR, a question, a decision — or the single line "Nothing — this is done.">
```

The look, which is what lets a human tell the sections apart in a terminal that renders markdown but
carries no color of its own:

- every section opens with a horizontal rule (`---` on its own line, blank lines on both sides) and
  then a `##` heading — the rule is the space, the heading is the weight
- the heading is the section's emoji, then the name between four `=` on each side, exactly as above:
  `## 📋 ==== OVERVIEW ====`. The emoji is the color; keep the same one per section every time so the
  eye learns it. Never a different emoji, never a different count of `=`
- a blank line under every heading, a blank line above every rule
- the blocking steps in `NEXT STEPS` carry `🚧 **Blocking:**` as their lead so they stand out from the
  hand-off below them
- no title above the first rule, no sign-off below the last section — `NEXT STEPS` ends the reply

An offer of follow-up work goes in the last line of `DETAILS`, or of `OVERVIEW` when there is no
`DETAILS` — never in `NEXT STEPS`.

**Which sections a reply has**, by the kind of request:

- a pure question that changed nothing (an explanation, an assessment, "what does X do" — the prompts
  PROMPT DADDY skips): `OVERVIEW` carries the whole answer; `NEXT STEPS` is "Nothing — this is done."
  unless the answer left the user a decision or a question. **Two sections is a complete reply**; add
  `DETAILS` only when there is a `file:line` beyond the answer, and never pad a question into four.
- a task worked from a REFINED PROMPT: `YOU ASKED`, `OVERVIEW`, usually `DETAILS` (code changed, so
  there is a file to jump to), `NEXT STEPS`.
- work started by a confirmation, a mid-task correction, or a skill trigger: `OVERVIEW`, `DETAILS` when
  earned, `NEXT STEPS` — no `YOU ASKED` unless the confirmation picked up a REFINED PROMPT logged
  earlier in the session (see below).

### `## 💬 ==== YOU ASKED ====`

Present **if and only if** a REFINED PROMPT was logged for the work this reply closes — in this turn,
or in the earlier turn that a bare confirmation ("yes do it", "the second one") picked up. Content: that
prompt, verbatim, in double quotes — the text under `prompt-daddy`'s `Refined prompt:` line, stated
assumptions and all. Only the rewrite: not the original beside it, not the questions it asked, no
`→ refined:` prefix. Those belong in the memory entry, not here. If the user corrected the REFINED PROMPT
mid-task, quote it with the correction applied, so the section still shows the request the work
answered.

Absent when `prompt-daddy` skipped itself — a question, a reply to a question you asked, a mid-task
correction, a skill trigger. The message the section would quote is sitting directly above the reply,
and an echo is not a check. Never paraphrase the prompt and never improve it here: this section exists
so the user can check the reply against the request without scrolling up.

### `## 📋 ==== OVERVIEW ====`

Always present. What the change was — or, for a question, what the answer is — at the altitude of a
commit subject line stretched to a short paragraph or a numbered list of at most five items. Rules:

- plain sentences; no file paths, no symbols, no line numbers
- name the TERMS from `TERMS.md` in caps, as `CLAUDE.md` requires
- if code changed, one clause on the gate: "687 tests green" / "the e2e suite was not run because…"
- if anything was left out, blocked, or scaled down, say so **here**, not buried below
- if nothing changed (a question, a diagnosis, a recommendation), open with that: "No code changed."

A reader who stops after this section should know the outcome and *whether* anything still needs them.
*What* they must do — the blocking steps and the hand-off — is `NEXT STEPS`, at the bottom, which is
what is on screen when the reply lands.

### `## 🔧 ==== DETAILS ====`

Present only when there is something `OVERVIEW` could not carry: a mechanism worth a sentence, a
`file:line` the user would jump to, a rejected approach they would otherwise ask about, a gotcha they
must know *now*. Absent for a question whose answer fit in `OVERVIEW` and for a change whose diff says
it all. Err on the side of **too little**: the user can ask for more, and a reply they have to scroll is
a reply they skim. Rules:

- bullets with a bold lead-in, in `OVERVIEW`'s order — but only for the items that have something
  beyond `OVERVIEW`; an item the overview already covered gets no bullet group
- `file:line` references where they help the user jump to the code — clickable, so use the real path
- the mechanism in one or two sentences per item; skip what the diff already shows
- rejected approaches only when the user would otherwise ask "why not X"
- gotchas the user should know about *now* (a behavior that changed, a setting they must flip); the
  rest go to `nebula-memory`, not here
- the one-line results of `nebula-memory` and `project-terms` ("logged the entry", "aliased 'top nav'
  to WORKSPACES BAR, promoted nothing") close the section; when they would be its only content, leave
  the section out — a question logs nothing worth a section
- an offer of follow-up work is the last line

### `## 👉 ==== NEXT STEPS ====`

Always present, always last. Where the user stands now that the reply is done, and what is theirs to
do: the steps the work cannot finish without come first, the hand-off after them. A reader who jumps to
the bottom of the reply finds everything they must do here, and nothing else.

**Blocking steps** lead the list, each opened with `🚧 **Blocking:**`. The test: is there a step you
could not take yourself that the outcome depends on? Then it is a blocking step. Counts:

- a command only they can run — an interactive login, NEBULA KILL / MAKE CYCLE (they take every live
  session down, this one included, so they run from a terminal outside nebula), a call that was denied
  or needs their terminal — offered in the `! <command>` form where its output should land in the session
- a setting they must flip, a process they must restart, a tool they must install or upgrade
- a decision you are blocked on that an `AskUserQuestion` did not settle, stated as the choice to make
- an approval for a destructive or outward-facing step you did not take without it
- a check only they can do that the work cannot be called done without — a manual test in their
  terminal, a look at a screenshot, a review before a merge

The exact command or setting goes in a code block on the step; the *why* is one clause on the step or
lives in `DETAILS`. Two or three blocking steps is typical — a longer list is usually a task you should
have finished.

**The hand-off** follows the blocking steps, or opens the list when there are none:

- a git hand-off: "Good to commit — `git add -A && git commit`", "Ready for a PR from branch X",
  "Nothing to commit"
- a question the user still owes an answer to — restated, not pointed at
- a command to run, in the `! <command>` form when its output should land in the session
- a decision the user must make — the options in one line
- a verification that is theirs but does not gate the work — a look at the result, a manual check
- when there is genuinely nothing left, the single line "Nothing — this is done." — that line alone,
  never an empty section and never a header without a body. This is the one section with an empty-state
  line, because the bottom of the reply is what the terminal shows when it lands

Does not count, and so is not an item:

- an offer of optional follow-up work ("I can also…") — the last line of `DETAILS`; never pad this
  section with offers
- a step you could still take yourself — take it, then reply
- a restatement of what changed, or a gotcha they only need to *know* — `OVERVIEW` and `DETAILS`
  already carry those
- something left out or scaled down — `OVERVIEW` says so; it becomes a step here only if finishing it
  needs them

Shape: a single line, or a numbered list of one to four items in the order to do them, in the user's
terms — the TERMS from `TERMS.md`, not file paths. No prose above the list, no sign-off below it — this
is the end of the reply.

## Worked examples

A recommendation worked from a REFINED PROMPT (the audit that produced this skill, 2026-08-28,
condensed) — all four sections:

```
---

## 💬 ==== YOU ASKED ====

"Survey nebula's whole feature surface and recommend the 3 features that cost the most — lines of
code, protocol/store surface, recorded gotchas in MEMORY.md — relative to what they actually deliver.
For each: what it does, why it's a poor trade, roughly how much code goes away, and what else depends
on it. Recommend only; don't remove anything yet."

---

## 📋 ==== OVERVIEW ====

No code changed. I surveyed the README, CLI, protocol, overlays, per-file line counts and the gotcha
history in `.claude/MEMORY.md`, then ranked features by cost against delivered value. Three stand out:

1. **CLAUDE CLOUD sessions** — ~1,800 lines to fake a live view of something the CLI won't let you attach to.
2. **`nebula browser` + `nebula tunnel`** — ~1,000 lines for a second remote path that duplicates `nebula ssh`.
3. **The PREWARM POOL** — ~800 lines and up to ~1.7 GB of idle `claude` processes to save ~2s on `n`.

Cutting all three removes roughly 3,500 lines, one external dependency, and needs one protocol bump.

---

## 🔧 ==== DETAILS ====

**1. CLAUDE CLOUD sessions**
- *What:* `Tab` on the Claude picker launches `claude --cloud=<task>`; the daemon re-runs
  `claude --teleport=<id>` every 45s to mirror it, because live attach is refused for this account.
- *Why cut:* three protocol bumps, a store migration, three memory entries of CLI workarounds.
- *Goes away:* `crates/nebula-daemon/src/pty/cloud.rs`, ~600 lines of `registry.rs`, ~490 lines of e2e.
- *Dependents:* the multiline prompt editor has only cloud callers — keep or drop.

**2. …**

---

## 👉 ==== NEXT STEPS ====

1. Decide which of the three to cut first; say the name and I'll start with it.
```

The original reply had the same content in prose that opened with a heading and a rationale paragraph;
the user asked for it twice more before it had this shape. Start with the shape.

A DAEMON change the running daemon cannot pick up without the user — the blocking step leads
`NEXT STEPS` (hypothetical, condensed):

````
---

## 💬 ==== YOU ASKED ====

"Make the HOOK RECEIVER accept the Cursor dialect's camelCase `stop` event so Cursor sessions turn
FINISHED instead of sticking on RUNNING."

---

## 📋 ==== OVERVIEW ====

The HOOK RECEIVER now maps Cursor's `stop` to the same FINISHED transition as Claude's `Stop`; 690
tests green and MAKE INSTALL has run. The live DAEMON is still the old build, so nothing changes on
screen until it is restarted — and that restart is yours to run.

---

## 🔧 ==== DETAILS ====

…

---

## 👉 ==== NEXT STEPS ====

1. 🚧 **Blocking:** restart the DAEMON from a terminal outside nebula — it stops every live SESSION,
   this one included:
   ```
   nebula kill && nebula
   ```
2. Good to commit once a Cursor SESSION shows FINISHED in the SESSIONS PANEL.
````

Had MAKE INSTALL been the only step left, the reply would have run it and `NEXT STEPS` would have
opened with the commit hand-off — the blocking marker is for what the user must do, not for what you
could still do.

A pure question — two sections, no `YOU ASKED` because PROMPT DADDY skipped it, no `DETAILS` because
the answer fit:

```
---

## 📋 ==== OVERVIEW ====

No code changed. The DONE BADGE counts UNSEEN SESSIONS — the ones that finished while you weren't
looking — and clears each one the moment you focus it. A FINISHED SESSION you have already looked at
is not counted.

---

## 👉 ==== NEXT STEPS ====

Nothing — this is done.
```

A bug fix worked from a REFINED PROMPT (the iTerm2 dead-mouse fix, 2026-09-04, condensed) — all four
sections; `DETAILS` carries what the diff cannot say (two causes, one rejected call) and closes with the
two skills' one-liners; the hand-off is the commit plus a look that is theirs but gates nothing:

```
---

## 💬 ==== YOU ASKED ====

"A user on iTerm2 had nebula working normally, then no click landed anywhere in the TUI and WHEEL
SCROLL scrolled iTerm's own view instead of the focused SESSION's TERMINAL PANE. Find out why nebula
stops receiving mouse events mid-run and fix it in nebula if the cause is ours; if it is a terminal
setting, name it. Keep DRAG SELECT, CLICK OUTSIDE and the WHEEL SCROLL steps as they are. (Assuming
iTerm2 3.5.x on macOS and a current release; agent unknown.)"

---

## 📋 ==== OVERVIEW ====

Two causes, both ours, both fixed. A panic on any background task handed the HOST TERMINAL back —
mouse off, raw mode off — while the TUI lived on; and iTerm2's Session ▸ Reset (⌘R) clears mouse
reporting without telling the app. The TUI now restores the terminal only for a panic on its own
thread, re-asks the HOST TERMINAL for its modes every 2 s and on resize, and flashes "a background task
crashed" instead of going quiet. nebula-tui 574 passed and clippy is clean — on a copy of HEAD, because
the SHARED CHECKOUT did not compile with another session's half-done change. MAKE INSTALL has run.

---

## 🔧 ==== DETAILS ====

**Two mechanisms, one symptom pair** — the report carried no log to pick one, so both live in the new
HOST TERMINAL module: `install_panic_hook` (`crates/nebula-tui/src/event_loop/host_terminal.rs:111`)
restores only for the loop's thread and counts the rest; `reassert_modes`, in the same file, re-sends
mouse capture, bracketed paste, focus reports and the KITTY PROTOCOL flags on a 2 s tick.

**Why not `Terminal::clear()` on resize** — ratatui's `clear()` asks the terminal for the cursor
position and crossterm gives up after 2 s with an error the loop turned into a fatal exit;
`Terminal::resize` clears without asking.

**Not a setting** — the profile's Mouse Reporting was still on; ⌘R is what turns it off.

Logged the entry (six gotchas); HOST TERMINAL row updated in `TERMS.md`, promoted nothing.

---

## 👉 ==== NEXT STEPS ====

1. Good to commit — `git add crates/nebula-tui/src/event_loop.rs crates/nebula-tui/src/event_loop/host_terminal.rs docs/keys.md && git commit`.
2. To see it, quit and relaunch the TUI — the DAEMON keeps every SESSION, so nothing else restarts.
```

A refactor whose diff says it all (hypothetical, condensed) — `YOU ASKED` because PROMPT DADDY ran, no
`DETAILS` because a move has no mechanism and the file list is the commit's, and the follow-up offer on
`OVERVIEW`'s last line because there is no `DETAILS` to carry it:

```
---

## 💬 ==== YOU ASKED ====

"Move the FOOTER drawing out of the UI module into its own module, KEEP MODULES SMALL style — no
behavior change, the same tests moved with it, nothing else touched."

---

## 📋 ==== OVERVIEW ====

The FOOTER — VERSION NAMEPLATE, key hints, FLASH, the tallies — now draws from its own module, and the
UI module lost exactly that block. Moved, not rewritten: the same functions, the same tests, the same
E2E TUI frames. nebula-tui 570 passed, the count unchanged; clippy clean. The SESSIONS PANEL drawing
is the next candidate if you want the same treatment.

---

## 👉 ==== NEXT STEPS ====

1. Good to commit — `git add crates/nebula-tui/src/ui.rs crates/nebula-tui/src/footer.rs crates/nebula-tui/src/lib.rs && git commit`.
```

A release (v0.21.0, 2026-08-29, condensed) — no `YOU ASKED`, because "commit push release" is a RELEASE
SKILL trigger PROMPT DADDY skips; NEBULA UPGRADE is the hand-off, not a 🚧 step, because the release is
out whether or not the user takes the build:

```
---

## 📋 ==== OVERVIEW ====

v0.21.0 is out: QUICK PROMPT FOCUS, so `p` no longer enters and locks the TERMINAL PANE. Cut from a
RELEASE WORKTREE off `origin/main` with the CARRY SET brought over from the SHARED CHECKOUT; 819 tests
green there, the RELEASE WORKFLOW green on all four targets, the RELEASE NOTES written over the
auto-generated list. PROTOCOL VERSION stays 34, so no NEBULA KILL is needed. The SHARED CHECKOUT is
untouched and still 8 commits behind `origin/main`.

---

## 🔧 ==== DETAILS ====

**The CARRY SET split two ways** — the three TUI files and the README were strictly ahead of
`origin/main`, so a `cp` was right; the three MEMORY LOG files had moved on both sides, so they were
merged with `git diff v0.20.0 -- <paths> | git apply --3way` and two keep-both conflicts resolved
newest-first. `git diff HEAD` was the wrong base: it spans both of today's releases.

**The notes say what the code says** — the REFINED PROMPT called the SETTING "Focus new session"; the
shipped label is "Quick prompt focus" (`crates/nebula-tui/src/config.rs`), so the notes use that.

Logged the entry (six gotchas, the patch-base one folded into the carry STANDING GOTCHA); promoted
nothing.

---

## 👉 ==== NEXT STEPS ====

1. Take the build when you want it: `nebula upgrade` — with live SESSIONS the DAEMON keeps the old
   binary until NEBULA KILL, so run that from a terminal outside nebula.
```

A diagnosis that changed nothing (the VERSION SKEW read, 2026-08-28, condensed) — all four sections,
`OVERVIEW` opens with "No code changed", and `NEXT STEPS` is what the user does with the answer: a
cutover only they can run and a decision, neither marked 🚧, because the diagnosis was the deliverable
and nothing about it waits on them:

```
---

## 💬 ==== YOU ASKED ====

"Several SESSIONS in the SESSIONS PANEL are still named `agent-1`, `agent-2`, `agent-3` after I've
sent Claude a prompt in them, so AUTO-TITLE didn't retitle them. Find out why and tell me before
changing anything — is AUTO-TITLE broken, or is something else (VERSION SKEW, the hook) blocking the
`nebula rename` it runs? (Assuming this is the DEV INSTANCE started with `make dev` at 20:19 today.)"

---

## 📋 ==== OVERVIEW ====

No code changed. AUTO-TITLE is intact; this is VERSION SKEW. The DEV INSTANCE daemon started at 20:19
speaks PROTOCOL VERSION 32, and at 21:09 another session's MAKE INSTALL replaced the `nebula` on PATH
with a v33 build, so every `nebula rename` the agents have run since is refused by the handshake — and
the three untitled rows are exactly the ones created after 21:09. The SQLITE STORE agrees: all three
still carry the pending flag.

---

## 🔧 ==== DETAILS ====

**The tell is the row set, not a log** — the DAEMON LOG records nothing for a refused handshake (the
client refuses before sending); `select name, auto_title_pending, created_at from agents` against the
install's mtime dates the break.

**Why a DEV INSTANCE is exposed** — its daemon runs from `current_exe()` and stays put, but AUTO-TITLE's
bare `nebula rename` follows whatever PATH holds now, so a sibling session's MAKE INSTALL silently
untitles every SESSION this instance starts afterwards.

**Proposed enforcement** — route `nebula rename` over the unversioned HOOK RECEIVER (the AGENT ENV
already carries the BEARER TOKEN), so VERSION SKEW can never break it again.

Logged the entry (three gotchas); promoted nothing.

---

## 👉 ==== NEXT STEPS ====

1. Cut the DEV INSTANCE over so both binaries are v34: quit the dev TUI (that kills its seven live
   SESSIONS), then `make install && make dev`. The pending rows self-title on their next prompt, or
   `r` renames them now.
2. Decide on the enforcement — `nebula rename` over the HOOK RECEIVER, or leave it on the socket. Say
   which and I'll do it.
```

A task the user's terminal has to unblock (hypothetical, condensed) — `OVERVIEW` says what was left out
and why, the 🚧 step carries the `! <command>` form so the login's output lands in the session, and the
re-run is not a step of theirs: once the login is back it is mine, so the list ends with the word to
give me and the commit choice:

````
---

## 💬 ==== YOU ASKED ====

"Backfill the PR ARCHIVE for every PR merged before the action existed, with the same script the
action runs, so a grep of the MEMORY LOG reaches all of them."

---

## 📋 ==== OVERVIEW ====

Eleven of the twenty-six merged PRs are in the PR ARCHIVE; the other fifteen are not. `gh` lost its
login part way through — every call after the eleventh PR came back "authentication required" — and
the login is interactive, so I could not renew it. The eleven rendered files are complete; the script
is unchanged and nothing else was touched.

---

## 🔧 ==== DETAILS ====

**Where it stopped** — the script is the action's own, `.github/scripts/pr_archive.py`, and takes the
PR numbers as one argument; the call that failed is its paginated `gh api` for the reviews. A PR whose
description rendered but whose reviews did not is a partial file, which a re-run overwrites — nothing to
clean up.

---

## 👉 ==== NEXT STEPS ====

1. 🚧 **Blocking:** renew the GitHub login in this session — it hands you a browser code only you
   can complete:
   ```
   ! gh auth login
   ```
2. Then say so and I'll archive the remaining fifteen. The eleven are good to commit now —
   `git add .claude/memory/prs && git commit` — or wait for one commit of all twenty-six.
````
