<!-- 07 · Collapsible — a big diff whose detail would bury the summary. Headings stay outside the
     <details> blocks so the TOC links still resolve; only the bodies fold. -->

<Two sentences: what this PR is, and the one number that says how big (files, tests, a version).>

**Contents:** [🎯 Summary](#summary) · [✨ Changes](#changes) · [📸 Screenshots](#screenshots) · [🧩 Architecture](#architecture) · [⚠️ Risk](#risk) · [🔧 Technical overview](#technical-overview) · [📝 Notes](#notes)

## 🎯 Summary <a id="summary"></a>

- **<Hook one>.** <One sentence.>
- **<Hook two>.** <One sentence.>
- **<Hook three>.** <One sentence.>

## ✨ Changes <a id="changes"></a>

<details>
<summary>🚀 <b>&lt;Change one&gt;</b> — &lt;one clause&gt;</summary>

<Two or three paragraphs or bullets, still high level: the key or command in backticks, what the user sees, what stayed the same.>

</details>

<details>
<summary>🔔 <b>&lt;Change two&gt;</b> — &lt;one clause&gt;</summary>

<…>

</details>

<details>
<summary>🐛 <b>&lt;Fix&gt;</b> — &lt;symptom → now&gt;</summary>

<The cause in one clause, the new behaviour in the next.> Fixes #<N>.

</details>

<!-- A blank line after <summary> and before </details> is what makes GitHub render the markdown inside. -->

## 📸 Screenshots <a id="screenshots"></a>

| <Change one> | <Change two> |
|---|---|
| ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<one>.png) | ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<two>.png) |

## 🧩 Architecture <a id="architecture"></a>

```mermaid
flowchart TB
  subgraph cli["nebula (CLI)"]
    M[main.rs] --> CL[cli.rs]
  end
  subgraph daemon["nebula-daemon"]
    S[server.rs] --> R[registry.rs]
    S --> H[hooks/]
  end
  subgraph tui["nebula-tui"]
    E[event_loop.rs] --> A[app.rs] --> UI[ui.rs]
  end
  CL --> S
  E <-->|"protocol vN"| S
  classDef changed fill:#fde68a,stroke:#b45309,color:#111
  class R,UI changed
```

## ⚠️ Risk <a id="risk"></a>

**Verdict:** <🟢 Low risk · 🟡 Merge with care · 🔴 Do not merge as-is — pick one, then one clause saying why. The author's own read; the PR REVIEWER SKILL checks it against the diff.>

| | Level | Why |
|---|---|---|
| 🔒 **Security & production** | <Low / Medium / High> | <who can reach the new code and what it reaches — a new `ClientRequest`, a hook route, a shell call, a token, a file the DAEMON writes — or "no new surface: <why>"> |
| ⚡ **Performance** | <Low / Medium / High> | <the hot path touched — the TUI draw, the event-loop drain, the PTY byte path, the WORKTREE SYNC tick — or "off every hot path: <why>"> |
| 🧩 **Fit with the codebase** | <Low / Medium / High> | <the existing pattern it follows, or the departure and why> |

**Rollback:** <one line — `git revert <merge>`, plus what the revert does not undo: a PROTOCOL VERSION bump, a migrated store, a pushed branch.>

## 🔧 Technical overview <a id="technical-overview"></a>

<details>
<summary>Mechanism, files, rejected approaches, gate</summary>

- **Mechanism.** <Three or four sentences per change.>
- **Files.** `crates/<crate>/src/<file>.rs` — <clause>; `crates/<crate>/src/<file>.rs` — <clause>.
- **Not done.** <The approach rejected and why.>
- **Gate.** <`make ci` green — N tests; or what did not run and why.>

</details>

## 📝 Notes <a id="notes"></a>

- <Merge state, conflicts, upgrade note.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
