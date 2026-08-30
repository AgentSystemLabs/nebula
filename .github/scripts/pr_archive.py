#!/usr/bin/env python3
"""PR ARCHIVE — renders one merged pull request into `.claude/memory/prs/` as agent context.

Run by `.github/workflows/pr-archive.yml` after a PR merges into `main`, and by hand for a backfill:

    GH_TOKEN=$(gh auth token) python3 .github/scripts/pr_archive.py 42
    GH_TOKEN=$(gh auth token) python3 .github/scripts/pr_archive.py 1 2 3   # several

Writes, per PR:
  .claude/memory/prs/<number>-<slug>.md       the rendered digest an agent reads
  .claude/memory/prs/raw/<number>-<slug>.json the complete `gh` payload, so nothing is lost
  .claude/memory/prs/INDEX.md                 regenerated from the directory, newest PR first

Deliberately outside `.claude/memory/entries/`: MEMORY CHECK (`.claude/memory/check.py`) demands an
index line in `.claude/MEMORY.md` for every file there, and the MEMORY LOG is hand-written by the
NEBULA-MEMORY SKILL. This corpus is machine-written and grepped on demand instead.
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, ".claude/memory/prs")
RAW = os.path.join(OUT, "raw")

# Every --json field `gh pr view` exposes that carries context. Inline per-line review comments are
# not among them (see the PR ROW standing gotcha) — they come from the REST API below.
FIELDS = (
    "number,title,url,state,isDraft,body,author,assignees,labels,milestone,"
    "createdAt,updatedAt,closedAt,mergedAt,mergedBy,mergeCommit,"
    "baseRefName,headRefName,headRefOid,additions,deletions,changedFiles,files,"
    "commits,comments,reviews,reviewRequests,closingIssuesReferences"
)


def gh(*args):
    r = subprocess.run(["gh", *args], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"gh {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout


def gh_paginate(path):
    """Every page of a REST list endpoint.

    `gh api --paginate` without `--jq` concatenates one JSON array per page (`[…][…]`), which
    `json.loads` rejects the moment a PR has more than 30 inline comments. `--jq '.[]'` flattens it
    to one compact object per line on every gh version.
    """
    out = gh("api", "--paginate", path, "--jq", ".[]")
    return [json.loads(line) for line in out.splitlines() if line.strip()]


def slug(title, limit=60):
    s = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    return (s[:limit].rstrip("-") or "untitled")


def login(obj):
    if not obj:
        return "unknown"
    name = obj.get("login") or obj.get("name") or "unknown"
    return f"@{name}" if not name.startswith("@") else name


def quote(text):
    """Body text, indented as a blockquote so a PR's own headings can't outrank the file's."""
    text = (text or "").replace("\r\n", "\n").strip()
    if not text:
        return "_(empty)_"
    return "\n".join("> " + line if line else ">" for line in text.split("\n"))


def render(pr, inline):
    n, out = pr["number"], []
    add = out.append
    add(f"# PR #{n} — {pr['title']}\n")
    add(f"- **URL:** {pr['url']}")
    add(f"- **Author:** {login(pr.get('author'))}")
    merged = pr.get("mergedAt")
    if merged:
        sha = (pr.get("mergeCommit") or {}).get("oid", "")
        add(f"- **Merged:** {merged} by {login(pr.get('mergedBy'))}" + (f" (`{sha[:12]}`)" if sha else ""))
    else:
        add(f"- **State:** {pr.get('state')} (closed {pr.get('closedAt')})")
    add(f"- **Opened:** {pr.get('createdAt')}")
    add(f"- **Branch:** `{pr.get('headRefName')}` → `{pr.get('baseRefName')}`")
    labels = ", ".join(l["name"] for l in pr.get("labels") or [])
    if labels:
        add(f"- **Labels:** {labels}")
    if pr.get("assignees"):
        add(f"- **Assignees:** {', '.join(login(a) for a in pr['assignees'])}")
    if pr.get("milestone"):
        add(f"- **Milestone:** {pr['milestone'].get('title')}")
    if pr.get("closingIssuesReferences"):
        add("- **Closes:** " + ", ".join(f"#{i['number']} {i.get('title','')}".strip()
                                          for i in pr["closingIssuesReferences"]))
    add(f"- **Diff:** +{pr.get('additions', 0)} −{pr.get('deletions', 0)} across "
        f"{pr.get('changedFiles', 0)} file(s)")
    add("")

    add("## Description\n")
    add(quote(pr.get("body")))
    add("")

    files = pr.get("files") or []
    if files:
        add(f"## Changed files ({len(files)})\n")
        for f in files:
            add(f"- `{f['path']}` +{f.get('additions', 0)} −{f.get('deletions', 0)}")
        add("")

    commits = pr.get("commits") or []
    if commits:
        add(f"## Commits ({len(commits)})\n")
        for c in commits:
            subject = (c.get("messageHeadline") or "").strip()
            authors = ", ".join(login(a.get("user") or a) for a in c.get("authors") or []) or "unknown"
            add(f"- `{c.get('oid','')[:12]}` {subject} — {authors}")
        add("")

    comments = pr.get("comments") or []
    add(f"## Conversation ({len(comments)})\n")
    if not comments:
        add("_(no issue comments)_\n")
    for c in comments:
        add(f"### {login(c.get('author'))} · {c.get('createdAt')}\n")
        add(quote(c.get("body")))
        add("")

    reviews = pr.get("reviews") or []
    add(f"## Reviews ({len(reviews)})\n")
    if not reviews:
        add("_(no review submissions)_\n")
    for r in reviews:
        add(f"### {login(r.get('author'))} · {r.get('state')} · {r.get('submittedAt')}\n")
        add(quote(r.get("body")))
        add("")

    add(f"## Inline review comments ({len(inline)})\n")
    if not inline:
        add("_(no inline comments)_\n")
    for c in inline:
        line = c.get("line") or c.get("original_line") or "?"
        head = f"### `{c.get('path')}:{line}` — {login(c.get('user'))} · {c.get('created_at')}"
        if c.get("in_reply_to_id"):
            head += " · reply"
        add(head + "\n")
        hunk = (c.get("diff_hunk") or "").strip()
        if hunk:
            add("```diff\n" + hunk + "\n```\n")
        add(quote(c.get("body")))
        add("")

    return "\n".join(out).rstrip() + "\n"


