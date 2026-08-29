#!/usr/bin/env python3
"""Print the MEMORY LOG index line for an entry file — `python3 .claude/memory/index_line.py <entry.md>`.

The NEBULA-MEMORY SKILL uses it after writing `.claude/memory/entries/<date>-<slug>.md`: the TERMS cell is
the TERMS the entry names most (ALL-CAPS matches against TERMS.md, aliases as a fallback), the files cell
the file basenames it mentions most, the count its Gotchas bullets. Prepend the printed line under
`## Index` in `.claude/MEMORY.md`, then edit the TERMS cell if the top six are not the ones a future
prompt on this subject would use — that cell is what the RECALL HOOK matches on.
"""
import collections
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TERM_ROW = re.compile(r"^\| \*\*([A-Z0-9][A-Z0-9 ./+'-]*?)\*\* \|")
ALIAS_ROW = re.compile(r'^\| ("[^|]*") \| ([A-Z0-9][^|]*?) \|\s*$')
FILEISH = re.compile(r"\b[\w-]+\.(?:rs|toml|sh|py|json|mdc)\b")
HEAD = re.compile(r"^# (.+?) — (\d{4}-\d{2}-\d{2})((?: → \d{2}-\d{2})?)\s*$", re.M)


def load_terms():
    names, aliases, alias_index = [], {}, False
    for line in open(os.path.join(ROOT, "TERMS.md"), encoding="utf-8"):
        if line.startswith("## Alias index"):
            alias_index = True
        if alias_index:
            m = ALIAS_ROW.match(line)
            if m:
                for t in re.split(r"\s*/\s*", m.group(2).strip()):
                    for a in re.findall(r'"([^"]+)"', m.group(1)):
                        if len(a) >= 4:
                            aliases.setdefault(t.strip(), set()).add(a.lower())
            continue
        m = TERM_ROW.match(line)
        if m:
            names.append(m.group(1).strip())
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) > 2:
                for a in re.findall(r'"([^"]+)"', cells[2]):
                    if len(a) >= 4:
                        aliases.setdefault(names[-1], set()).add(a.lower())
    return sorted(set(names), key=len, reverse=True), aliases


def terms_in(body, names, aliases, limit=6):
    work, counts = body, collections.Counter()
    for t in names:
        pat = re.compile(r"(?<![A-Z])" + re.escape(t) + r"(?![A-Z])")
        n = len(pat.findall(work))
        if n:
            counts[t] = n
            work = pat.sub("\x00", work)
    out = [t for t, _ in counts.most_common(limit)]
    if len(out) < 3:
        low, extra = body.lower(), collections.Counter()
        for t, als in aliases.items():
            if t in out:
                continue
            for a in als:
                n = len(re.findall(r"(?<![\w-])" + re.escape(a) + r"(?![\w-])", low))
                if n:
                    extra[t] += n
        out += [t for t, _ in extra.most_common(limit - len(out))]
    return out


def gotcha_count(body):
    m = re.search(r"\*\*Gotchas:\*\*\s*\n(.*?)(?:\n\*\*[A-Z][a-z]+:\*\*|\Z)", body, re.S)
    return len(re.findall(r"^- ", m.group(1), re.M)) if m else 0


def index_line(path):
    body = open(path, encoding="utf-8").read()
    m = HEAD.search(body)
    if not m:
        sys.exit(f"{path}: first line must be '# <Title> — YYYY-MM-DD'")
    title, date, rng = m.group(1).strip(), m.group(2), m.group(3).replace(" → ", "→")
    names, aliases = load_terms()
    terms = terms_in(body, names, aliases)
    files = [f for f, _ in collections.Counter(FILEISH.findall(body)).most_common(4)]
    rel = os.path.relpath(os.path.abspath(path), os.path.join(ROOT, ".claude"))
    return f"- {date}{rng} · [{title}]({rel}) · TERMS: {'; '.join(terms)} · files: {'; '.join(files)} · gotchas: {gotcha_count(body)}"


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(index_line(p))
