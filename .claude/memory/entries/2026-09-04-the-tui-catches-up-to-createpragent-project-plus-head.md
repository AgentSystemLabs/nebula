# The TUI Catches Up To `CreatePrAgent`'s `project` + `head` — 2026-09-04

**Asked:** "make dev fails with error[E0559]: variant `ClientRequest::CreatePrAgent` has no field
named `worktree` … now, fix it, pull latest from main, merge ito here, commit EVERYTHING AND PUSH to
main, then fix https://github.com/AgentSystemLabs/nebula/pull/26 to get passing and merge"
→ refined: "`make dev` fails to compile: `crates/nebula-tui/src/event_loop.rs:5421` still passes
`worktree` to `ClientRequest::CreatePrAgent`, which this SHARED CHECKOUT's uncommitted DAEMON-side PR
SESSION work changed to `project` + `head`. Update the TUI call site to the new shape (keep the
DAEMON's new PROTOCOL as-is — don't revert it), get `cargo check --workspace` and MAKE CI green, then
pull `origin/main`, merge it into this tree, commit everything here including the other session's
daemon work, and push to `main`. Then get PR #26 (`session-title-sync`) passing CLAUDE REVIEW and
merge it." (no questions asked)

**Did:** Another session had left the DAEMON half of PROTOCOL VERSION 38 uncommitted in the SHARED
CHECKOUT — a PR SESSION no longer runs in the ROOT WORKTREE, it gets the PROJECT's checkout of the
PR's head branch (`registry.rs::pr_worktree`, `git.rs::add_pr_worktree`, `pr_scope::CreatePrAgentSpec`)
— so `CreatePrAgent` had swapped `worktree: WorktreeId` for `project: ProjectId` + `head: String`
while nebula-tui still sent the old shape. Completed their change on the client side rather than
reverting it: `pull_request.rs::OpenPr` gains `head` from `gh pr list --json …,headRefName`; the new
`pull_request::PrLaunch { url, head }` replaces the bare `pr_url: Option<String>` that rode
`KindPicker`, `MenuAction::NewAgentOfKind`, `PromptKind::NewAgent` and `AgentLaunchDraft`, so the URL
and its branch can never travel apart; `event_loop.rs::project_of_worktree` resolves the PROJECT at
the send site (the picker's ROOT WORKTREE now only names *which project*, not where the session
lands). `cargo check --workspace --all-targets` clean, 880 tests passed across nine binaries, STRICT
clippy and `cargo fmt` clean, `make memory-check recall-eval terms-check` ok.

**Gotchas:**
- **`land_pending_selection` already re-seats the WORKTREES PANEL cursor onto the created AGENT's own
  worktree** (`event_loop.rs:4695`, the `landed_worktree` fallback after the visible-rows miss), so
  the DAEMON relocating a PR SESSION into a freshly created checkout needed no TUI selection work at
  all — the row lands and attaches. Check that fallback before adding one.
- **`cargo fmt --all` in the SHARED CHECKOUT reformats the *other* session's uncommitted lines too.**
  `git.rs` went +101 → +110 and `pr_scope.rs` +250 → +262 without a single edit of mine in either
  file. Diff the stat before and after fmt, or you will read someone else's reflow as your own work.
- **The compile error named one site; `--all-targets` named three more.** `cargo check --workspace`
  alone stops at the lib (`event_loop.rs:5421`); the `E0026` pattern-match in the test module and the
  seven `OpenPr` literals in `event_loop.rs`'s test seeds only appear with `--all-targets`, which is
  the same trap as the standing "test-only struct literals drift silently" MAKE CI line.
