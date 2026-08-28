---
name: release
description: "Cut a nebula release — verify the tree is green, commit, bump the workspace version, tag, push, and replace the auto-generated GitHub release notes with a real changelog. Use when the user says \"do a release\", \"cut a release\", \"release this\", \"ship it\", \"commit and push and release\", or asks for a new version. Also use when they ask what the release process is."
user-invocable: true
---

Nebula releases are tag-driven: pushing a `v*` tag makes `.github/workflows/release.yml` build the
cross-compile matrix and publish a GitHub release with the binaries attached. Everything before the tag
push is your job; everything after it is CI's, except the changelog, which CI gets wrong.

Work through the steps in order. Do not skip the green gate.

## 1. Preflight — assume you are not alone in this tree

**Other agents edit this repo concurrently, and they will fight you for the index.** All three of these
happened while cutting v0.3.0:

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

Then bring your change in by *content*, file by file, checking each one as you go:

```bash
git -C <repo> diff -- <file>     # read it: is every hunk yours?
cp <repo>/<file> "$W"/<file>
```

For a file where your change is tangled with someone else's, extract only your hunks
(`git diff -- <file> > all.patch`, keep your `@@` blocks, `git apply` them onto the pristine copy) and
re-read the result. Read `.claude/MEMORY.md` for anything recorded about the subsystem you're releasing.

## 2. Green gate — the tag must point at code that compiles

In the worktree, with a **separate `CARGO_TARGET_DIR`** (sharing the main one with a concurrently
building session makes both of you thrash fingerprints and rebuild from scratch):

```bash
(cd "$W" && CARGO_TARGET_DIR=<scratchpad>/vtarget cargo test --workspace)
```

Do not release on a build you did not watch pass. "Those errors were all from the other session" is a
guess until a green run proves it.

**When a test fails, prove whose fault it is before you decide.** Check out `origin/main` in the same
worktree and run that same test: if it fails there too, it is pre-existing and not a release blocker —
say so in your report rather than silently ignoring it. Two known-environmental patterns in this repo:

- *Every* `e2e_tui`/`e2e_pty` test failing with "daemon did not come up … daemon.log: No such file or
  directory" is orphan-daemon starvation. Dozens of stale `target/debug/nebula daemon --foreground`
  processes accumulate over days and starve new test daemons. Check with
  `pgrep -f "target/debug/nebula daemon" | wc -l`.
- A single `e2e_tui` timeout waiting for footer text is usually a stale expectation in
  `crates/nebula/tests/e2e_tui.rs` (e.g. `FOOTER_TERMINAL_LOCKED = "Ctrl+q: panels"` while the footer
  renders `^q: panels`), not a regression.

## 3. Commit the work — inside the worktree

Every `git add` / `git commit` from here on runs with `cd "$W"`, on the release branch. The worktree has
its own index, so nothing another agent does can reset it mid-commit.

One commit for the change, in the repo's voice: a subject line that says what a *user* now gets, not
what the diff did. Look at `git log --oneline -10` and match it — "Rebindable keys, a settings overlay,
and a status signal that survives cancel", not "feat(tui): add keymap module".

End the message with:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Keep project-memory scaffolding (`.claude/`, `CLAUDE.md`, `AGENTS.md`, `TERMS.md`) in its own commit — it is not part of the
release story and clutters the changelog.

## 4. Bump the version

One place, `Cargo.toml` under `[workspace.package]`:

```toml
[workspace.package]
version = "0.3.0"
```

Every member crate inherits it. Then refresh the lockfile — `Cargo.lock` pins all four workspace
members (`nebula`, `nebula-core`, `nebula-daemon`, `nebula-tui`) by version and CI builds `--locked`:

```bash
cargo check --workspace   # rewrites Cargo.lock
git add Cargo.toml Cargo.lock
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
not a changelog. Overwrite it:

```bash
gh release edit v0.3.0 --notes "$(cat <<'EOF'
## What's new

**Open the repo on its git host — `Shift+G`.** …one short paragraph per feature, written for someone
who has not read the diff: what the key does, where it shows up, what it does when it can't.

## Fixes

- …

**Full install:** `curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh`
EOF
)"
```

Writing to the API needs an account with write access to `AgentSystemLabs/nebula`. Check first:

```bash
gh auth status
```

Two accounts are usually logged in. `webdevcody` is the admin; `codyseibert` has read only and fails
with "must be a collaborator". If the wrong one is active:
`gh auth switch --hostname github.com --user webdevcody`.

The repo slug is **`AgentSystemLabs/nebula`** — never `webdevcody/nebula`.

## 8. Confirm and record

Check the release actually carries its binaries:

```bash
gh release view v0.3.0 --json assets -q '.assets[].name'
```

Then report to the user: the version, the tag URL, and the asset list. Finish by invoking the
`nebula-memory` skill to log the release and anything that bit you along the way.
