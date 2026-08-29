# The ⌂ Root Row Named After A Worktree — 2026-08-25

**Asked:** "for some reason, in a terminal of my project is says I'm on main, but that root row in
worktrees list shows a worktree name and under it there is a main row for a worktree, but when I click it
and open a terminal, it points to a worktree called gentle-narwahl-files. can you double check the logic
around worktrees and the root row to determine why my root row isn't matching my actual branch, and why
somehow a worktree row is labeled as main" — then "yes fix them".

**Did:** Three daemon fixes; the TUI was never at fault (label and id both come from the same
`visible_worktrees()` entry, so a row can't be mislabeled). `add_project`
(`crates/nebula-daemon/src/registry.rs:558`) now roots the project at `git worktree list`'s **first**
entry instead of `rev-parse --show-toplevel`, and derives `is_main` as `entry.path == repo_path`.
`worktree_probe_stamp` (`crates/nebula-daemon/src/lib.rs:221`) goes through the new `git_common_dir`,
which follows a `gitdir:` file and its `commondir` hop. `reconcile_project_worktrees`
(`registry.rs:~1010`, the WORKTREE SYNC) re-derives root-ness every pass from `entries.first()` via the new
`Store::set_worktree_main`, and its delete pass dropped the `w.is_main ||` reprieve. 6 new tests
(4 in `registry::tests`, 2 in the new `nebula_daemon::probe_tests`); workspace suite 621 green.

**Gotchas:**
- **`git rev-parse --show-toplevel` inside a linked worktree returns the worktree, not the repo.** So
  `nebula add .` from a worktree made the *worktree* the project: named `gentle-narwahl-files`,
  `repo_path` pointing at it, and a ⌂ ROOT WORKTREE row for `…/repo` — a directory the project didn't own.
  `git worktree list --porcelain` always puts the main checkout first (verified: main first, then linked
  ones sorted by path), so it is the cheaper and more reliable root oracle, and it's already being called
  two lines later.
- **A probe that can't read anything is not a fingerprint.** `worktree_probe_stamp` did
  `repo_path.join(".git").join("HEAD")`; in a worktree `.git` is a *file*, so every stamp was `None`,
  `None == None`, and the project **never synced again after boot**. That alone is the "root row isn't
  matching my actual branch" half — confirmed live: root on `feature-x`, row still saying `main`
  indefinitely, while a normally-rooted control picked up its new branch within one 2s tick. The WORKTREE SYNC loop
  now refuses to cache a `None` stamp.
- **`is_main` was written once at insert and never updated** — no `UPDATE` of it existed anywhere. Nothing
  could repair a project seeded with the badge on the wrong row.
- **git will happily swap a root and a worktree's branches**, so this can also be *reality* faithfully
  reported, not a nebula bug: `git switch --ignore-other-worktrees <wt-branch>` in the root succeeds
  (plain `checkout` refuses), which frees `main` for the worktree to take. Check `git worktree list` before
  blaming the row.
- **The pre-fix breakage self-heals but only halfway.** Forged the old state in sqlite and restarted: the
  ⌂ ROOT BADGE moves back onto the repo's checkout and the branch goes live again, but `repo_path` and the
  project `name` still point at the worktree (deliberately not migrated — it would silently repoint a
  project, and could collide with the repo added separately in the same workspace). Remove + re-add is the
  remedy for those two fields.
- `crates/nebula-tui/*` in the SHARED CHECKOUT was already dirty with another session's in-flight work when this started; the
  whole change is confined to `nebula-daemon`. See [Shared Working Tree Is Raced By Other Sessions].
