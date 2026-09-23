---
name: release
description: "Cut a nebula release — verify the tree is green, commit, bump the workspace version, tag, push, and replace the auto-generated GitHub release notes with a real changelog. Use when the user says \"do a release\", \"cut a release\", \"release this\", \"ship it\", \"commit and push and release\", \"commit, push, release\", asks for a new version, or asks to write or rewrite the release notes / release description for a tag. Also use when they ask what the release process is."
user-invocable: true
---

Nebula releases are tag-driven: pushing a `v*` tag makes `.github/workflows/release.yml` build the
cross-compile matrix and publish a GitHub release with the binaries attached. Everything before the tag
push is your job; everything after it is CI's, except the changelog, which CI gets wrong.

Work through the steps in order. Do not skip the green gate. If the user only wants the release notes
for a tag that already exists, jump to step 7.

## 1. Preflight — assume you are not alone in this tree

**Other agents edit this repo concurrently, and they will fight you for the index.** All of these
have happened while cutting a release:

- Files you never opened turn up modified mid-task (`git status` from two minutes ago proves nothing).
- `git add` silently captures *their* half-finished edits to a file you also touched — the staged diff
  came back 66 lines when the change under review was 56.
- The index gets reset out from under a staged commit, so `git commit` reports "no changes added".
- A `git worktree` you created gets pruned away underneath you.

So do not stage in the shared index at all. **Do the entire release in a private worktree on a branch,
and push that branch to `main`.** The shared working tree is never touched, and nothing another agent
does can corrupt what you are about to tag.

```bash
git fetch origin
W=<scratchpad>/release
git worktree add -b release-vX.Y.Z "$W" origin/main
```

A `release-vX.Y.Z` branch that already exists is a prior attempt from another session's scratchpad
(`git worktree list`); `git branch -m` it aside and cut fresh.

Then bring your change in by *content*, file by file, checking each one as you go:

```bash
git -C <repo> diff -- <file>     # read it: is every hunk yours?
cp <repo>/<file> "$W"/<file>
```

A plain `cp` is only safe when the shared checkout's `HEAD` *is* `origin/main`
(`git rev-list --left-right --count HEAD...origin/main` prints `0 0`); otherwise a blind copy reverts
whatever origin merged since, so carry the change as a `--3way` patch instead. For a file where your
change is tangled with someone else's, extract only your hunks (`git diff -- <file> > all.patch`, keep
your `@@` blocks, `git apply` them onto the pristine copy) and re-read the result.

## 2. Green gate — the tag must point at code that compiles

In the worktree, with a **separate `CARGO_TARGET_DIR`** (sharing the main one with a concurrently
building session makes both of you thrash fingerprints and rebuild from scratch). `release.yml` runs
no tests, so this is the only gate: a fmt check and the whole suite with `--no-fail-fast` (plain
`cargo test` stops at the first failing binary and hides everything after it). Log it to a file rather
than piping through `tail`, or the per-suite `test result:` lines get cut off:

```bash
cd "$W" && cargo fmt --all -- --check && \
  CARGO_TARGET_DIR=<scratchpad>/vtarget cargo test --workspace --no-fail-fast > <scratchpad>/gate.log 2>&1
echo "exit=$?"; grep -E '^test result:|FAILED|panicked' <scratchpad>/gate.log
```

Do not release on a build you did not watch pass. "Those errors were all from the other session" is a
guess until a green run proves it.

**When a test fails, prove whose fault it is before you decide.** Check out `origin/main` in the same
worktree and run that same test: if it fails there too, it is pre-existing and not a release blocker —
say so in your report rather than silently ignoring it. Run that check in the *same* worktree (or in a
scratch worktree with its **own** `CARGO_TARGET_DIR`): two worktrees built into one target dir share the
`nebula` bin's unit hash, the base build overwrites `debug/nebula`, and every E2E run after it in the
release worktree exercises `origin/main`'s binary until you `cargo clean -p` the workspace crates (v0.18.0). Two known-environmental patterns in this repo:

- *Every* `e2e_tui`/`e2e_pty` test failing with "daemon did not come up … daemon.log: No such file or
  directory" is orphan-daemon starvation. Dozens of stale `target/debug/nebula daemon --foreground`
  processes accumulate over days and starve new test daemons. Check with
  `pgrep -f "target/debug/nebula daemon" | wc -l`.
