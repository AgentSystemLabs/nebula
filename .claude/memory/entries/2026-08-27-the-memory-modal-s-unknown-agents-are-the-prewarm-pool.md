# The MEMORY MODAL's Unknown Agents Are The PREWARM POOL — 2026-08-27

**Asked:** "I notice I have a BUNCH of unknown agents.. are these sub agents claude is spawning? is so
display grouped in tree format.. also try to figure out a better label for them. find root cause what
these are from"

**Did:** They are not subagents (the Agent tool runs in-process inside `claude`). Every `(unknown agent)`
row was a **PREWARM POOL spare** (a WARM SPARE): `Daemon::prewarm_agent` in `crates/nebula-daemon/src/registry.rs` boots
a real `claude` CLI per (worktree, kind) with a fresh `NEBULA_AGENT_ID` and *no store row*, so the TUI's
`app.tree.agents` lookup in the MEMORY MODAL missed. `SessionMetrics` (`nebula-core/src/protocol.rs`)
gained `prewarm: Option<PrewarmInfo { worktree, kind, model }>` (`#[serde(default)]`, no protocol bump);
`Daemon::session_pids` fills it from the `prewarmed` map; the modal in `nebula-tui/src/ui.rs`
(`Overlay::Metrics`) now lists spares under a `warm spares (N)` header as a `├`/`└` tree, named
`claude · opus`, placed in their worktree, inert on Enter, with their own `warm … pre-booted for new
agents` rollup line; `footer_usage` counts them as `N warm` (the WARM COUNT) instead of agents. Test:
`metrics_groups_prewarm_spares_under_their_own_header` in `event_loop.rs`.

**Gotchas:**
- **Root cause of the count, not just the label:** resting the sidebar cursor on a worktree for 250ms
  (the PREWARM DEBOUNCE: `PREWARM_DEBOUNCE`, `schedule_prewarm` → `fire_pending_prewarm` in `event_loop.rs`) prewarms it, and each
  spare lives up to 15 min (`PREWARM_MAX_AGE`). Scrolling across 7 projects = 7 idle `claude`s at
  150–300 MB each (the CLI plus its MCP children — python + MissionControl's `recall-mcp.mjs`) ≈ 1.7 GB.
  Opt-out is the hand-added `"prewarm_agents": false` key in the DATA DIR's `config.json`; the SETTINGS
  OVERLAY does not expose it. Not changed — the user has not asked for the policy to move.
- To identify a mystery daemon child, `ps -Eo command -p <pid>` shows its `NEBULA_AGENT_ID` /
  `NEBULA_DATA_DIR` env, and `grep prewarm <data_dir>/state/daemon.log` maps the id to its spawn
  (`prewarmed agent session agent=… worktree=<branch>` — the branch only, not the project, so `main`
  repeats across projects).
- `ps -axo … -p <pids>` on macOS ignores `-p` and dumps every process; drop `-a`.
