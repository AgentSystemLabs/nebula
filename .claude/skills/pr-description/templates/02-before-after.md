<!-- 02 · Before / After — the change replaces something visible: a layout, a key, a status colour.
     Every change is a row with both states; every screenshot is a pair. -->

<One sentence: what was replaced, and the one-clause reason.>

**Contents:** [🔁 Before / After](#before-after) · [📸 Screenshots](#screenshots) · [🧭 What moved](#what-moved) · [⚠️ Risk](#risk) · [🔧 Technical overview](#technical-overview) · [📝 Notes](#notes)

## 🔁 Before / After <a id="before-after"></a>

| | Before | After |
|---|---|---|
| **<Thing one>** | <what it did — one clause> | <what it does now — one clause, the key in backticks> |
| **<Thing two>** | <…> | <…> |
| **<Thing three>** | <…> | <…> |
| **Unchanged** | <what the user liked and still has — say it, so the reviewer knows you kept it> | same |

## 📸 Screenshots <a id="screenshots"></a>

| Before | After |
|---|---|
| ![<alt: … before>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<thing-one>-before.png) | ![<alt: … after>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<thing-one>-after.png) |
| ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<thing-two>-before.png) | ![<alt>](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/<branch>/<thing-two>-after.png) |

<!-- The "before" shot comes from an origin/main build in the SCREENSHOT HARNESS; same size, same demo data, same cursor position. -->

## 🧭 What moved <a id="what-moved"></a>

```mermaid
flowchart TB
  subgraph Before
    a1["old thing"] --> a2["old result"]
  end
  subgraph After
    b1["new thing"] --> b2["new result"]
  end
  a1 -. replaced by .-> b1
  classDef gone fill:#fecaca,stroke:#991b1b,color:#111
  classDef new fill:#bbf7d0,stroke:#166534,color:#111
  class a1,a2 gone
  class b1,b2 new
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

- **What actually changed.** <The mechanism in three sentences; name the type, the flag, the function that moved.>
- **Files.** `crates/<crate>/src/<file>.rs` — <clause>; `crates/<crate>/src/<file>.rs` — <clause>.
- **Kept as-is on purpose.** <The behaviour the change stops short of, and why.>
- **Gate.** <`make ci` green — N tests; or what did not run and why.>

## 📝 Notes <a id="notes"></a>

- <Merge state and conflicts.>
- <Upgrade note, if any.>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

<the session link, only when the harness footer includes one — otherwise delete this line>
