<!-- 10 · Crate map — the change spans the workspace. Organise by where it lives; the diagram is
     the workspace with the touched crates lit up. -->

<Two sentences: what landed across the workspace, and the one place a reviewer should start.>

**Contents:** [📦 By crate](#by-crate) · [🧩 Workspace map](#workspace-map) · [📸 Screenshots](#screenshots) · [⚠️ Risk](#risk) · [🔧 Technical overview](#technical-overview) · [📝 Notes](#notes)

## 📦 By crate <a id="by-crate"></a>

### 🧱 nebula-core

- **<Hook>.** <The type, flag or protocol field, and what it means for the user, one sentence.>

### ⚙ nebula-daemon

- **<Hook>.** <What the DAEMON now does, the hook route or mechanism in backticks.>
- **<Hook>.** <…>

### 🖥 nebula-tui

- **<Hook>.** <What the user sees, the key in backticks, the panel it lives in.>

### 💻 nebula (CLI)

- **<Hook>.** <The subcommand or flag, `nebula <sub>`, and what it prints.>

### 📚 docs / memory

- **<Hook>.** <Which of the DOCS PAGES or which MEMORY LOG entry changed.>

## 🧩 Workspace map <a id="workspace-map"></a>

```mermaid
flowchart LR
  subgraph ws["workspace"]
    core["nebula-core"]
    daemon["nebula-daemon"]
    tui["nebula-tui"]
    cli["nebula"]
  end
  cli --> daemon
  cli --> tui
  tui -->|"protocol vN"| daemon
  daemon --> core
  tui --> core
  agent["agent CLI"] -->|"hooks"| daemon
  classDef changed fill:#fde68a,stroke:#b45309,color:#111
  class core,daemon,tui changed
```

<!-- Give the `changed` class to every crate the diffstat names; leave the rest plain. -->

## 📸 Screenshots <a id="screenshots"></a>

| <Caption: what the TUI shows after the change> |
|---|
| ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<after>.png) |

<!-- A pure daemon/core change: a fenced block of the terminal output instead, plus one line on why there is no PNG. -->

## ⚠️ Risk <a id="risk"></a>

**Verdict:** <🟢 Low risk · 🟡 Merge with care · 🔴 Do not merge as-is — pick one, then one clause saying why. The author's own read; the PR REVIEWER SKILL checks it against the diff.>

| | Level | Why |
|---|---|---|
| 🔒 **Security & production** | <Low / Medium / High> | <who can reach the new code and what it reaches — a new `ClientRequest`, a hook route, a shell call, a token, a file the DAEMON writes — or "no new surface: <why>"> |
| ⚡ **Performance** | <Low / Medium / High> | <the hot path touched — the TUI draw, the event-loop drain, the PTY byte path, the WORKTREE SYNC tick — or "off every hot path: <why>"> |
| 🧩 **Fit with the codebase** | <Low / Medium / High> | <the existing pattern it follows, or the departure and why> |

**Rollback:** <one line — `git revert <merge>`, plus what the revert does not undo: a PROTOCOL VERSION bump, a migrated store, a pushed branch.>

## 🔧 Technical overview <a id="technical-overview"></a>

- **nebula-core.** `crates/nebula-core/src/<file>.rs` — <clause>. <PROTOCOL VERSION N → N+1, or "unchanged".>
- **nebula-daemon.** `crates/nebula-daemon/src/<file>.rs` — <clause>; `…/<file>.rs` — <clause>.
- **nebula-tui.** `crates/nebula-tui/src/<file>.rs` — <clause>.
- **nebula.** `crates/nebula/src/<file>.rs` — <clause>.
- **Not done.** <The approach rejected and why.>
- **Gate.** <`make ci` green — N tests; or what did not run and why.>

## 📝 Notes <a id="notes"></a>

- <Merge state, conflicts.>
- <Upgrade note: `nebula kill` first when the PROTOCOL VERSION moved.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
