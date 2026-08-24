---
name: release
description: "Cut a nebula release — verify the tree is green, commit, bump the workspace version, tag, push, and replace the auto-generated GitHub release notes with a real changelog. Use when the user says \"do a release\", \"cut a release\", \"release this\", \"ship it\", \"commit and push and release\", or asks for a new version. Also use when they ask what the release process is."
user-invocable: true
---

Nebula releases are tag-driven: pushing a `v*` tag makes `.github/workflows/release.yml` build the
cross-compile matrix and publish a GitHub release with the binaries attached. Everything before the tag
push is your job; everything after it is CI's, except the changelog, which CI gets wrong.

Work through the steps in order. Do not skip the green gate.

## 1. Preflight — is this tree even yours?

**Other agents edit this repo concurrently.** A `git status` from two minutes ago is not evidence about
now. Run it again immediately before you stage:

```bash
git status --short
```

Compare against the files you actually touched. Anything else — a crate you never opened, a
`.swp` file — belongs to somebody else's in-flight work.

- **Never `git add -A` or `git commit -a`.** Stage your paths by name, always.
- **Never stash** to get a clean tree. You will race the other session and lose their work.
- If their edits break the build (a half-renamed enum variant, a function that doesn't exist yet),
  that is *expected* and is not yours to fix. Go to step 2's worktree path.

Read `.claude/MEMORY.md` for anything recorded about the subsystem you're releasing.

## 2. Green gate — the tag must point at code that compiles

```bash
cargo test --workspace
```

If the shared tree is polluted by another session, that command tells you nothing about *your* commit.
Verify the staged snapshot in isolation instead:

```bash
git add <your paths>
W=<scratchpad>/verify
git worktree add --detach "$W" HEAD
git diff --cached --binary > "$W/staged.patch"
(cd "$W" && git apply staged.patch && rm staged.patch)
(cd "$W" && CARGO_TARGET_DIR=<scratchpad>/vtarget cargo test --workspace)
```

Use a **separate `CARGO_TARGET_DIR`**. Sharing the main one with a concurrently-building session makes
both of you thrash fingerprints and rebuild from scratch every time. Cold deps cost a few minutes; the
thrash costs more. Clean up with `git worktree remove --force "$W"` when done.

Do not release on a build you did not watch pass. "The errors were all from the other session" is a
guess until a green run proves it.

## 3. Commit the work

One commit for the change, in the repo's voice: a subject line that says what a *user* now gets, not
what the diff did. Look at `git log --oneline -10` and match it — "Rebindable keys, a settings overlay,
and a status signal that survives cancel", not "feat(tui): add keymap module".

End the message with:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Keep project-memory scaffolding (`.claude/`, `CLAUDE.md`) in its own commit — it is not part of the
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

## 5. Push, then tag

Push the branch first. A tag whose commit isn't on the remote produces a release built from nothing.

```bash
git push origin main
git tag v0.3.0
git push origin v0.3.0
```

`git push` goes over SSH and is unaffected by which `gh` account is active.

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
