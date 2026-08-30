# PR #19 — Allow the user to rename a project from the projects tab (giving it a alias)

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/19
- **Author:** @lnmunhoz
- **Merged:** 2026-08-28T05:23:56Z by @webdevcody (`f29288ebe330`)
- **Opened:** 2026-08-28T04:49:54Z
- **Branch:** `feat/rename-project` → `main`
- **Diff:** +533 −36 across 11 file(s)

## Description

> Renames a project's row in the Projects panel with `r`, without touching anything on disk.
>
> ## What
>
> A project row has always been named after its folder. `r` on a selected project now retitles that row, and only that row: `repo_path` is untouched, so the checkout, its worktrees, and everything keyed on the path carry on unchanged.
>
> ```
> ▌● Acme API
> ▌  └ acme-repo
> ```
>
> A renamed row keeps its folder visible underneath. An unrenamed row shows nothing extra — the "has it been renamed?" test is derived, not stored (`Project::folder_subtitle` returns the folder name only while it differs from `name`).
>
> **Undo:** submitting an empty name puts the row back on the folder's name. That is the only way back from a rename, and the prompt says so on its label.
>
> Also on the context menu (`m` / right-click) as **Rename**, plus footer hint, help overlay and README rows.
>
> ## Why no migration
>
> `projects.name` was already a plain display column and `repo_path` already carried the folder, so a rename is one `UPDATE`. No schema change, no new field, no flag.
>
> ## On the folder line's styling
>
> A terminal cell has exactly one font size, so "smaller text" is not renderable. The subordination is spelled with the three signals that do work in every terminal and font:
>
> - **weight** — the chosen label is bold at full strength
> - **opacity** — the folder line is the dimmest theme color *plus* `DIM` (SGR 2, faint)
> - **position** — a `└` hangs it off the label, the same tree glyph the metrics modal uses
>
> Kitty's text sizing protocol (OSC 66) can render genuinely half-size text inside the same cells, but only Kitty and Foot implement it. WezTerm does not; Ghostty parses OSC 66 and renders nothing (["not implemented in the GUI yet"](https://ghostty.org/docs/install/release-notes/1-3-0), tracking issue ghostty-org/ghostty#10333). On a terminal that ignores OSC 66 the whole run *including the text* is eaten as an OSC string, so emitting it unprobed would make the folder name vanish. The Unicode fallbacks are font-dead too: small caps are missing 25/26 glyphs in Hack Nerd Font Mono and SF Mono, superscripts 26/26 and 24/26.
>
> ## Protocol
>
> **28 → 29** for `ClientRequest::RenameProject`. A daemon from before this change refuses the new client until it restarts, which the client already handles with its kill-and-restart offer.
>
> ## Notable fix along the way
>
> `submit_prompt` cancels empty input for every `PromptKind` not on an explicit allowlist — it flashes `cancelled: empty input` and returns *before* building the request. That left the daemon's reset-to-folder-name branch unreachable from the UI while its daemon-side test passed. `RenameProject` now joins `NewAgent` and `NewWorktree` on that allowlist.
>
> ## Tests
>
> | Test | Covers |
> |---|---|
> | `rename_project_relabels_the_row_and_leaves_the_folder_alone` | daemon: trims, resets on empty, folder untouched and still on disk |
> | `r_renames_the_selected_project_row` | `r` opens the prompt prefilled, submit sends `RenameProject` |
> | `renaming_a_project_to_nothing_undoes_the_rename` | the empty value reaches the daemon instead of being cancelled |
> | `a_renamed_project_shows_its_folder_name_underneath` | layout **and** style: bold label, faint folder, `└` flush with the label, faint surviving the selection lift |
> | `tui_project_rename_shows_the_folder_and_empty_undoes_it` | real PTY, real binary: rename → assert both lines → clear → assert the row is the folder name again |
>
> The e2e test was A/B'd against the unfixed code: without the allowlist change it fails with `cancelled: empty input` on screen and the row still renamed.
>
> **698 tests pass.** `cargo fmt` clean, no new clippy warnings.
>
> ## Screenshots
>
> Press "R" in a project to rename it:
> <img width="2034" height="1754" alt="CleanShot 2026-08-28 at 11 53 36@2x" src="https://github.com/user-attachments/assets/1d44013a-3eac-4da2-8fb2-28632bc3a7e5" />
>
> After renaming it:
> <img width="1074" height="1730" alt="CleanShot 2026-08-28 at 11 54 03@2x" src="https://github.com/user-attachments/assets/cc618c53-3733-425c-80d5-ee56b0fe17e6" />
>
> In search is also displayed:
> <img width="2066" height="1728" alt="CleanShot 2026-08-28 at 11 55 48@2x" src="https://github.com/user-attachments/assets/a39b452c-dd19-4c14-ae3d-89ee91bcab53" />

