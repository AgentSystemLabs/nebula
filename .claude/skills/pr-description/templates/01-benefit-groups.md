<!-- 01 · Benefit groups — the default. A feature PR with two to four things the user gets.
     Sections are named for the benefit, fixes file under the promise they keep. -->

<One or two sentences: what landed and why — the two things a reader must know before the diff.>

**Contents**
- [✨ What you get](#what-you-get)
  - [🚀 <Benefit group one>](#benefit-group-one)
  - [🔔 <Benefit group two>](#benefit-group-two)
- [📸 Screenshots](#screenshots)
- [🧭 How it flows](#how-it-flows)
- [⚠️ Risk](#risk)
- [🔧 Technical overview](#technical-overview)
- [📝 Notes](#notes)

## ✨ What you get <a id="what-you-get"></a>

### 🚀 <Benefit group one> <a id="benefit-group-one"></a>

- **<Two-to-five-word hook>.** <What it does, in two or three sentences for someone who has not read the diff: the key or command in backticks, the setting in the `Settings › Sessions › done_sound` form, where it lives.>
- **<Hook>.** <…>

### 🔔 <Benefit group two> <a id="benefit-group-two"></a>

- **<Hook>.** <…>
- **<A fix, filed here because this is the promise it keeps>.** <The cause in one clause, the new behaviour in the next.>

<!-- A group with one bullet merges into its neighbour. Two or three bullets per group. -->

## 📸 Screenshots <a id="screenshots"></a>

| <What this shows — the screen after the change> |
|---|
| ![<alt: the SESSIONS PANEL with …>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<after>.png) |

<!-- Captured with the SCREENSHOT HARNESS at 190x50; hosted on the pr-assets branch (skill step 4). -->

## 🧭 How it flows <a id="how-it-flows"></a>

```mermaid
flowchart LR
  U([User]) -->|"key or command"| T[TUI]
  T -->|"request"| D[DAEMON]
  D -->|"what is new"| N["new mechanism"]
  N -->|"result"| T
  classDef changed fill:#fde68a,stroke:#b45309,color:#111
  class N changed
```

<!-- Draw the change, not the system: ten to twenty nodes, the new node(s) given the `changed` class. -->

## ⚠️ Risk <a id="risk"></a>

**Verdict:** <🟢 Low risk · 🟡 Merge with care · 🔴 Do not merge as-is — pick one, then one clause saying why. The author's own read; the PR REVIEWER SKILL checks it against the diff.>

| | Level | Why |
|---|---|---|
| 🔒 **Security & production** | <Low / Medium / High> | <who can reach the new code and what it reaches — a new `ClientRequest`, a hook route, a shell call, a token, a file the DAEMON writes — or "no new surface: <why>"> |
| ⚡ **Performance** | <Low / Medium / High> | <the hot path touched — the TUI draw, the event-loop drain, the PTY byte path, the WORKTREE SYNC tick — or "off every hot path: <why>"> |
| 🧩 **Fit with the codebase** | <Low / Medium / High> | <the existing pattern it follows, or the departure and why> |

**Rollback:** <one line — `git revert <merge>`, plus what the revert does not undo: a PROTOCOL VERSION bump, a migrated store, a pushed branch.>

## 🔧 Technical overview <a id="technical-overview"></a>

- **Mechanism.** <How it works, in three or four sentences: the type or flag added, who owns the state, what triggers it.>
- **Files.** `crates/<crate>/src/<file>.rs` — <one clause>; `crates/<crate>/src/<file>.rs` — <one clause>.
- **Not done.** <The approach rejected and why, so the reviewer does not ask "why not X".>
- **Gate.** `make ci` green — fmt, clippy, <N> tests. <Or: what did not run, and why.>

## 📝 Notes <a id="notes"></a>

- <Merge state: `origin/main` (vX.Y.Z) is merged in; conflicts and what they broke, or "none".>
- <Anything the user must do to upgrade — a PROTOCOL VERSION bump means `nebula kill` first.>
- Closes #<N>.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
