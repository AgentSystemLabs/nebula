# PR #29 — A merged or closed PR ROW goes; drafts sink dimmed to the bottom of the PROJECT OPEN PRS GROUP

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/29
- **Author:** @webdevcody
- **Merged:** 2026-09-05T03:55:46Z by @webdevcody (`201faee7bc77`)
- **Opened:** 2026-09-05T03:55:25Z
- **Branch:** `pr-rows-drafts-last` → `main`
- **Diff:** +299 −102 across 12 file(s)

## Description

> ## Summary
>
> The SESSIONS PANEL's WORKTREE OPEN PRS GROUP kept showing a PR ROW for a worktree's branch after that pull request was merged or closed: `gh pr view` with no argument answers with the branch's *latest* PR whatever its state, and the row carried a `merged` / `closed` badge under an `OPEN PRS` header. A merged or closed PR is now an ordinary "no PR", so the row disappears on the next GIT POLL beat.
>
> In the PROJECT OPEN PRS GROUP (the WORKTREES PANEL's `OPEN PRS · n` group), drafts now sort after every open PR, newest first within each half, and render dimmed end to end with the DRAFT BADGE's `draft` text kept in front of the title. A draft PR ROW in the SESSIONS PANEL gets the same look.
>
> ## What changed
>
> - `pull_request.rs::parse` returns `None` unless `state` is `OPEN`; `PullRequest.state` and `is_open()` are gone, `badge()` is `draft` / `pr`.
> - New `pull_request::drafts_last` (a stable `sort_by_key(is_draft)`) runs in `event_loop.rs::note_open_prs_answer`, the one path every list lands through. The URL-following cursor reconcile absorbs the reorder.
> - New module `crates/nebula-tui/src/pr_row.rs` (`look`, `spans`): both `ui.rs::draw_worktrees`'s `WorktreeEntry::Row` arm and `draw_session_row`'s `SessionRow::Link` arm build through it. A draft is `th.dim` (the PR PREVIEW's `draft` role); an open PR keeps `th.accent`.
> - Docs: `docs/sessions.md`, `docs/how-it-works.md`. MEMORY LOG entry, standing gotcha and `TERMS.md` rows for PR ROW, DRAFT BADGE and PROJECT OPEN PRS GROUP.
>
> ## Risk
>
> TUI-only, no PROTOCOL VERSION change and no DAEMON change. The visible behavior change is that a worktree whose branch's PR was merged or closed shows no PR ROW at all, where it used to show a `merged` / `closed` badge. "Show label" was read as the DRAFT BADGE text, not GitHub labels (no PR on this repo has carried one).
>
> ## Tests
>
> `a_merged_or_closed_branch_pull_request_is_no_row`, `drafts_sink_below_the_open_rows_in_their_own_order`, two tests in `pr_row.rs` (every THEME preset; badge before title), `draft_pull_requests_render_at_the_bottom_of_the_group`. `the_cursor_follows_its_pull_request_across_a_reorder` now seeds an open PR as the newest row. MAKE CI green locally.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_01KsZp1cfBa4Kz2eXu4U2uz5

## Changed files (12)

- `.claude/MEMORY.md` +1 −0
- `.claude/memory/entries/2026-09-04-merged-pr-rows-go-drafts-sink-dimmed-to-the-bottom.md` +51 −0
- `.claude/memory/gotchas.md` +2 −2
- `TERMS.md` +5 −4
- `crates/nebula-tui/src/app.rs` +3 −2
- `crates/nebula-tui/src/event_loop.rs` +23 −13
- `crates/nebula-tui/src/lib.rs` +1 −0
- `crates/nebula-tui/src/pr_row.rs` +119 −0
- `crates/nebula-tui/src/pull_request.rs` +70 −43
- `crates/nebula-tui/src/ui.rs` +20 −36
- `docs/how-it-works.md` +2 −1
- `docs/sessions.md` +2 −1

## Commits (1)

- `2e10a62f350c` A merged or closed PR ROW goes; drafts sink dimmed to the bottom of t… — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