## Changed files (11)

- `.claude/MEMORY.md` +95 −3
- `README.md` +1 −0
- `crates/nebula-core/src/entities.rs` +20 −1
- `crates/nebula-core/src/protocol.rs` +9 −1
- `crates/nebula-daemon/src/registry.rs` +58 −6
- `crates/nebula-daemon/src/server.rs` +8 −0
- `crates/nebula-daemon/src/store.rs` +8 −0
- `crates/nebula-tui/src/app.rs` +8 −0
- `crates/nebula-tui/src/event_loop.rs` +236 −3
- `crates/nebula-tui/src/ui.rs` +54 −22
- `crates/nebula/tests/e2e_tui.rs` +36 −0

## Commits (3)

- `7b4201b482a7` Rename a project's row with r, keeping the folder name under it — @lnmunhoz
- `eeaa35c3982e` Log the project rename and the OSC 66 dead end — @lnmunhoz
- `c570e8b577cc` Merge main into feat/rename-project — @webdevcody

## Conversation (1)

### @webdevcody · 2026-08-28T05:12:03Z

> ## PR Security Audit — #19 Allow the user to rename a project from the projects tab (giving it a alias)
>
> **Risk: 1/5 — Minimal**
>
> **Verdict:** No security blockers
>
> **Confidence:** High
>
> ### Findings
>
> **No material security findings.**
>
> ### Sensitive paths reviewed
>
> - **IPC trust boundary:** `ClientRequest::RenameProject` is protocol-gated and frame-bounded (`crates/nebula-core/src/protocol.rs:101`, `crates/nebula-core/src/codec.rs:40`). The Unix socket lives under the daemon's existing `0700` runtime directory, so this remains a same-OS-user control plane (`crates/nebula-daemon/src/lifecycle.rs:114`).
> - **Authorization/ownership:** the request identifies a project by opaque `ProjectId` and checks that it exists before mutation (`crates/nebula-daemon/src/registry.rs:723`). Workspaces are UI groupings, not tenant/security boundaries; the new handler matches the existing project CRUD trust model.
> - **Injection:** the SQLite update uses bound parameters, so the name cannot alter the query (`crates/nebula-daemon/src/store.rs:414`). The name is not used in filesystem paths, Git commands, shell arguments, subprocess environment, URLs, or HTTP requests.
> - **Terminal safety:** pasted text can contain control characters, but Ratatui filters control characters and zero-width graphemes before cells reach the Crossterm backend. Project names are otherwise truncated at their display sites (`crates/nebula-tui/src/ui.rs:2602`, `crates/nebula-tui/src/ui.rs:3562`).
> - **Persistence/data flow:** rename changes only `projects.name`; `repo_path`, worktree paths, and path-keyed reconciliation remain unchanged (`crates/nebula-daemon/src/registry.rs:726`).
>
> ### Residual risk / questions
>
> - There is no small semantic length limit or explicit control-character rejection for project names; the IPC frame limit is the effective upper bound. A smaller shared label validator would be reasonable defense in depth, but this is not scored as a vulnerability: the endpoint is same-user-only, stored SQL is parameterized, rendering drops terminal controls, and the value never reaches an execution sink.
> - This was a static review plus targeted local testing, not hostile runtime fuzzing of the MessagePack socket.
>
> ### Checks performed
>
> - Exact submitted range reviewed: `5b662984ada89f07859a9a7c9ae3be23a69dfeaf...eeaa35c3982e3a7243819ce1468412fdc500337f`.
> - `cargo test -p nebula-daemon rename_project_relabels_the_row_and_leaves_the_folder_alone` — **passed** (1/1).
> - The three focused `nebula-tui` project-rename unit tests — **passed** (3/3).
> - `cargo test -p nebula --test e2e_tui tui_project_rename_shows_the_folder_and_empty_undoes_it` — **passed** (1/1 real PTY test).
> - `cargo fmt --all -- --check` and `git diff --check <base>...<head>` — **passed**.
> - Added-line credential/dangerous-primitive scan — **no credential material found**; no dependency, lockfile, migration, install-script, or workflow changes to audit.
>
> ### Rating rationale
>
> This PR adds a local metadata mutation with no new remote exposure or privileged execution path. The request is bounded and same-user, the database write is parameterized, the renamed value is display-only, terminal controls are filtered by the rendering layer, and the exact feature tests pass. The absence of a tighter display-label validator is a low-value hardening opportunity rather than a reachable security issue, so the PR rates **1/5 — Minimal**.
>
> <!-- pr-security-audit -->

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
