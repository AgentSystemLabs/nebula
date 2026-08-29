#!/usr/bin/env python3
"""RECALL HOOK — a Claude Code `UserPromptSubmit` hook.

Reads the prompt from stdin, maps its words onto TERMS (TERM names and the Alias index in
`TERMS.md`) and onto file / symbol names, scores every MEMORY LOG index line in `.claude/MEMORY.md`
and `.claude/memory/archive.md`, and prints the Gotchas of the best few entries plus the matching lines
of `.claude/memory/gotchas.md`. Printed text becomes additional context for the turn. Prints nothing
when nothing matches; any error exits 0 silently so a broken hook never blocks a prompt.

Registered in `.claude/settings.json`. Try it by hand:
    echo '{"prompt":"the stop gate turns green while subagents run"}' | python3 .claude/hooks/recall.py
"""
import json
import math
import os
import re
import sys

MAX_ENTRIES = 5          # entries injected per prompt
MAX_ENTRY_CHARS = 1400   # per entry
MAX_STANDING = 15        # lines from gotchas.md
MAX_TOTAL_CHARS = 8000
MIN_PROMPT_LEN = 12      # "yes", "do it", "the second one" carry no nouns
TERM_ROW = re.compile(r"^\| \*\*([A-Z0-9][A-Z0-9 ./+'-]*?)\*\* \|(.*)$")
ALIAS_ROW = re.compile(r'^\| ("[^|]*") \| ([A-Z0-9][^|]*?) \|\s*$')
INDEX_ROW = re.compile(r"^- (\d{4}-\d{2}-\d{2}\S*) · \[(.+?)\]\((.+?)\)(?: · TERMS: (.*?))?(?: · files: (.*?))?(?: · gotchas: \d+)?\s*$")
PATHISH = re.compile(r"\b(?:crates/)?[\w-]+(?:/[\w-]+)*\.(?:rs|md|toml|sh|py|json|mdc)\b|\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b|\b\w+::\w+\b")


def cells(row):
    return [c.strip() for c in row.strip().strip("|").split("|")]


def load_terms(path):
    """TERM -> set of lowercase aliases (the TERM itself included)."""
    terms = {}
    alias_index = False
    for line in open(path, encoding="utf-8"):
        if line.startswith("## Alias index"):
            alias_index = True
        if not alias_index:
            m = TERM_ROW.match(line)
            if m:
                term = m.group(1).strip()
                parts = cells(line)
                also = parts[2] if len(parts) > 2 else ""
                terms.setdefault(term, {term.lower()})
                terms[term].update(a.strip().lower() for a in re.findall(r'"([^"]+)"', also))
        else:
            m = ALIAS_ROW.match(line)
            if m:
                aliases = [a.lower() for a in re.findall(r'"([^"]+)"', m.group(1))]
                for term in re.split(r"\s*/\s*", m.group(2).strip()):
                    term = term.strip()
                    if term:
                        terms.setdefault(term, {term.lower()}).update(aliases)
    return terms


def contains(hay, needle):
    return re.search(r"(?<![\w-])" + re.escape(needle) + r"(?![\w-])", hay) is not None


def match_terms(prompt_lower, terms):
    hit = {}
    for term, aliases in terms.items():
        for a in aliases:
            if len(a) >= 3 and contains(prompt_lower, a):
                hit[term] = max(hit.get(term, 0), 2 if a == term.lower() else 1)
                break
    return hit


def index_rows(root):
    rows = []
    for name in (".claude/MEMORY.md", ".claude/memory/archive.md"):
        p = os.path.join(root, name)
        if not os.path.exists(p):
            continue
        for line in open(p, encoding="utf-8"):
            m = INDEX_ROW.match(line)
            if m:
                date, title, rel, tlist, flist = m.groups()
                rows.append({
                    "date": date, "title": title,
                    "path": os.path.normpath(os.path.join(root, ".claude", rel)),
                    "terms": [t.strip() for t in (tlist or "").split(";") if t.strip()],
                    "files": [f.strip() for f in (flist or "").split(";") if f.strip()],
                })
    return rows


