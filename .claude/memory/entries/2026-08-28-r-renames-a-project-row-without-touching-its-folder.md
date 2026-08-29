# `r` Renames A PROJECT Row Without Touching Its Folder — 2026-08-28

**Asked:** "Allow me to rename a project from the projects section, by using hte shortcut "r" if a project
is selected. Currently the project name is the folder name. We don't want to changhe the folder name, is
just a visual change, but then if the project is renamed, display under it on a smaller font size the
folder name still." — then, after the first cut shipped a plain `th.dim` foreground: "I told you to use a
smaller fontsize though for the project folder name that is being displayed under, and displayed it kinda
with less opacity, so the priority in the ui the project name we chose" — then "Yes but in Terms of
hierarchy we should display like the folder name in a smaller font size. Is there any way that we can do
that?" — then, on testing it: "After renaming the project, if I rename again to an empty string, it should
be like undoing the rename and come back to display as the default way"

**Did:** No schema change — `projects.name` was already a plain display column and `repo_path` already
carried the folder, so a rename is one `UPDATE`. `nebula-core/src/entities.rs` grew
`Project::folder_name(&Path)` (the last path component, `"project"` fallback — lifted verbatim out of
`add_project`, which now calls it) and `Project::folder_subtitle() -> Option<String>`, which returns the
folder name only while it differs from `name`. That derived check *is* the "has it been renamed?" flag;
no new field, no migration. Daemon: `Store::rename_project`, `Daemon::rename_project`
(`registry.rs:~739`, trims, and an **empty name resets the row to the folder name** — the only way back
from a rename), `ClientRequest::RenameProject` + **PROTOCOL VERSION 28 → 29**, `server.rs` dispatch. TUI:
`PromptKind::RenameProject` / `MenuAction::RenameProject`, the previously-empty
`Action::Rename => Focus::Projects => {}` arm in `event_loop.rs:1455`, a "Rename" row in both project
CONTEXT MENUs (keyboard `m` and right-click), FOOTER hint, HELP OVERLAY PROJECTS row, README keymap row.
`ui.rs::draw_projects` grows a renamed row to `PROJECT_BTN_H + 1` and renders the folder name on the row
under the name as `└ <folder>` — the `└` flush with the name's first column (not the STATUS DOT), the
whole line `th.dim` **plus `Modifier::DIM`**. Three signals, because a fourth is not available: see the
font-size gotcha below. Tests:
`rename_project_relabels_the_row_and_leaves_the_folder_alone` (registry),
`r_renames_the_selected_project_row`, `renaming_a_project_to_nothing_undoes_the_rename` and
`a_renamed_project_shows_its_folder_name_underneath` (event_loop), plus the real-PTY
`tui_project_rename_shows_the_folder_and_empty_undoes_it` (e2e_tui). Workspace: 698 green, fmt/clippy
clean.

**Gotchas:**
- **`submit_prompt` cancels empty input for every `PromptKind` not on an explicit allowlist**
  (`event_loop.rs:~3739`): it flashes `cancelled: empty input` and returns *before* the `match`, so the
  request never leaves the client. That made the daemon's "empty name resets to the folder name" branch
  **unreachable from the UI** while its daemon-side test passed the whole time — the user found it by
  using the feature. `RenameProject` had to join `NewAgent | NewWorktree` on that allowlist. Any future
  prompt whose empty value *means* something must opt in the same way, and only an end-to-end press
  proves it: a registry test and a "does the prompt open" test both stay green.
