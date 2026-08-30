# Every NEBULA Command Gets Its Own Wrapped `--help` Page — 2026-08-30

**Asked:** "i want to improve the cli use experience.  I want it so when I run nebula browser --help
it will print only the information related to browser, try to do that same for ALL commands"
→ refined: "Make every `nebula` command's `--help` a readable page of its own: `nebula browser
--help` shows nothing but NEBULA BROWSER, wrapped to the terminal width (clap has no `wrap_help`
feature today, so option lines run 265 characters). Cut `nebula --help` to one short line per
command, move the full prose into each command's own page, and give every command — NEBULA ADD,
NEBULA DAEMON, NEBULA KILL, NEBULA RENAME, NEBULA WORKTREE, NEBULA SPAWN, NEBULA WORKSPACE, NEBULA
BROWSER, NEBULA SSH, NEBULA TUNNEL, NEBULA UPGRADE — an `Examples:` block. Keep flag names, parsing
and behavior exactly as they are; the hidden `_raw-attach` and `_stale-daemon-note` stay hidden."
(asked: root `nebula --help` → one short line per command; per-command additions → `Examples:` block)

**Did:** Two commits. `3b7af30` moved `Cli`, `Command`, `WorkspaceCommand` and `parse_agent_kind`
out of `crates/nebula/src/main.rs` into a new `crates/nebula/src/cli.rs`, text byte-identical
(proved by diffing `--help` for the root and four subcommands against the installed 0.21.0 binary);
`main.rs` is now 162 lines of dispatch and logging. `0fb7b43` added `wrap_help` to clap in
`crates/nebula/Cargo.toml` with `max_term_width = 100` on the root, split every doc comment into a
one-sentence first paragraph plus prose, and put an `Examples:` block in `after_help` on the root,
all eleven visible commands and the five NEBULA WORKSPACE subcommands (whose positional args had no
doc comment at all and rendered as a blank `Arguments:` entry). `crates/nebula/tests/help_cli.rs`
(6 tests) pins the contract; `docs/commands.md` was resynced; `TERMS.md`'s NEBULA row now points at
`cli.rs::Cli`. `make ci` green.

**Gotchas:**
- **The premise was wrong and the symptom was real.** `nebula browser --help` already printed only
  browser — clap scopes subcommand help for free. What looked like "it prints everything" was
  265-character option lines hard-wrapping into the left margin. Check the actual bytes
  (`awk '{print length}'`) before believing a scoping bug.
- **Without clap's `wrap_help` feature, `StyledStr::wrap` compiles to `fn wrap(&mut self, _: usize) {}`**
  (`clap_builder-4.6.6/src/builder/styled_str.rs:77`). Every width path still runs — `term_w()`
  computes 100 — and then wraps nothing. `max_term_width`/`term_width` are equally inert. The feature
  is the whole mechanism; nothing in the API hints at it.
- **`clap_derive` splits a doc comment on blank lines**: paragraph 1 → `about`, the whole comment →
  `long_about` (`clap_derive-4.6.4/src/utils/doc_comments.rs:125-127`), and the root's command index
  renders `about` only (`help_template.rs::write_subcommand`). One paragraph therefore means
  `about == long_about` and a wall of text in the index. It also strips the trailing period from
  `about`, so the index reads clean while the page keeps its full stop.
- **`max_term_width` on the root reaches every subcommand** — `_propagate_subcommand` does
  `sc.app_ext.update(&self.app_ext)`, and both widths live in `app_ext`. Setting it once is enough.
- **`after_help` is wrapped like everything else**, so hand-aligned example columns must stay under
  the narrowest width you care about (~78) or they reflow into a mess. `after_long_help` falls back
  to `after_help`, so one string serves both `-h` and `--help`.
- **A width test cannot assert `narrow < wide` for every page**: `nebula workspace list --help` is
  58 columns at any width and fails a strict-shrink assertion. Assert `<= width` per page and prove
  the reflow once on a prose-heavy page (NEBULA TUNNEL).
- **Count help columns in `chars()`, not bytes** — the em-dashes are 3 bytes each, so `awk` reported
  102 and 103 for pages that are 98 and 99 characters wide.