def gotchas_of(body):
    m = re.search(r"\*\*Gotchas:\*\*\s*\n(.*)", body, re.S)
    text = m.group(1) if m else ""
    if not text.strip():
        m = re.search(r"\*\*Did:\*\*\s*(.*)", body, re.S)
        text = (m.group(1) if m else body)[:600]
    return text.strip()


def main():
    root = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    raw = sys.stdin.read()
    try:
        prompt = json.loads(raw).get("prompt", "")
    except Exception:
        prompt = raw
    prompt = (prompt or "").strip()
    if len(prompt) < MIN_PROMPT_LEN or prompt.startswith("/"):
        return
    low = prompt.lower()
    terms = load_terms(os.path.join(root, "TERMS.md"))
    hit = match_terms(low, terms)
    paths = {p for p in PATHISH.findall(prompt) if len(p) >= 5}
    if not hit and not paths:
        return

    rows = index_rows(root)
    # A TERM on half the index lines (SESSION, TUI) says little; weight each hit by how rare it is.
    df = {}
    for row in rows:
        for t in set(row["terms"]):
            df[t] = df.get(t, 0) + 1
    scored = []
    for row in rows:
        score = sum(hit.get(t, 0) * 2.0 / math.log(2 + df.get(t, 1)) for t in row["terms"])
        title_low = row["title"].lower()
        score += sum(1 for t in hit if contains(title_low, t.lower()))
        score += 2 * sum(1 for f in row["files"] if any(f in p or p in f for p in paths))
        body = ""
        if paths and os.path.exists(row["path"]):
            body = open(row["path"], encoding="utf-8").read()
            score += 2 * sum(1 for p in paths if p in body)
        if score > 0:
            scored.append((score, row["date"], row, body))
    scored.sort(key=lambda s: (s[0], s[1]), reverse=True)

    out = []
    # Standing gotchas first — the curated layer — matched on each line's own **TERM** or a file name.
    gpath = os.path.join(root, ".claude/memory/gotchas.md")
    if os.path.exists(gpath):
        keep, group = [], ""
        hit_low = {t.lower() for t in hit}
        for line in open(gpath, encoding="utf-8"):
            if line.startswith("## "):
                group = line[3:].strip()
                continue
            m = re.match(r"- \*\*(.+?)\*\* — ", line)
            if not m:
                continue
            term = m.group(1).strip().lower()
            if term in hit_low or any(p in line for p in paths):
                keep.append("- [%s] %s" % (group, line[2:].rstrip()))
                if len(keep) >= MAX_STANDING:
                    break
        if keep:
            out.append("[nebula recall] Standing gotchas for %s (.claude/memory/gotchas.md):\n%s"
                       % (", ".join(sorted(hit)) or ", ".join(sorted(paths)), "\n".join(keep)))
    if scored:
        out.append("\n[nebula recall] MEMORY LOG entries matching this prompt (TERMS: %s). Open the file for the full Asked / Did / Gotchas."
                   % (", ".join(sorted(hit)) or ", ".join(sorted(paths))))
        for score, _date, row, body in scored[:MAX_ENTRIES]:
            if not body and os.path.exists(row["path"]):
                body = open(row["path"], encoding="utf-8").read()
            rel = os.path.relpath(row["path"], root)
            g = gotchas_of(body)
            if len(g) > MAX_ENTRY_CHARS:
                g = g[:MAX_ENTRY_CHARS].rsplit("\n", 1)[0] + "\n  …"
            out.append("\n### %s — %s (%s)\n%s" % (row["title"], row["date"], rel, g))

    text = "\n".join(out).strip()
    if text:
        sys.stdout.write(text[:MAX_TOTAL_CHARS] + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        pass
    sys.exit(0)
