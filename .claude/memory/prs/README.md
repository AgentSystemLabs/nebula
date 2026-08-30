# Merged PR archive

One rendered Markdown file per pull request merged into `main`, written by
`.github/workflows/pr-archive.yml` (script: `.github/scripts/pr_archive.py`) the moment the PR
merges. It exists so an agent can read *why* a change looks the way it does — the review that pushed
back, the comment that explains the tradeoff — without a network call or a `gh` token.

| Path | What |
|---|---|
| `INDEX.md` | one line per archived PR, newest first — regenerated every run, never hand-edited |
| `<number>-<slug>.md` | the digest: metadata, description, changed files, commits, issue comments, review submissions, inline review comments with their diff hunks |
| `raw/<number>-<slug>.json` | the complete `gh pr view --json` payload plus the REST review comments, so nothing the digest omits is lost |

**This is not the MEMORY LOG.** The MEMORY LOG (`.claude/MEMORY.md` + `.claude/memory/entries/`) is
hand-written by the NEBULA-MEMORY SKILL, capped, indexed, and injected into every prompt by the
RECALL HOOK. This archive is machine-written, uncapped, and reached only by grep:

```
grep -ril '<symbol, TERM or symptom>' .claude/memory/prs
```

It deliberately sits *outside* `.claude/memory/entries/`, where MEMORY CHECK
(`.claude/memory/check.py`) would demand an index line in `.claude/MEMORY.md` for every file.

## Re-running it by hand

```
GH_TOKEN=$(gh auth token) python3 .github/scripts/pr_archive.py 42        # one PR
GH_TOKEN=$(gh auth token) python3 .github/scripts/pr_archive.py 1 2 3     # a backfill
```

Or run the **Archive Merged PR** workflow from the Actions tab with a space-separated list of
numbers. Re-running a PR overwrites its files (and drops the old ones if it was retitled), so a
backfill is safe to repeat.
