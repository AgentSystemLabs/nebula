# Orchestrator

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

The ORCHESTRATOR view is a chat-first way to run nebula. Instead of moving
from project to project and opening each worktree by hand, the user keeps one
pinned agent session in front of them and asks it to coordinate the rest.

## Shape of the feature

- **One orchestrator session.** The daemon persists a session role on agent rows.
  Exactly that row is allowed to call the orchestrator-only socket commands.
  The default harness is Claude, and the launch path can choose another harness
  later without changing the permission model.
- **Chat-first layout.** In orchestrator mode the orchestrator terminal owns the
  main view. A compact status strip shows every live session on the machine in
  the same attention order used by `.` and `,`: needs feedback, running, unseen
  finishes, then the rest by recency.
- **Review modal.** The orchestrator, or the user from the status strip, can open
  a full-screen tabbed modal for one or more sessions. Tabs are terminal, diff,
  history and pull request. The modal reuses the normal terminal pane, git diff
  loader and pull request reader instead of inventing a second renderer.
- **Mobile default.** `orchestrator_view` controls the startup mode: `auto`
  switches to the orchestrator view on narrow terminals, `always` starts there
  everywhere, and `never` keeps the grid default.

## Agent-facing commands

The orchestrator receives extra guidance in its system prompt. These commands
use the same CLI-over-socket pattern as `nebula spawn`, `nebula worktree` and
`nebula open`, but they are daemon-gated to the persisted orchestrator session:

```sh
nebula orchestrator list [--json]
nebula orchestrator read <session-id> [--bytes N]
nebula orchestrator send <session-id> <prompt>
nebula orchestrator spawn --project <project-id-or-name> [--worktree <branch-or-id>] <prompt>
nebula orchestrator review <session-id> [--tab terminal|diff|history|pr]...
```

The text format is compact and model-friendly. `--json` prints stable IDs and
status fields so the orchestrator can call a later command without fuzzy
matching a label. `read` is bounded by the daemon's tail cap and is intended for
recent terminal output, not full transcript export.

## Permission model

The socket is already same-user only, but orchestrator commands are still
privileged: they can message other agents and open review UI in every client.
Every orchestrator request carries the caller's `NEBULA_AGENT_ID`; the daemon
checks that:

1. the caller exists,
2. it is not archived, and
3. its persisted role is `orchestrator`.

Prompt text alone is never trusted. A normal agent can read the guidance only if
it was accidentally pasted into its context, but the daemon still refuses the
request because its row is not marked as the orchestrator.

The orchestrator also does not approve permission prompts for other agents. The
default loop is tell-and-ask: report that a session is waiting and offer to open
it in review. Higher-autonomy policy can be added later once the UI has a clear
way to show what was approved and why.

## Attention feed

The robust first design is **poll-on-demand plus visible chips**, not injected
prompts. The daemon already broadcasts status changes to every TUI, and the
orchestrator can call `nebula orchestrator list --json` whenever it needs a
fresh state read. The TUI status strip updates immediately from the same
events. This avoids writing unsolicited text into an idle chat box, avoids
races with a model mid-turn, and still gives the user an always-visible feed of
what needs attention.

Future work may add rate-limited, clearly marked notes to the orchestrator only
when it is idle. That should be opt-in because injected notes become model
input, while the current list command is explicit and auditable.

## Review modal

The review modal is a tab shell around existing surfaces:

- **Terminal** attaches to the selected session with the normal pane.
- **Diff** opens the selected session's worktree diff through the existing git
  diff view.
- **History** starts with bounded recent output from the session tail; richer
  transcript summaries can be layered behind the same tab later.
- **PR** reads the pull request associated with the session's worktree when the
  TUI has one cached.

`Tab` and `Shift+Tab` cycle tabs, `1`-`9` jump to a tab, and `Esc` closes back to
the orchestrator chat. On phone-width terminals the modal is full-screen and the
status strip collapses to a single line.

## Follow-ups

- First-class transcript summaries per harness instead of terminal-tail history.
- Optional idle-note injection for the orchestrator, behind an autonomy setting.
- Multiple review subjects in one modal, once the single-session tab shell has
  had real use.
- A richer spawn target grammar for project aliases and worktree creation bases.
