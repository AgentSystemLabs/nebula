<!-- 08 · Bug fix — issue-driven. Symptom, cause, fix, proof, in that order; the pair of screenshots
     is the proof a reviewer can see without running anything. -->

Closes #<N>. <One sentence: what was wrong, and what is true now.>

**Contents:** [🐛 Symptom](#symptom) · [🔍 Cause](#cause) · [✅ Fix](#fix) · [📸 Before / After](#before-after) · [🔁 State](#state) · [⚠️ Risk](#risk) · [🔧 Technical overview](#technical-overview) · [🧪 Proof](#proof) · [📝 Notes](#notes)

## 🐛 Symptom <a id="symptom"></a>

<What the user saw, in their words (the issue title or the MEMORY LOG **Asked** line): the screen, the key, the terminal, the version. Two or three sentences.>

## 🔍 Cause <a id="cause"></a>

<The root cause in plain words, one paragraph: which value was wrong, since when, and why nothing caught it.>

## ✅ Fix <a id="fix"></a>

- **<Hook>.** <The new behaviour, one or two sentences.>
- **Unchanged.** <What around it was deliberately left alone.>

## 📸 Before / After <a id="before-after"></a>

| Before (#<N>) | After |
|---|---|
| ![<alt: the bug on screen>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/before.png) | ![<alt: fixed>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/after.png) |

## 🔁 State <a id="state"></a>

```mermaid
stateDiagram-v2
  [*] --> StateA
  StateA --> StateB: event
  StateB --> StateC: event that used to be missed
  note right of StateC
    Before: stuck in StateB
    After: event now reaches here
  end note
  StateC --> [*]
```

<!-- Or a flowchart with the buggy edge in red and the fixed edge in green, when the bug is a wrong path rather than a missing transition. -->

## ⚠️ Risk <a id="risk"></a>

**Verdict:** <🟢 Low risk · 🟡 Merge with care · 🔴 Do not merge as-is — pick one, then one clause saying why. The author's own read; the PR REVIEWER SKILL checks it against the diff.>

| | Level | Why |
|---|---|---|
| 🔒 **Security & production** | <Low / Medium / High> | <who can reach the new code and what it reaches — a new `ClientRequest`, a hook route, a shell call, a token, a file the DAEMON writes — or "no new surface: <why>"> |
| ⚡ **Performance** | <Low / Medium / High> | <the hot path touched — the TUI draw, the event-loop drain, the PTY byte path, the WORKTREE SYNC tick — or "off every hot path: <why>"> |
| 🧩 **Fit with the codebase** | <Low / Medium / High> | <the existing pattern it follows, or the departure and why> |

**Rollback:** <one line — `git revert <merge>`, plus what the revert does not undo: a PROTOCOL VERSION bump, a migrated store, a pushed branch.>

## 🔧 Technical overview <a id="technical-overview"></a>

- **The line that mattered.** `crates/<crate>/src/<file>.rs` — <what the old code did and what the new code does, two sentences>.
- **Why it was missed.** <The test gap or the environment difference.>
- **Why not <the obvious alternative>.** <One sentence.>

## 🧪 Proof <a id="proof"></a>

- **Regression test.** `<test_name>` in `crates/<crate>/src/<file>.rs` — fails on `origin/main`, passes here.
- **Gate.** <`make ci` green — N tests; or what did not run and why.>

## 📝 Notes <a id="notes"></a>

- <Merge state, conflicts, upgrade note.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
