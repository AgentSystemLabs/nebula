# `Tab` And `⇧Tab` Retarget One QUICK PROMPT Launch — Harness Or AGENT PRESET — 2026-08-29

**Asked:** "add a hotkey on the quick prompt modal which will let the user configure the prompt
harness & effort to be different from the default, or they can press a hotkey to select from a list
of existing presets they've already defined."
→ refined: "In the QUICK PROMPT (`p`), add two in-dialog hotkeys that change what this one launch uses
without losing what I have typed: `Tab` opens the same AGENT KIND → MODEL / EFFORT picker the NEW
SESSION PICKER uses (TYPE-AHEAD and all), and `Shift+Tab` opens my saved AGENT PRESETS as a picker —
choosing one adopts its harness, MODEL / EFFORT and its prefix/postfix wrapping for this launch. `Esc`
in either picker comes back to the box with my text intact. The override is per-dialog: it never
rewrites the `Quick prompt agent` SETTING, and the next `p` starts from the default again. Show the
live spec in the dialog title and the two new keys on its bottom border."

**Did:** `PromptKind::QuickPrompt` became a tuple over the new `quick_prompt::QuickLaunch` (worktree,
kind, model, effort, optional AGENT PRESET) with `from_config` / `of_kind` / `of_preset` / `compose` /
`title` / `label` on it, so every surface reads one spec. `Tab` and `⇧Tab` in the PROMPT DIALOG
(`event_loop.rs`, guarded to `QuickPrompt`) open `quick_prompt::open_launch_picker` — the NEW SESSION
PICKER's own CONTEXT MENU rows, so `→` reuses `build_submenu` — and `open_preset_picker`, which is
`AgentPresetsView` with a new `quick: Option<QuickReturn>` putting it in pick-only mode (title
` Use preset `, Enter applies, Esc returns, `a`/`e`/`d` flash). Both carry a `QuickReturn` (the launch
as it stood + the typed text); `MenuAction::NewAgentOfKind` gained `quick` and rides it through the
submenus like `pr_url`. A pick returns through `quick_prompt::reopen`; `Tab`'s pick clears any preset
(a launch spec has one source), `⇧Tab`'s sets it and its `compose` wraps the task, sharing the
`MAX_CLOUD_PROMPT_BYTES` ceiling with AGENT PRESET TASK. `ui.rs` grew `task_prompt_hint(kind, width)`
with a test measuring every tier against the border. 559 nebula-tui (7 new), 162 daemon, 12 core, 26
E2E PTY, 7 E2E TUI, clippy + fmt clean.

**Gotchas:**
- **One `app.overlay` at a time means a picker opened from a dialog evicts it**, so the dialog has to
  ride along in the picker's own data (`MenuAction::NewAgentOfKind { quick }`, `AgentPresetsView.quick`)
  and be put back on both exits. In a CONTEXT MENU only the *top-level* Esc arm restores it — a
  submenu's Esc just pops `menu.parent`, which is already the right behavior.
- `build_submenu` destructures exactly one `MenuAction` variant, so reusing the MODEL / EFFORT
  submenus for a new caller means adding a field to `NewAgentOfKind`, not a new variant.
- **The task box's border hint was already two columns too wide at 36–41 columns** (`>= 36` picked a
  40-char line into a 34-column border) — found by the new width test, fixed to `>= 42`; ratatui clips
  silently, so only a measurement catches it.
- `WorktreeId` (and every `id_newtype!`) implements `From<String>` but *not* `From<&str>` — test
  fixtures need `WorktreeId::from("wt-1".to_string())`.
- A `cargo build -p nebula-tui` right after an edit can report `Finished` without recompiling; `touch
  src/lib.rs` first when a build looks suspiciously clean, and remember `build` never compiles
  `#[cfg(test)]` code — only `cargo test --no-run` proves the tests still match new struct shapes.
