# `nebula open <file>…` Raises The FILE TABS: Tabbed Preview, Embedded EDITOR, A Staged Ctrl+Q — 2026-09-04

**Asked:** "bro Im supposed to pick the best one and you show me each. add a way for claude to "open
file" inside this app which will make a modal with tabs i can go through. uses similar tab modal like
settings modal, when I enter on tab show file in vim below, when i focus show the file preview,
control - q should go back up to tab bar first , then test by opening these 5 examples you made, write
those to a tmp dir in the skill dir then run that commamd to verufy they will opem in this modal"
→ refined: Add `nebula open <file>…` — an "open file" command like NEBULA SPAWN, run by an AGENT from
inside its SESSION over the AGENT ENV — that makes the TUI show a new OVERLAY: a tab strip like the
SETTINGS OVERLAY's, one tab per file. The focused tab shows a read-only preview of its file below (the
TREE BROWSER's syntax-highlighted preview); Enter on a tab runs the EDITOR in that space instead (the
VIM MODAL machinery, sized to the modal body). Ctrl+Q steps back to the tab bar first — out of vim or
the preview — and closes the modal only from the tab bar (I know HARDWIRED UNLOCK closes every OVERLAY
outright since 2026-08-30; this modal is the exception). One Esc and a CLICK OUTSIDE close it from the
tab bar. Then write the five worked examples to a tmp dir under `.claude/skills/output-doctor/` and
prove `nebula open` on those five opens the modal with five tabs. Keep the FILE FINDER, TREE BROWSER
and SETTINGS OVERLAY as they are. (Assuming every attached TUI opens the modal whichever SESSION is
selected, and that the five files are the examples as written, not five alternative layouts.)