- A single `e2e_tui` timeout waiting for footer text is usually a stale expectation in
  `crates/nebula/tests/e2e_tui.rs` (e.g. `FOOTER_TERMINAL_LOCKED = "Ctrl+q: sessions"` while the footer
  renders `^q: sessions`), not a regression.

Under zsh, quote shell separators (`echo '-----'`): a bare `=====` is `=cmd` expansion and sinks the
whole `&&` chain.

## 3. Commit the work — inside the worktree

Every `git add` / `git commit` from here on runs with `cd "$W"`, on the release branch. The worktree has
its own index, so nothing another agent does can reset it mid-commit.

One commit for the change, in the repo's voice: a subject line that says what a *user* now gets, not
what the diff did. Look at `git log --oneline -10` and match it — "Rebindable keys, a settings overlay,
and a status signal that survives cancel", not "feat(tui): add keymap module". Messages with backticks
go through `git commit -F <file>`; zsh command-substitutes them inside `-m "…"`.

End the message with:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Keep `.claude/` changes (skills, settings) in their own commit — they are not part of the release story
and clutter the changelog.

## 4. Bump the version

The version lives in `Cargo.toml` under `[workspace.package]`:

```toml
[workspace.package]
version = "0.3.0"
```

Every member crate inherits it. The `Release vX.Y.Z` commit also carries:

- **`Cargo.lock`** — it pins all four workspace members (`nebula`, `nebula-core`, `nebula-daemon`,
  `nebula-tui`) by version and CI builds `--locked`. `cargo check --workspace` rewrites it.
- **`docs/commands.md`** — the `nebula --version` example (`nebula 0.3.0`) near the top.
- **A config fixture** — `crates/nebula-core/fixtures/config-X.Y.Z.json`, added to `CONFIG_FIXTURES`
  in `crates/nebula-tui/src/config.rs`. It is the `config.json` this release writes with every value
  off its default, so `config_files_from_earlier_releases_still_load_every_key` holds every later
  build to it. Start from the previous release's fixture and add each key introduced since (diff the
  `pub struct Config` fields against it), set to a non-default value (v0.27.0 added
  `"ssh_sync_config": false`). `docs/configuration.md`'s compatibility rules promise one per release.
  `crates/nebula-daemon/src/config.rs` has its own single-fixture test for the daemon's keys, pinned
  to 0.26.0; move it only when a daemon key changes.

```bash
cargo check --workspace   # rewrites Cargo.lock
cargo test -p nebula-tui config_files_from_earlier_releases
git add Cargo.toml Cargo.lock docs/commands.md crates/nebula-core/fixtures crates/nebula-tui/src/config.rs
git commit -m "Release v0.3.0"
```

Pre-1.0 convention, from the existing tags: a new user-facing feature is a **minor** bump
(`0.2.0` → `0.3.0`); fixes and polish alone are a **patch** (`0.1.1` → `0.1.2`).

## 5. Push the branch *to* main, then tag

You are on a release branch, not on `main` — and you must not try to fast-forward the shared `main`,
because its working tree belongs to another agent. Push the branch onto the remote `main` instead, and
confirm the diff is only your work first:

```bash
git fetch origin
git diff --stat origin/main release-vX.Y.Z   # nothing here should be someone else's
git push origin release-vX.Y.Z:main
git tag vX.Y.Z <release commit>
git push origin vX.Y.Z
```

Push the branch before the tag. A tag whose commit isn't on the remote produces a release built from
nothing. `git push` goes over SSH and is unaffected by which `gh` account is active.

