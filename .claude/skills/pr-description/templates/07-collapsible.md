<!-- 07 · Collapsible — a big diff whose detail would bury the summary. Headings stay outside the
     <details> blocks so the TOC links still resolve; only the bodies fold. -->

<Two sentences: what this PR is, and the one number that says how big (files, tests, a version).>

**Contents:** [🎯 Summary](#-summary) · [✨ Changes](#-changes) · [📸 Screenshots](#-screenshots) · [🧩 Architecture](#-architecture) · [🔧 Technical overview](#-technical-overview) · [📝 Notes](#-notes)

## 🎯 Summary

- **<Hook one>.** <One sentence.>
- **<Hook two>.** <One sentence.>
- **<Hook three>.** <One sentence.>

## ✨ Changes

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

## 📸 Screenshots

| <Change one> | <Change two> |
|---|---|
| ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<one>.png) | ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<two>.png) |

## 🧩 Architecture

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

## 🔧 Technical overview

<details>
<summary>Mechanism, files, rejected approaches, gate</summary>

- **Mechanism.** <Three or four sentences per change.>
- **Files.** `crates/<crate>/src/<file>.rs` — <clause>; `crates/<crate>/src/<file>.rs` — <clause>.
- **Not done.** <The approach rejected and why.>
- **Gate.** <`make ci` green — N tests; or what did not run and why.>

</details>

## 📝 Notes

- <Merge state, conflicts, upgrade note.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
