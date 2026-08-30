# A GitHub Action Archives Every Merged PR Into `.claude/memory/prs/` For Agent Context — 2026-08-30

**Asked:** "add a github action where after a pull request is merged, pull all data related to the pr
including comments and put in the repo somewhere for context."
→ refined: "Add a GitHub Actions workflow that fires when a pull request is merged into `main` and
archives it into the repo as agent context: PR metadata, description, every issue comment, every
review, every inline review comment with file and line, commit subjects and changed files — pulled
with `gh pr view --json` plus `gh api` for the inline comments no `--json` field exposes. Write one
Markdown file per PR to `.claude/memory/prs/<number>-<slug>.md`, a sibling of the MEMORY LOG entries
so `make memory-check`'s index ↔ entries agreement is untouched, and commit it back to `main` as a bot
commit. Leave the RELEASE WORKFLOW and the Claude Code Review workflow alone." (no questions asked)

**Did:** New `.github/workflows/pr-archive.yml` + `.github/scripts/pr_archive.py`. The script renders
`<n>-<slug>.md` (metadata, description, changed files, commits, issue comments, review submissions,
inline comments with their diff hunks), dumps the whole payload to `raw/<n>-<slug>.json`, and
regenerates `INDEX.md` from the directory. The workflow runs on `pull_request_target: [closed]` with
`if: merged == true`, checks out `main`, and pushes a bot commit with a 5-attempt rebase retry
(`origin/main` moves on its own here). Backfilled all 11 merged PRs (200 KB). Added the PR ARCHIVE row
to the AGENTS.md memory table and `.claude/memory/prs/README.md`. Verified: rendering against
`cli/cli#14289` (3 reviews, 2 inline comments with hunks and a reply), two-page pagination via
`?per_page=1`, and `make memory-check` / `recall-eval` / `terms-check` all ok.

**Gotchas:**
- **A `pull_request` run for a PR from a fork gets a read-only `GITHUB_TOKEN` and no secrets**, so the
  push would 403 on exactly the contributor PRs this exists to archive (PR #20 was external).
  `pull_request_target` runs in the base repo's context with a write token and reads its own definition
  from `main`. It is only the dangerous trigger when it checks out and executes the PR's code — this one
  pins `ref: main` and runs main's script.
- **The repo's default workflow permission is `read`** (`gh api
  repos/AgentSystemLabs/nebula/actions/permissions/workflow` → `default_workflow_permissions: "read"`),
  but an explicit job-level `permissions: contents: write` overrides the default. No repo setting has to
  be flipped — the read-only default only applies when a workflow declares nothing.
- **`gh api --paginate` without `--jq` concatenates one JSON array per page** (`[…][…]`), which
  `json.loads` rejects the moment a PR has more than 30 inline comments. `--jq '.[]'` flattens to one
  compact object per line on every gh version — proved with `?per_page=1` over a 2-comment PR.
- **Re-hit: inline per-line review comments are not a `gh pr view --json` field.** They only come from
  `repos/{owner}/{repo}/pulls/{n}/comments`, whose fields are `path`, `line`, `original_line`, `user.login`,
  `created_at`, `in_reply_to_id`, `diff_hunk` — none of them camelCase like the GraphQL side.
- **zsh does not word-split an unquoted `$VAR`**: `python3 pr_archive.py $NUMS` passed "22 21 20 …" as one
  argv and died in `int()`. bash on the runner *does* split, so CI would have been fine and the local
  backfill was not — the script now splits every argv on whitespace, which fixes both.
- **The archive deliberately sits outside `.claude/memory/entries/`**: MEMORY CHECK
  (`.claude/memory/check.py`) errors on any `.md` there without an index line in `.claude/MEMORY.md`, so
  a machine-written corpus in that directory would fail `make ci` on every merge.