**Did:** The FILE TABS (a candidate TERM), end to end. Protocol: `ClientRequest::OpenFiles { req_id,
id, paths }` and the pushed `ServerEvent::FilesOpened { agent, root, paths }`, PROTOCOL VERSION 36 →
37 (`crates/nebula-core/src/protocol.rs`). CLI: `Command::Open { files }` + `OPEN_EXAMPLES` in
`crates/nebula/src/cli.rs`, `nebula_tui::run_open` → `ipc::open_files_for_current_agent` (canonicalizes
each path in the caller's cwd, refuses a missing one before connecting, prints the one-liner the model
reads). Daemon: new `crates/nebula-daemon/src/open_files.rs` — `Daemon::open_files` (agent row must
exist; its WORKTREE path rides along as the editor's cwd) and `CLAUDE_OPEN_GUIDANCE`, pushed third in
`registry.rs::push_system_prompt` (the `guided()` test helper and the composed-prompt assert follow),
plus `Bash(nebula open:*)` in `installer.rs::CLAUDE_ALLOW_RULES`. TUI: new
`crates/nebula-tui/src/file_tabs.rs` (`FileTabsView`: tabs, `on_tabs`, highlighted preview via the now
`pub(crate)` `tree_browser::read_preview`, `handle_key` on the SETTINGS OVERLAY's command-enum pattern,
`handle_mouse`, `open`, `open_in_editor` → `spawn_editor_modal` with `vim.embedded = true`);
`Overlay::FileTabs` in `app.rs`; `ui.rs` draws it on the TREE BROWSER's footprint with the tab strip
and preview factored into `tab_strip` / `strip_rule` / `preview_window` (the settings and tree arms now
use them too) and `draw_vim` renders an embedded editor into whichever overlay owns a pane; the
HARDWIRED UNLOCK exception is a pre-check in `overlay_close::force_close` (preview → strip) plus
`close_vim`'s `FileTabs` arm (editor → strip, preview re-read). `handle_server_event` gets the
`FilesOpened` arm. Docs: `docs/commands.md`, `docs/keys.md` (a row, and the Ctrl+Q exception),
`docs/how-it-works.md`, `README.md`, the root `--help` sentence. Tests: eight in `file_tabs.rs`, two in
`event_loop.rs` (the event raises the modal; Ctrl+Q staging through a real `/bin/sh` editor), the
`every_overlay` row + `overlay_label` arm + count 15, `help_cli.rs` count 13 + `VISIBLE`, an E2E PTY
test (CLI → daemon → `FilesOpened` on every subscriber, relative paths, a missing path, no AGENT ENV)
and an E2E TUI test that types `nebula open` into the stand-in `/bin/sh` inside a live session and
waits for the modal. Gate: nebula-tui 598, nebula-daemon 171, nebula-core 12, nebula 41+7 unit + E2E PTY
28 + E2E TUI 8 + help_cli 6, all passed; clippy `-D warnings` clean; `cargo fmt --check` clean. The
five examples live in `.claude/skills/output-doctor/tmp/0{1..5}-*.md` (untracked); a tmux-driven
isolated instance of this build opened them and the PNGs (scratchpad `shot/`) show five tabs, vim in
the body, and the strip after Ctrl+Q. Not run against the user's DEV INSTANCE: its daemon and the
`nebula` on PATH are the old build (VERSION SKEW at v37) until `make install && make dev`.

**Gotchas:**
- **The HARDWIRED UNLOCK now has one staged exception.** `force_close` opens with a FILE TABS
  pre-check (preview → strip, return) and `handle_vim_key`'s Ctrl+Q already ran first for the editor,
  so the "never stages, lands on the panels every time" contract in `overlay_close.rs`, `docs/keys.md`
  and the TERMS row is "…except the FILE TABS, which step to the strip first". Anyone widening Ctrl+Q
  again must keep that arm.
- **E2E TUI's `add_project` hops back to the PROJECTS PANEL** (`FOOTER_PROJECTS`), unlike the
  in-TUI add that lands in Worktrees — waiting for `FOOTER_WORKTREES` after it times out, and in a
  tmux drive `nebula add` from outside leaves focus on Projects too: walk Enter until the footer says
  `n: agent` before pressing `n`, or `n` creates a worktree with an invented name instead of a session.
- **Typing the CLI into the stand-in shell is the real E2E TUI proof**: the `/bin/sh` inside the
  session holds the AGENT ENV, so `env!("CARGO_BIN_EXE_nebula") open a b` + Enter is exactly what a
  model runs. Assert on the modal title (`Open files (2)`) and the file's *contents* — the shell echoes
  the command line, so a tab label alone matches the echo.
- **`crates/nebula/tests/help_cli.rs` pins the root command count** (`the_root_help_lists_one_line_per
  _command`, 13 with `help`) and `VISIBLE` lists the subcommands whose help must wrap — a new
  subcommand fails the suite until both are bumped; the compiler says nothing.
- **macOS has no `timeout` binary**: `timeout 280 cargo check …` exits 127 with no output, and a
  `grep -c '^error'` of the empty log prints 0 — a clean-looking "baseline" that never ran. The Bash
  tool's own timeout is the way to bound a build.
- A `FilesOpened` while an editor is up does not kill it (an unsaved buffer): `open` clears
  `vim.embedded` so it floats above the new tabs, and `close_vim`'s FILE TABS arm reloads the preview
  whether the editor was embedded or floating.
- Ctrl+Q kills vim, so vim leaves a `.swp` beside the file (the footer read `+1 file` in the shot);
  `:q` is the clean way out and the hint row says so. Inherent to the hatch, not fixed.
- A python script that edits several files in one run must assert every anchor **before** writing
  any: one late `AssertionError` left `event_loop.rs` written and `overlay_close.rs` / `ipc.rs`
  untouched, and only the compiler's two errors showed the batch was half-applied.
- The SCREENSHOT recipe in my notes held: a private tmux server driven in one Bash call,
  `capture-pane -epN`, a ~90-line pillow renderer (`python3 -m venv` in the scratchpad + `pip install
  pillow` works), Menlo; Apple Symbols stands in for the emoji.
