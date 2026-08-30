# `p` Opens A QUICK PROMPT That Launches An AGENT On What You Typed — 2026-08-29

**Asked:** "as a user, I shall be able to press a hotkey which shows a modal with an prompt input.  a
user can type into this prompt (try to make it multi line an scrollable), and it should spin up a
default (configurable in settings) to use for prompts."
→ refined: "Add a hotkey (`p`, rebindable on the HOTKEYS TAB) that opens a PROMPT DIALOG anywhere in
the TUI: a multi-line, word-wrapped, scrollable task box like the CLOUD TASK EDITOR (`Shift+Enter` /
`Ctrl+J` newline, `Esc` cancels, `Enter` submits). Submitting spins up a new AGENT in the selected
WORKTREE with the typed text as its STARTING PROMPT, and attaches it. The AGENT KIND comes from a new
SETTING on the SETTINGS OVERLAY's AGENTS TAB (default `claude`), with that kind's existing MODEL /
EFFORT defaults. Empty input cancels." (asked: what does "a default" mean → an AGENT KIND only, not an
AGENT PRESET and not a row listing both)

**Did:** New `crates/nebula-tui/src/quick_prompt.rs` (KEEP MODULES SMALL — `launch_options`, the
`open_quick_prompt` hotkey entry, the dialog `title`) plus `PromptKind::QuickPrompt` in `app.rs`
(added to `is_multiline`, so it reuses the CLOUD TASK EDITOR's wrapped, caret-following renderer
untouched), `Action::QuickPrompt` on `p` in `keymap.rs`, the `open_prompt` title/label arm and the
`submit_prompt` launch arm in `event_loop.rs`, and the `quick_prompt_kind` SETTING (`config.rs`, the
seven edits, first row of the AGENTS TAB, read through `Config::quick_prompt_kind`). Unlike the AGENT
PRESETS list it does not require FOCUS on the SESSIONS PANEL — only a selected WORKTREE. The launch
goes through the existing `create_agent` with `starting_prompt: Some(text)` and an empty name, so
AUTO-TITLE names the row from the prompt and a daemon refusal reopens the box with the text intact.
README key table gained a `p` row. `cargo test --workspace --lib`: 551 nebula-tui (9 new), 162
nebula-daemon, 12 nebula-core, clippy clean.

**Gotchas:**
- **`submit_prompt`'s multiline block errors on an empty value *before* the allowlist below it**, so
  staying off that allowlist is not enough to make "empty = cancel" work: the per-kind `needs` message
  had to become an `Option` (`None` for `QuickPrompt`) to fall through to `cancelled: empty input`.
- **`create_agent` skips the WARM SPARE refill for any launch carrying a `starting_prompt`** — such a
  create can never adopt the spare, so there is nothing to refill. The local gating it was named
  `preset`; renamed to `with_first_prompt` now that a second kind of launch uses it.
- `cycle_choice` takes `&'static [&'static str]`, so the AGENT KIND choice list could not be derived
  from `AgentKind::ALL` — spelled out as `config::AGENT_KIND_NAMES` with a test pinning the two equal.
- A harness switched off on the AGENTS TAB *after* being chosen would otherwise launch a kind the NEW
  SESSION PICKER no longer offers; `Config::quick_prompt_kind()` steps on to the first enabled kind
  (and an unparseable name reads as Claude).
- `p` was free: every other lowercase letter in `keymap.rs::ACTIONS` is taken (`h l j k n o g t r a u
  m d e f b z w s q`, plus `/` and `?`) — `c i v x y` are what is left.
- The SESSIONS FOOTER hint line is already truncated at 100 columns, so `p` is advertised in the HELP
  OVERLAY (built from the KEYMAP) and the README only, not the FOOTER.
