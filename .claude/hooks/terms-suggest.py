#!/usr/bin/env python3
"""TERMS SUGGEST — the `fileSuggestion` command behind Claude Code's `@` picker in this repo.

Claude Code runs it on every change of the text after `@`, with `{"query": "..."}` on stdin, and
shows the first 15 stdout lines as Tab-completable candidates. It answers with the TERMS from
`TERMS.md` that match the query (by name or by an alias from the Alias index) first, then repo
file paths — because setting `fileSuggestion` replaces the built-in file completion entirely.

An accepted TERM lands in the prompt as literal text (`@"WORKSPACES BAR"`), which the RECALL HOOK
and the model both read as the TERM's words; a file path is attached the way `@` always attaches
one. Wired up in `.claude/settings.json`; must answer within Claude Code's 5 s budget.
"""
import json
import os
import re
import subprocess
import sys

LIMIT = 15
TERM_ROW = re.compile(r"^\| \*\*([A-Z0-9][A-Z0-9 ./+'-]*?)\*\* \|(.*)$")
ALIAS_ROW = re.compile(r'^\| ("[^|]*") \| ([A-Z0-9][^|]*?) \|\s*$')


def load_terms(path):
    """Return (ordered TERM names, {term: [aliases]}); the Retired section is left out."""
    terms, aliases, retired, in_alias_index = [], {}, False, False
    try:
        lines = open(path, encoding="utf-8").read().split("\n")
    except OSError:
        return terms, aliases
    for line in lines:
        if line.startswith("## "):
            retired = "Retired" in line
            in_alias_index = line.startswith("## Alias index")
            continue
        if in_alias_index:
            m = ALIAS_ROW.match(line)
            if m:
                words = [w.strip().strip('"').lower() for w in m.group(1).split('", "')]
                for term in re.split(r"\s*/\s*", m.group(2).strip()):
                    aliases.setdefault(term, []).extend(w for w in words if w)
            continue
        m = TERM_ROW.match(line)
        if m and not retired:
            term = m.group(1).strip()
            if term not in terms:
                terms.append(term)
            cell = m.group(2).split("|")
            if len(cell) >= 3:
                for w in re.findall(r'"([^"]+)"', cell[1]):
                    aliases.setdefault(term, []).append(w.lower())
    return terms, aliases


def matching_terms(query, terms, aliases):
    q = query.lower().replace("_", " ").strip()
    if not q:
        return []
    prefix, inside, via_alias = [], [], []
    for term in terms:
        low = term.lower()
        if low.startswith(q):
            prefix.append(term)
        elif q in low:
            inside.append(term)
        elif any(q in a for a in aliases.get(term, ())):
            via_alias.append(term)
    return prefix + inside + via_alias


def repo_files(root):
    try:
        out = subprocess.run(
            ["git", "ls-files", "-co", "--exclude-standard"],
            cwd=root, capture_output=True, text=True, timeout=3, check=False,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    return [p for p in out.split("\n") if p]


def matching_files(query, files):
    q = query.lower()
    if not q:
        return files[:LIMIT]
    by_name = [p for p in files if q in os.path.basename(p).lower()]
    by_path = [p for p in files if q in p.lower() and p not in by_name]
    return by_name + by_path


def suggest(query, root):
    terms, aliases = load_terms(os.path.join(root, "TERMS.md"))
    hits = matching_terms(query, terms, aliases)
    if len(hits) < LIMIT:
        hits += matching_files(query, repo_files(root))
    return hits[:LIMIT]


def main():
    try:
        query = json.loads(sys.stdin.read() or "{}").get("query", "")
    except (ValueError, AttributeError):
        query = ""
    root = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    sys.stdout.write("".join(f"{s}\n" for s in suggest(str(query), root)))


if __name__ == "__main__":
    main()
