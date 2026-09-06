# PR #30 — An Experimental "Hide root worktree" setting, a quick prompt that cuts a fresh worktree, and worktrees branched from the fetched origin/HEAD

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/30
- **Author:** @webdevcody
- **Merged:** 2026-09-06T04:09:44Z by @webdevcody (`f31bcd61cbe0`)
- **Opened:** 2026-09-06T03:13:49Z
- **Branch:** `hide-root-auto-worktree` → `main`
- **Diff:** +1120 −192 across 38 file(s)

## Description

> A new Experimental SETTING keeps the shared checkout out of the WORKTREES PANEL and turns `p` there into "a fresh WORKTREE, then this task in it" — and every new worktree, however it is created, now starts at what `origin` has rather than at the root checkout's stale HEAD.
>
> **Contents**
> - [✨ What you get](#user-content-what-you-get)
>   - [🌱 Every session in its own worktree](#user-content-every-session-in-its-own-worktree)
>   - [🔄 Worktrees that start at remote main](#user-content-worktrees-that-start-at-remote-main)
> - [📸 Screenshots](#user-content-screenshots)
> - [🧭 How it flows](#user-content-how-it-flows)
> - [⚠️ Risk](#user-content-risk)
> - [🔧 Technical overview](#user-content-technical-overview)
> - [📝 Notes](#user-content-notes)
>
> ## ✨ What you get <a id="what-you-get"></a>
>
> ### 🌱 Every session in its own worktree <a id="every-session-in-its-own-worktree"></a>
>
> - **Hide the root row** — `Settings › Experimental › Hide root worktree` (`hide_root_worktree`, off by default)
>   - the ROOT WORKTREE row leaves the WORKTREES PANEL
>   - the checkout and its sessions keep running, and stay reachable from the PALETTE (`/`)
>   - switch it off and the row is straight back
> - **`p` cuts a worktree first** — with the setting on, `p` while the WORKTREES PANEL has FOCUS
>   - opens the QUICK PROMPT titled `Quick prompt · new worktree yellow-fox-jumps (claude)` — the same random name the `n` prompt offers
>   - `Enter` creates the WORKTREE, moves the cursor onto its row, and starts the agent there with your text as its STARTING PROMPT
>   - a second `p` from the SESSIONS PANEL lands in that same checkout
>   - a worktree the DAEMON refuses brings the box back with your text
> - **Everything else is as it was**
>   - `p` from the PROJECTS PANEL or the SESSIONS PANEL still launches into the selected worktree
>   - FOCUS stays on the panel you pressed `p` in, unless `Settings › Agents › Quick prompt › Focus` is on
>   - `Tab` and `Shift+Tab` still retarget one launch and hand the box back, new-worktree target intact
>
> ### 🔄 Worktrees that start at remote main <a id="worktrees-that-start-at-remote-main"></a>
>
> - **Fetched `origin/HEAD`, not local HEAD** — `n` in the WORKTREES PANEL, a bare `nebula worktree`, and the flow above
>   - before: `git worktree add -b <branch>` with no start point, so every branch began at the root checkout's HEAD — two commits behind `origin/main` when this task started
>   - now: a base-less create fetches `origin` and branches from its default branch (`origin/main` here), so the new checkout already has what everyone else merged
> - **Untracked on purpose** — the new branch is cut `--no-track`
>   - a branch cut from `origin/main` would otherwise *track* it, and a bare `git push` would aim at main
>   - an explicit `nebula worktree --base <ref>` keeps git's tracking, as the PR SESSION path relies on
> - **Offline still works** — no `origin`, or a fetch that fails or stalls past 30 s
>   - falls back to HEAD, with a warning in the daemon log
>   - a worktree cut offline beats none
>
> ## 📸 Screenshots <a id="screenshots"></a>
>
> | The Experimental tab of the SETTINGS OVERLAY, its one row switched on | The WORKTREES PANEL without its root row, and `p` there naming the worktree it will cut |
> |---|---|
> | ![The SETTINGS OVERLAY open on the Experimental tab: the "Hide root worktree" row reads on, with its hint about p on Worktrees cutting a fresh worktree off origin/main](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/hide-root-auto-worktree/settings-experimental.png) | ![The WORKTREES PANEL listing only feature-x and wheel-one-line, no ⌂ root row, with the QUICK PROMPT open and titled "Quick prompt · new worktree <name> (claude)"](https://raw.githubusercontent.com/AgentSystemLabs/nebula/pr-assets/hide-root-auto-worktree/quick-prompt-new-worktree.png) |
>
> Captured with the SCREENSHOT HARNESS (`make shot`) at 190×50 against its demo repo, whose root and two worktrees stand in for a real project.
>
> ## 🧭 How it flows <a id="how-it-flows"></a>
>
> ```mermaid
> sequenceDiagram
>   actor U as User
>   participant T as TUI · WORKTREES PANEL
>   participant D as DAEMON
>   participant G as git
>   U->>T: p (hide_root_worktree on)
>   T-->>U: QUICK PROMPT · new worktree ‹branch›
>   U->>T: task, Enter
>   T->>D: CreateWorktree { branch, base: None }
>   D->>G: fetch origin
>   D->>G: origin/HEAD (set-head --auto when unset)
>   D->>G: worktree add --no-track -b ‹branch› origin/main
>   D-->>T: Ack + worktree upsert
>   T->>T: cursor onto the new row, FOCUS unchanged
>   T->>D: CreateAgent { worktree, starting_prompt }
>   D-->>T: session row, previewed in the TERMINAL PANE
> ```
>
> The same `CreateWorktree { base: None }` is what `n` sends and what `nebula worktree` without `--base` reaches, so all three start at the fetched `origin/HEAD`.
>
> ## ⚠️ Risk <a id="risk"></a>
>
> **Verdict:** 🟢 Low risk — the setting is off by default, and the one always-on change (where a new worktree starts) has an offline fallback and its own tests.
>
> | | Level | Why |
> |---|---|---|
> | 🔒 **Security & production** | Low | No new `ClientRequest`, route or file. The DAEMON now runs `git fetch origin` on every base-less worktree create — the same network call, with the same credentials, that the PR SESSION path already makes; it is killed after 30 s so a stalled remote cannot hold the worktree lock. |
> | ⚡ **Performance** | Low | Off every hot path. One fetch per worktree creation (seconds, network-bound, under the DAEMON's `worktree_ops` lock); the panel filter is one boolean per row. |
> | 🧩 **Fit with the codebase** | Low | The SETTING follows the seven-edit `config.rs` pattern plus the `apply_config` mirror; the two-step launch rides a `PendingIntent` like every other create; the new step lives in its own `event_loop/quick_launch.rs` per KEEP MODULES SMALL. The one wider touch: the quick launch's `worktree` became a `target` enum, so the `Tab` / `Shift+Tab` pickers now take a context checkout explicitly. |
>
> **Rollback:** `git revert` of the merge commit undoes all of it — no PROTOCOL VERSION bump, no MIGRATION. A `config.json` that already carries `hide_root_worktree` is ignored by older builds. Worktrees already cut from `origin/HEAD` simply stay where they are.
>
> ## 🔧 Technical overview <a id="technical-overview"></a>
>
> - **Mechanism.** `QuickLaunch.target` is a `QuickTarget` — `Worktree(id)` or `NewWorktree { project, branch }`; `open_quick_prompt` picks the latter when `hide_root_worktree` is on and FOCUS is on the WORKTREES PANEL. `quick_launch::submit` sends `CreateWorktree { base: None }` with a `PendingIntent::LaunchInCreatedWorktree { launch, text }`; the Ack retargets the launch at the created id, selects its row, restores FOCUS and calls `create_agent`; an Error reopens the box. The pickers get a context checkout from `picker_context` (the hidden root for a new-worktree target) and rebuild the launch from `back.launch.target`, never from that context. On the DAEMON, `registry::create_worktree` — the choke point for the TUI and for `nebula worktree` — routes a base-less create through `git::add_worktree_off_default` → `git::default_base` (`remote get-url origin`, `fetch --quiet origin` with `kill_on_drop` and a 30 s timeout, `symbolic-ref origin/HEAD` with one `remote set-head origin --auto`), cutting with `--no-track`.
> - **Files.** `crates/nebula-daemon/src/git.rs` — `default_base`, `fetch_origin`, `origin_head`, `add_worktree_off_default`, three tests; `crates/nebula-daemon/src/registry.rs` — the base-less branch in `create_worktree`; `crates/nebula-tui/src/quick_prompt.rs` — `QuickTarget`, the new open path, `picker_context`; `crates/nebula-tui/src/event_loop/quick_launch.rs` — submit, Ack and the shared draft; `crates/nebula-tui/src/event_loop.rs` — the Ack and Error arms, `set_hide_root_worktree`, four tests; `crates/nebula-tui/src/config.rs` — the SETTING and its Experimental tab; `crates/nebula-tui/src/app.rs` — the mirror, `visible_worktrees`, `project_branches`, the intent.
> - **Not done.** Slugifying the task into the branch name (a task is a paragraph; the random name is the documented empty-prompt path). Moving FOCUS into the SESSIONS PANEL after the create (every QUICK PROMPT launch leaves FOCUS where it was — settled on 2026-08-29). A TUI notice when the fetch falls back to HEAD (`apply_config` has no request channel; the daemon log carries the warning).
> - **Gate.** `make ci` green in the task's worktree: fmt, clippy, memory-check, recall-eval, terms-check, and the suites — nebula-tui 607, nebula-daemon 196, e2e_pty 29, e2e_tui 8. New tests: `git.rs` ×3 (no origin; a fetch that fails; an origin that moved behind the checkout's back, asserting the new HEAD and no upstream), `config.rs` ×1, `event_loop.rs` ×3 (root row hidden and the cursor clamped; `p` on Worktrees creates then launches, through the `Tab` round trip and the error path; the default unchanged), `quick_prompt.rs` ×1. Not run: the live DEV INSTANCE — restarting the daemon would have killed the session doing the work.
>
> ## 📝 Notes <a id="notes"></a>
>
> - `origin/main` (v0.22.0 plus its release log) is merged in. Conflicts on the MEMORY LOG index and the Candidates ledger, both sides having prepended a line; resolved by keeping both. The auto-merge left the standing gotchas at 301 of 300 lines, fixed by merging a twin pair in the RELEASE SKILL group.
> - Nothing to do on upgrade: no PROTOCOL VERSION bump, no MIGRATION.
> - Also on this branch: the MEMORY LOG entry for the task with three standing gotchas (the `--base origin/main` tracking trap, the missing `origin/HEAD` symref, the picker round-trip retarget); three skill edits from the SKILL AUDIT HOOK; and `CLAUDE.md`, `AGENTS.md` and the `land` skill now route every PR creation through the PR DESCRIPTION SKILL — this PR's first body was the short hand-written kind that rule now forbids.
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
>
> https://claude.ai/code/session_015VpqzNV3TGkDjrebL3mgkU

## Changed files (38)

- `.claude/MEMORY.md` +2 −0
- `.claude/memory/entries/2026-09-04-pr-body-headings-get-no-github-anchors-toc-links-a-id.md` +1 −1
- `.claude/memory/entries/2026-09-05-hide-root-worktree-setting-p-cuts-a-worktree-off-origin-head.md` +88 −0
- `.claude/memory/entries/2026-09-05-make-pr-hand-written-body-land-deferred-pr-description.md` +60 −0
- `.claude/memory/gotchas.md` +10 −9
- `.claude/skills/land/SKILL.md` +14 −9
- `.claude/skills/output-doctor/SKILL.md` +0 −24
- `.claude/skills/pr-description/SKILL.md` +34 −34
- `.claude/skills/pr-description/templates/01-benefit-groups.md` +20 −12
- `.claude/skills/pr-description/templates/02-before-after.md` +1 −1
- `.claude/skills/pr-description/templates/03-category-table.md` +1 −1
- `.claude/skills/pr-description/templates/04-story.md` +1 −1
- `.claude/skills/pr-description/templates/05-release-note.md` +1 −1
- `.claude/skills/pr-description/templates/06-user-journey.md` +1 −1
- `.claude/skills/pr-description/templates/07-collapsible.md` +1 −1
- `.claude/skills/pr-description/templates/08-bug-fix.md` +1 −1
- `.claude/skills/pr-description/templates/09-scorecard.md` +1 −1
- `.claude/skills/pr-description/templates/10-crate-map.md` +1 −1
- `.claude/skills/project-terms/SKILL.md` +2 −1
- `.claude/skills/prompt-daddy/SKILL.md` +4 −1
- `AGENTS.md` +2 −1
- `CLAUDE.md` +2 −1
- `TERMS.md` +18 −13
- `crates/nebula-daemon/src/git.rs` +210 −2
- `crates/nebula-daemon/src/registry.rs` +8 −1
- `crates/nebula-tui/src/agent_picker.rs` +6 −4
- `crates/nebula-tui/src/app.rs` +29 −1
- `crates/nebula-tui/src/config.rs` +56 −0
- `crates/nebula-tui/src/event_loop.rs` +300 −33
- `crates/nebula-tui/src/event_loop/quick_launch.rs` +83 −0
- `crates/nebula-tui/src/preset_overlays.rs` +1 −1
- `crates/nebula-tui/src/quick_prompt.rs` +129 −29
- `crates/nebula-tui/src/ui.rs` +3 −1
- `docs/configuration.md` +4 −2
- `docs/how-it-works.md` +3 −1
- `docs/keys.md` +2 −2
- `scripts/shot/scenes/quick-prompt-new-worktree.keys` +12 −0
- `scripts/shot/scenes/settings-experimental.keys` +8 −0

## Commits (6)

- `9b6a46664190` An Experimental "Hide root worktree" SETTING, a QUICK PROMPT that cut… — @webdevcody, @claude
- `d35d93f6c57a` Merge origin/main into hide-root-auto-worktree — @webdevcody
- `0a0cb9a2707a` Record the post-merge gotchas-cap overflow in the MEMORY LOG entry — @webdevcody, @claude
- `aa18cd5514fb` Route every PR body through the PR DESCRIPTION SKILL, and make its TO… — @webdevcody, @claude
- `3ccb11f69d02` PR bodies: a hook plus one-fact sub-bullets, and two skill-audit tigh… — @webdevcody, @claude
- `40dcd41835bd` The hidden-root cursor follows its row, and the set-head round-trip i… — @webdevcody, @claude

## Conversation (0)

_(no issue comments)_

## Reviews (4)

### @claude · COMMENTED · 2026-09-06T03:26:38Z

_(empty)_

### @claude · COMMENTED · 2026-09-06T03:26:45Z

_(empty)_

### @webdevcody · COMMENTED · 2026-09-06T04:09:40Z

_(empty)_

### @webdevcody · COMMENTED · 2026-09-06T04:09:41Z

_(empty)_

## Inline review comments (4)

### `crates/nebula-tui/src/event_loop.rs:3982` — @claude[bot] · 2026-09-06T03:26:38Z

```diff
@@ -3954,6 +3949,19 @@ fn set_hide_worktrees(app: &mut App, hidden: bool) {
     }
 }
 
+/// Show or hide the ROOT WORKTREE row (Settings → Experimental). Hiding it
+/// shortens the list by one from the top, so a cursor past the new end
+/// is pulled back onto the last row; the pane catches up on the next
+/// move, as it does after any re-sort.
+fn set_hide_root_worktree(app: &mut App, hidden: bool) {
+    if app.hide_root_worktree == hidden {
+        return;
+    }
+    app.hide_root_worktree = hidden;
+    let last = app.worktree_row_count().saturating_sub(1);
+    app.sel_worktree = app.sel_worktree.min(last);
+}
+
 fn save_panel_visibility(app: &mut App) {
     let mut cfg = crate::config::Config::load();
```

> **Bug:** `set_hide_root_worktree` only clamps the cursor when it falls past the new list length — it doesn't account for the fact that removing the ROOT WORKTREE row shifts every remaining row's index down by one (and restoring it shifts everything up by one).
>
> Concrete trace with `visible_worktrees() == [main(root), A, B, C]` and `sel_worktree == 1` (cursor on A): toggling hide-root on gives `worktree_row_count() == 3`, `last == 2`, `sel_worktree = min(1, 2) == 1`. But the new list is `[A, B, C]`, so index 1 is now **B** — the cursor silently moved off A even though no clamping was actually needed.
>
> Worse, toggling the setting back **off** always shifts the cursor one worktree earlier for every value (since the list grows by one at the front): from `sel_worktree == 0` (A) it lands back on **main**, the ROOT WORKTREE row this setting exists to keep users off of. The same issue affects `selected_worktree_pr()`, which can silently switch the PR preview to a different pull request.
>
> The rest of the codebase already handles this class of problem correctly: `reconcile_selection_inner` snapshots the selected `WorktreeId` and re-finds it by identity after any list change, rather than clamping a stale index. `set_hide_root_worktree` should do the same — snapshot the selected worktree (or PR) before flipping the setting, then re-find it afterward (or shift the index by ±1 when the cursor wasn't on the root row itself).
>
> The new test (`hide_root_worktree_drops_the_root_row_from_the_worktrees_panel`) only seeds 2 worktrees, where "clamp" and "correct" produce the same answer, so it can't catch this. A 3+-worktree fixture asserting the cursor still names the same branch across both toggle directions would catch it.
>
> https://github.com/AgentSystemLabs/nebula/blob/0a0cb9a2707aef22d52577163afcd99d08460c45/crates/nebula-tui/src/event_loop.rs#L3949-L3966

### `crates/nebula-daemon/src/git.rs:317` — @claude[bot] · 2026-09-06T03:26:45Z

```diff
@@ -201,6 +230,82 @@ pub async fn add_worktree(repo: &Path, branch: &str, base: Option<&str>) -> Resu
     }
 }
 
+/// How long `default_base` waits for `git fetch origin` before branching
+/// from local HEAD instead. The fetch holds the DAEMON's worktree lock,
+/// so a stalled connection must not hold every worktree op with it.
+const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
+
+/// The start point for a new branch when the caller named none: the
+/// remote's default branch, fetched first, so it is what `origin` has
+/// right now (`origin/main` for most repos) and not what the ROOT WORKTREE
+/// happens to have pulled. `None` — git's own default, the checkout's
+/// HEAD — when the repo has no `origin` or the fetch fails, which the
+/// daemon log says; a worktree cut offline is better than none.
+pub async fn default_base(repo: &Path) -> Option<String> {
+    if git(repo, &["remote", "get-url", "origin"]).await.is_err() {
+        return None;
+    }
+    if let Err(e) = fetch_origin(repo).await {
+        tracing::warn!(
+            repo = %repo.display(),
+            error = %e,
+            "fetch before worktree add failed; branching from local HEAD"
+        );
+        return None;
+    }
+    origin_head(repo).await
+}
+
+/// `git fetch origin`, killed and reported as an error past `FETCH_TIMEOUT`.
+async fn fetch_origin(repo: &Path) -> Result<()> {
+    let run = Command::new("git")
+        .arg("-C")
+        .arg(repo)
+        .args(["fetch", "--quiet", "origin"])
+        .kill_on_drop(true)
+        .output();
+    let output = match tokio::time::timeout(FETCH_TIMEOUT, run).await {
+        Ok(output) => output.map_err(spawn_err)?,
+        Err(_) => bail!(
+            "git fetch origin did not finish within {}s",
+            FETCH_TIMEOUT.as_secs()
+        ),
+    };
+    if !output.status.success() {
+        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
+    }
+    Ok(())
+}
+
+/// `origin/HEAD` as a short ref (`origin/main`). A repo whose remote was
+/// added by hand (`git remote add`, a fresh push) has no such symref, so
+/// one `git remote set-head origin --auto` asks the remote which branch
+/// it means and records the answer for next time. None only when the
+/// remote itself has no HEAD.
+async fn origin_head(repo: &Path) -> Option<String> {
+    for attempt in 0..2 {
+        if let Ok(out) = git(
+            repo,
+            &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
+        )
+        .await
+        {
+            let short = out.trim();
+            if !short.is_empty() {
+                return Some(short.to_string());
+            }
+        }
+        if attempt == 0
+            && git(repo, &["remote", "set-head", "origin", "--auto"])
+                .await
+                .is_err()
+        {
+            return None;
+        }
```

> **Bug:** this `git remote set-head origin --auto` call is a network round-trip (it queries the remote to determine its HEAD when no local symref exists — exactly the case this function exists to handle), but it goes through the plain `git()` helper, which has no timeout or `kill_on_drop`. That's inconsistent with `fetch_origin` just above, which this PR deliberately wraps in `FETCH_TIMEOUT` (30s) with `kill_on_drop(true)` because, per its own comment, "the fetch holds the DAEMON's worktree lock, so a stalled connection must not hold every worktree op with it."
>
> `create_worktree` (`registry.rs`) holds `self.worktree_ops` (a `tokio::sync::Mutex<()>`) across the entire call chain into `default_base` → `origin_head`, so a hang on this `set-head` call (e.g. a dropped connection between the fetch succeeding and this call, or a credential prompt with no controlling tty) wedges every worktree operation — `pr_worktree`, `delete_worktree`, and the background auto-sync all serialize on the same mutex. It also stalls the client's own request loop, since `CreateWorktree` is awaited inline there rather than spawned off (unlike `DeleteWorktree`).
>
> Suggest routing this call through a shared timeboxed helper (the same `FETCH_TIMEOUT` + `kill_on_drop(true)` pattern used for `fetch_origin`) so a stalled `set-head` degrades to `None` (branch from local HEAD) instead of hanging indefinitely, matching the fallback behavior this function already implements for every other failure mode.
>
> https://github.com/AgentSystemLabs/nebula/blob/0a0cb9a2707aef22d52577163afcd99d08460c45/crates/nebula-daemon/src/git.rs#L296-L304

### `crates/nebula-tui/src/event_loop.rs:3982` — @webdevcody · 2026-09-06T04:09:40Z · reply

```diff
@@ -3954,6 +3949,19 @@ fn set_hide_worktrees(app: &mut App, hidden: bool) {
     }
 }
 
+/// Show or hide the ROOT WORKTREE row (Settings → Experimental). Hiding it
+/// shortens the list by one from the top, so a cursor past the new end
+/// is pulled back onto the last row; the pane catches up on the next
+/// move, as it does after any re-sort.
+fn set_hide_root_worktree(app: &mut App, hidden: bool) {
+    if app.hide_root_worktree == hidden {
+        return;
+    }
+    app.hide_root_worktree = hidden;
+    let last = app.worktree_row_count().saturating_sub(1);
+    app.sel_worktree = app.sel_worktree.min(last);
+}
+
 fn save_panel_visibility(app: &mut App) {
     let mut cfg = crate::config::Config::load();
```

> Fixed in 40dcd41: `set_hide_root_worktree` now snapshots the selected worktree (or OPEN PRS row, by URL) before the flip and re-finds it by identity afterwards, the way `reconcile_selection` does — a cursor on the root itself lands on the first row left, never past the end. The test now seeds three worktrees and asserts the cursor still names `feat` after hiding, after showing again (the root does not get the cursor back), and after starting on the root row.

### `crates/nebula-daemon/src/git.rs:317` — @webdevcody · 2026-09-06T04:09:41Z · reply

```diff
@@ -201,6 +230,82 @@ pub async fn add_worktree(repo: &Path, branch: &str, base: Option<&str>) -> Resu
     }
 }
 
+/// How long `default_base` waits for `git fetch origin` before branching
+/// from local HEAD instead. The fetch holds the DAEMON's worktree lock,
+/// so a stalled connection must not hold every worktree op with it.
+const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
+
+/// The start point for a new branch when the caller named none: the
+/// remote's default branch, fetched first, so it is what `origin` has
+/// right now (`origin/main` for most repos) and not what the ROOT WORKTREE
+/// happens to have pulled. `None` — git's own default, the checkout's
+/// HEAD — when the repo has no `origin` or the fetch fails, which the
+/// daemon log says; a worktree cut offline is better than none.
+pub async fn default_base(repo: &Path) -> Option<String> {
+    if git(repo, &["remote", "get-url", "origin"]).await.is_err() {
+        return None;
+    }
+    if let Err(e) = fetch_origin(repo).await {
+        tracing::warn!(
+            repo = %repo.display(),
+            error = %e,
+            "fetch before worktree add failed; branching from local HEAD"
+        );
+        return None;
+    }
+    origin_head(repo).await
+}
+
+/// `git fetch origin`, killed and reported as an error past `FETCH_TIMEOUT`.
+async fn fetch_origin(repo: &Path) -> Result<()> {
+    let run = Command::new("git")
+        .arg("-C")
+        .arg(repo)
+        .args(["fetch", "--quiet", "origin"])
+        .kill_on_drop(true)
+        .output();
+    let output = match tokio::time::timeout(FETCH_TIMEOUT, run).await {
+        Ok(output) => output.map_err(spawn_err)?,
+        Err(_) => bail!(
+            "git fetch origin did not finish within {}s",
+            FETCH_TIMEOUT.as_secs()
+        ),
+    };
+    if !output.status.success() {
+        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
+    }
+    Ok(())
+}
+
+/// `origin/HEAD` as a short ref (`origin/main`). A repo whose remote was
+/// added by hand (`git remote add`, a fresh push) has no such symref, so
+/// one `git remote set-head origin --auto` asks the remote which branch
+/// it means and records the answer for next time. None only when the
+/// remote itself has no HEAD.
+async fn origin_head(repo: &Path) -> Option<String> {
+    for attempt in 0..2 {
+        if let Ok(out) = git(
+            repo,
+            &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
+        )
+        .await
+        {
+            let short = out.trim();
+            if !short.is_empty() {
+                return Some(short.to_string());
+            }
+        }
+        if attempt == 0
+            && git(repo, &["remote", "set-head", "origin", "--auto"])
+                .await
+                .is_err()
+        {
+            return None;
+        }
```

> Fixed in 40dcd41: the `set-head --auto` call goes through the same timebox as the fetch — a `git_remote` helper with `kill_on_drop(true)` and the (renamed) 30 s `REMOTE_TIMEOUT` now backs both `fetch_origin` and `origin_head` — so a remote that stops answering between the two calls degrades to `None` and a branch from local HEAD, and the worktree lock is released either way.