def write_index():
    rows = []
    for name in os.listdir(OUT):
        if not name.endswith(".md") or name in ("INDEX.md", "README.md"):
            continue
        m = re.match(r"^(\d+)-", name)
        if not m:
            continue
        title, merged = name, ""
        with open(os.path.join(OUT, name), encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("# PR #"):
                    title = line.split("—", 1)[-1].strip()
                elif line.startswith("- **Merged:**"):
                    merged = line.split("**Merged:**", 1)[1].split("by")[0].strip()
                    break
                elif line.startswith("## "):
                    break
        rows.append((int(m.group(1)), name, title, merged))
    rows.sort(reverse=True)
    body = [
        "# Merged PRs — index",
        "",
        "One line per archived pull request, newest first. Regenerated by "
        "`.github/scripts/pr_archive.py`; do not hand-edit.",
        "",
    ]
    body += [f"- [#{num} — {title}]({name}){f' · merged {merged}' if merged else ''}" for num, name, title, merged in rows]
    with open(os.path.join(OUT, "INDEX.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(body) + "\n")
    return len(rows)


def archive(number):
    pr = json.loads(gh("pr", "view", str(number), "--json", FIELDS))
    # GH_REPO is what the workflow sets; outside CI, gh resolves the repo from the git remote.
    repo = os.environ.get("GH_REPO") or json.loads(gh("repo", "view", "--json", "nameWithOwner"))["nameWithOwner"]
    inline = gh_paginate(f"repos/{repo}/pulls/{number}/comments")

    base = f"{pr['number']}-{slug(pr['title'])}"
    os.makedirs(RAW, exist_ok=True)
    md = os.path.join(OUT, base + ".md")

    # A retitled PR would otherwise leave its old file behind.
    for stale in os.listdir(OUT):
        if re.match(rf"^{pr['number']}-.*\.md$", stale) and stale != base + ".md":
            os.remove(os.path.join(OUT, stale))
    for stale in os.listdir(RAW):
        if re.match(rf"^{pr['number']}-.*\.json$", stale) and stale != base + ".json":
            os.remove(os.path.join(RAW, stale))

    with open(md, "w", encoding="utf-8") as fh:
        fh.write(render(pr, inline))
    with open(os.path.join(RAW, base + ".json"), "w", encoding="utf-8") as fh:
        json.dump({"pr": pr, "review_comments": inline}, fh, indent=2, sort_keys=True)
        fh.write("\n")
    print(f"pr_archive: wrote {os.path.relpath(md, ROOT)} "
          f"({len(pr.get('comments') or [])} comments, {len(pr.get('reviews') or [])} reviews, "
          f"{len(inline)} inline)")


def main(argv):
    # Each argument may itself be a whitespace-separated list: zsh does not word-split an unquoted
    # "$NUMS", and the workflow_dispatch input arrives as one string.
    numbers = [n for arg in argv for n in arg.split()]
    if not numbers:
        raise SystemExit("usage: pr_archive.py <pr-number> [<pr-number> …]")
    os.makedirs(OUT, exist_ok=True)
    for number in numbers:
        archive(int(number))
    print(f"pr_archive: INDEX.md lists {write_index()} PR(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
