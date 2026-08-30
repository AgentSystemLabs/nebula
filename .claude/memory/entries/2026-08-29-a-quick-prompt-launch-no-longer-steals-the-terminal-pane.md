# A QUICK PROMPT Launch No Longer Steals The TERMINAL PANE — 2026-08-29

**Asked:** "when using quick prompt, do not auto focus the new terminal that opens by default (allow a
user to enable it in the settings modal)"
→ refined: "When the QUICK PROMPT launches an AGENT, don't take FOCUS into the TERMINAL PANE by default:
select the new SESSION's row in the SESSIONS PANEL (so the pane previews it and MARK SEEN applies) but
leave FOCUS on the panel I was in, never LOCKED. Add a `Focus new session` SETTING on the SETTINGS
OVERLAY's AGENTS TAB, next to `Quick prompt agent`, default off, that restores today's behavior (attach,
FOCUS on the TERMINAL PANE, LOCKED PANE) when on. Every other launch — the NEW SESSION PICKER, AGENT
PRESETS, PR SESSION, CLOUD — keeps focusing exactly as it does now." (asked: how far the cursor moves
with auto-focus off → select the new SESSION's row but never enter the pane, not "nothing moves at all")

**Did:** The create Ack, not the launch site, is where "enter and lock the pane" was decided, so both
attach intents grew a flag: `PendingIntent::AttachCreated { focus }` and
`AttachCreatedWithCloudRetry { focus, .. }` (`app.rs`), bound by one or-pattern in the
`ServerEvent::Ack` arm of `event_loop.rs::handle_server_event` — the `select_when_seen` /
`land_pending_selection` / `attach_now` trio still runs, only `app.focus = Focus::Terminal;
app.term_locked = true` is now behind `if focus`. `AgentLaunchDraft` gained `focus_pane`, passed `true`
by the NEW SESSION PICKER (both arms), the CLOUD TASK EDITOR and AGENT PRESETS, and
`Config::load().quick_prompt_focus` by the `PromptKind::QuickPrompt` arm. The SETTING is the usual seven
`config.rs` edits (`SettingKind::QuickPromptFocus`, the AGENTS TAB row under `Quick prompt agent`, the
field, `Default` = false, the `obj.insert`, `value_label`, `cycle`). Two new tests:
`a_quick_prompt_lands_the_row_without_taking_the_pane` (drives `p` → Enter → upsert → Ack under both
config values) and `quick_prompt_focus_toggles_off_by_default_and_persists`. README `p` row extended.
`cargo test --workspace --lib`: 561 nebula-tui, 162 nebula-daemon, 12 nebula-core; `cargo test --test
e2e_tui` 7 passed; clippy clean.

**Gotchas:**
- **The Ack handler is the single choke point for post-create focus** — nothing in the QUICK PROMPT's own
  path touches `app.focus`, so a "don't focus it" ask that greps for `Focus::Terminal` near the launch
  finds nothing. `create_agent` builds the intent; the intent decides the focus.
- Both attach intents had to take the flag even though only one carries the QUICK PROMPT: `create_agent`
  picks between them on `(reopen_on_error, cloud_prompt)`, and QUICK PROMPT is `AttachCreatedWithCloudRetry`
  *because it reopens on error*, not because it is a cloud launch.
- The `ServerEvent::Error` arm matches `AttachCreatedWithCloudRetry { kind, task: text }` in an or-pattern
  with `ReopenPromptOnError`; a new field there is a compile error fixed with `..`, not a new arm.
- The per-row settings hint goes through the same `truncate(.., inner.width)` (82 cols) as
  `settings_keys_hint` — an 86-char hint chops silently, so it was shortened before anyone saw it.
- `std::mem::replace(app, App::new())` to hand a driven `App` back out of a test helper is a clippy
  `mem_replace_with_default` warning; take `&mut App` and assert on the caller's binding instead.