**Tell the user their local `main` is now behind `origin/main`.** You could not move it, so their next
`git pull` (or the other agent's push) has to reconcile. Keep the release branch around as a local
handle to the commits until they do.

## 6. Watch the build

```bash
gh run watch --exit-status $(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')
```

The matrix cross-compiles for macOS and Linux. If a target fails, the release is published without that
binary and `install.sh` silently falls back to building from source for those users — so a red matrix
is a real failure, not a cosmetic one. Fix forward and move the tag only if nothing has downloaded yet;
otherwise cut the next patch version.

## 7. Replace the release notes

The workflow publishes with `generate_release_notes: true`, which produces a bare commit list. That is
not a changelog. Overwrite it with release notes in the shape below — settled on 2026-08-28, when the
user picked this merge out of five variations of the v0.16.0 notes.

**Gather the input first.** The changelog spans everything since the *last tag*, not just the change you
carried in: `git log --oneline v<last>..vX.Y.Z`. v0.19.0 had three commits already on `origin/main`
above the previous tag, one of them user-facing, and writing from the dirty diff alone would have
shipped it silently. Read each commit's `--stat` to sort product changes from `.claude/`-only or
release-bump commits, which never make the notes.

- **A one-line opener.** `**Nebula vX.Y.Z is out.**` then the release in half a sentence, naming the
  headline items in the order the groups below use.
- **Benefit groups, not "Features" / "Fixes".** `###` headers, one emoji each, named for what the user
  gets. Reuse these when they fit and coin one when they do not: `🚀 Launch faster`,
  `🔔 Know when it's done`, `🧭 Lists that look after themselves`, `🫥 Shape the screen`,
  `🔌 Reach it from anywhere`. Two or three bullets per group; a group with one bullet merges into its
  neighbour.
- **Fixes file under the feature they belong to.** A fix is a bullet in the group whose promise it
  keeps — "No more early green" sits beside the done sound under *Know when it's done* — with the
  cause in one clause ("Claude Code 2.1 runs the Agent tool in the background, and its idle
  notification was read as 'turn over'") and the new behaviour in the next.
- **Tight bullets with a bold lead-in.** Each bullet opens with a bold two-to-five-word hook, then in
  two or three sentences, written for someone who has not read the diff: the key or command, the
  setting path in the `Settings › Sessions › done_sound` form, what happens, and where it lives
  (`agent_presets.json` beside `config.json`). Keys and identifiers in backticks; the emoji stays on
  the header, never on the bullet. Credit contributors inline: `Thanks @handle (#NN).`
- **`### ⚠️ Heads up`** for what the user must do to upgrade — a protocol version bump and
  `nebula kill` — as one line. Compare `PROTOCOL_VERSION` in `crates/nebula-core/src/protocol.rs` at
  the two tags (`git show v<last>:crates/nebula-core/src/protocol.rs | grep PROTOCOL_VERSION`); omit
  the group when nothing needs doing.
- **The install line last**, in a fenced block.

Every fact comes from the commits being released and their diffs; do not add a claim the code does not
make. The v0.16.0 notes, condensed to two groups, as the template:

````bash
gh release edit v0.16.0 --notes "$(cat <<'EOF'
**Nebula v0.16.0 is out.** Presets, a done sound, self-sorting lists, hideable panels, and a fix for sessions that went green too soon.

### 🚀 Launch faster
- **Agent presets** (`e` in the Sessions column). Save a name, harness, model, effort, and optional prefix/postfix once; `a` adds, `e` / `d` edit or delete. `Enter` takes the task in the wrapped editor and launches the CLI with *prefix + task + postfix* as its first prompt, so the agent is already working when the pane opens. Stored in `agent_presets.json` beside `config.json`.
- **Disable the CLIs you don't use.** Agents tab: `claude_enabled` / `codex_enabled` / `cursor_enabled` (at least one stays on). Disabled CLIs vanish from the new-session menu, PR launch and context menu.

### 🔔 Know when it's done
- **A done sound.** A ding when a turn finishes. Settings › Sessions › `done_sound`: a macOS system sound like Glass (default), `bell`, or `off`. Always the bell over `nebula ssh` and off macOS.
- **No more early green.** Claude Code 2.1 runs the Agent tool in the background, and its idle notification was read as "turn over". Sessions now stay RUNNING while subagents are tracked, with a 30-minute quiet grace so a killed worker can't wedge the row.

### ⚠️ Heads up
Protocol version is now **32** — run `nebula kill` on an older daemon before the new TUI attaches.

```
curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh
```
EOF
)"
````

Keep the `'EOF'` quoted so zsh does not command-substitute the backticks in the notes, or write the
notes to a file and pass `--notes-file` — the safer form when the notes run long. Show the user the
finished notes in your reply, not only on GitHub.

Writing to the API needs an account with write access to `AgentSystemLabs/nebula`. Check first:

```bash
gh auth status
```

Two accounts are usually logged in. `webdevcody` is the admin; `codyseibert` has read only and fails
with "must be a collaborator". If the wrong one is active:
`gh auth switch --hostname github.com --user webdevcody`.

The repo slug is **`AgentSystemLabs/nebula`** — never `webdevcody/nebula`.

## 8. Confirm and report

Check the release actually carries its binaries:

```bash
gh release view v0.3.0 --json assets -q '.assets[].name'
```

Then report to the user: the version, the tag URL, the asset list, any pre-existing test failures you
proved were not yours, and that their local `main` is behind `origin/main`. Remove the release worktree
(`git worktree remove "$W"`) once they have pulled.