- **"Render this text smaller" — the settled answer, so nobody re-derives it.** The user asked three
  times; all three avenues were checked, not guessed:
  1. **A terminal cell has exactly one font size.** No SGR attribute scales text down. DECDHL/DECDWL only
     go *bigger* and re-flow the line.
  2. **Kitty's text sizing protocol (OSC 66) genuinely does it** — `OSC 66 ; n=1:d=2 ; text ST` renders at
     half size *inside the same cells* (`n`/`d` fractional scale, `v` for top/bottom/middle alignment), so
     the grid survives. **But only Kitty and Foot implement it.** WezTerm: "does not support the Text
     Sizing protocol". Ghostty 1.3.x *parses* OSC 66 and renders nothing (1.3.0 notes: "not implemented in
     the GUI yet", tracking issue ghostty-org/ghostty#10333, open). tmux strips it even under Kitty. And on
     a terminal that doesn't parse OSC 66 the whole run **including the text** is eaten as an OSC string,
     so the content vanishes — never emit it without a CPR-based support probe.
     `https://sw.kovidgoyal.net/kitty/text-sizing-protocol/`
  3. **The Unicode fallbacks are font-dead.** Checked the real `cmap`s with fontTools:
     small caps — `HackNerdFontMono-Regular` and `SFNSMono` missing **25/26**, Menlo 17/26; superscripts —
     Hack **26/26** missing, SF Mono 24/26, only Menlo covers them (missing `q`); subscripts worse. Digits
     have no small-cap form at all. They render as tofu or fall back to a proportional face and break
     column alignment.
  So the levers that actually work everywhere are **weight, opacity and position**: BOLD full-strength
  name, `th.dim` (the dimmest color the theme has — nothing below it) + `Modifier::DIM`, and a `└ `.
  **The user's terminal is WezTerm** (`TERM_PROGRAM=WezTerm`), with Ghostty 1.3.1 also installed — the
  older entry saying they're on Ghostty is stale.
- **`Modifier::DIM` really is opacity**: `ratatui-crossterm-0.1.2/src/lib.rs:441` maps it to crossterm
  `Attribute::Dim` = **SGR 2 (faint)**, which Ghostty renders by blending fg toward bg. Confirmed on the
  wire, not just in the buffer — drawing the `TestBackend` buffer's cells through a
  `ratatui::prelude::CrosstermBackend::new(&mut Vec<u8>)` and grepping the bytes shows `\e[1m` before the
  label and `\e[2m\e[38;5;8;49m` before the folder. **`TestBackend` alone cannot prove this**; a `Style`
  in the buffer says nothing about whether the backend emits it. Worth the 20 lines when a change's whole
  point is an attribute.
- **`App::new()` starts with `sel_project = 0`, so the first project row is drawn *selected***, and
  `render_button` lifts a `th.dim` fg to `th.muted` there. A style assertion on row 0 that expects
  `th.dim` fails with `left: Some(Gray) right: Some(DarkGray)` and reads like a theme bug. Seed two rows
  and assert the second for the unselected style. The faint attribute survives the lift; the color does
  not.
- **`render_button` took `spans: Vec<Span>` and adding a second `sub: Vec<Span>` fails borrowck**, not
  just clippy: `spans.iter_mut().chain(sub.iter_mut())` errors with "lifetime may not live long enough"
  because `&mut [Span<'a>]` is invariant and the two params infer independent lifetimes. Adding `<'a>` to
  both fixes it but then trips `clippy::too_many_arguments` (8/7). The shape that satisfies both is one
  `text: Vec<Vec<Span<'a>>>` param whose entries take consecutive rows from `text_row` — 7 args, no
  chain. `render_row` passes `vec![spans]`.
- **The PROJECTS PANEL does not scroll** — `rows_rect` returns None past the bottom and `draw_projects`
  `break`s — and `PROJECT_BTN_H` has no callers outside that one function. So a variable row height is
  contained entirely in `draw_projects`; nothing in `event_loop.rs` does row math for this panel.
- **`seed_tree` names its project `demo` at `/tmp/demo`**, so `folder_subtitle()` is None and not one of
  the 462 existing nebula-tui tests grew a row. A new draw test that wants the subtitle must set
  `app.tree.projects[i].repo_path` to something *other* than the name — upserting a `project(id, name, n)`
  alone won't do it, since that helper derives `repo_path` as `/tmp/{name}`.
- **A `TestBackend` buffer line can't be byte-sliced** (`&line[..30]` panics inside `●`/`▌`); use
  `line.chars().take(n)` when dumping the buffer to eyeball a column.
- PROTOCOL VERSION 29 means a **v28 daemon still running from before the build refuses the new client** until it
  is restarted — expected, and the client already offers the kill-and-restart.
