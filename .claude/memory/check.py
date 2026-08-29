#!/usr/bin/env python3
"""MEMORY CHECK — `make memory-check`, part of `make ci`.

Fails the build when the always-loaded layer of the MEMORY LOG outgrows what a session can afford to
read, or when the index and the entry files disagree. The caps are the rule; the NEBULA-MEMORY SKILL
prunes to them rather than asking agents to remember to.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
INDEX = os.path.join(ROOT, ".claude/MEMORY.md")
ARCHIVE = os.path.join(ROOT, ".claude/memory/archive.md")
GOTCHAS = os.path.join(ROOT, ".claude/memory/gotchas.md")
ENTRIES = os.path.join(ROOT, ".claude/memory/entries")
INDEX_CAP = 200      # lines — the index is read by every session
GOTCHAS_CAP = 300    # lines — so is the standing gotchas file
INDEX_ROW = re.compile(r"^- (\d{4}-\d{2}-\d{2}\S*) · \[(.+?)\]\((memory/entries/[^)]+\.md)\)")


def lines(path):
    return open(path, encoding="utf-8").read().count("\n") if os.path.exists(path) else 0


def main():
    errors = []
    n = lines(INDEX)
    if n > INDEX_CAP:
        errors.append(f".claude/MEMORY.md is {n} lines (cap {INDEX_CAP}): move the oldest index lines to .claude/memory/archive.md")
    g = lines(GOTCHAS)
    if g > GOTCHAS_CAP:
        errors.append(f".claude/memory/gotchas.md is {g} lines (cap {GOTCHAS_CAP}): retire gotchas a test or hook now enforces, merge duplicates")
    if not os.path.exists(GOTCHAS):
        errors.append(".claude/memory/gotchas.md is missing")

    indexed = set()
    for path in (INDEX, ARCHIVE):
        if not os.path.exists(path):
            continue
        for i, line in enumerate(open(path, encoding="utf-8"), 1):
            m = INDEX_ROW.match(line)
            if m:
                rel = m.group(3)
                if not os.path.exists(os.path.join(ROOT, ".claude", rel)):
                    errors.append(f"{os.path.relpath(path, ROOT)}:{i} links to missing {rel}")
                indexed.add(rel)
            elif line.startswith("- ") and "memory/entries/" in line:
                errors.append(f"{os.path.relpath(path, ROOT)}:{i} index line is malformed (expected '- DATE · [Title](memory/entries/….md) · TERMS: … · files: … · gotchas: N')")
    if os.path.exists(INDEX):
        for i, line in enumerate(open(INDEX, encoding="utf-8"), 1):
            if line.startswith("**Asked:**") or line.startswith("### "):
                errors.append(f".claude/MEMORY.md:{i} holds an entry body; entries live in .claude/memory/entries/")
                break
    if os.path.isdir(ENTRIES):
        for name in sorted(os.listdir(ENTRIES)):
            if name.endswith(".md") and f"memory/entries/{name}" not in indexed:
                errors.append(f".claude/memory/entries/{name} has no index line in .claude/MEMORY.md or archive.md")

    if errors:
        print("memory-check: FAIL")
        for e in errors:
            print("  - " + e)
        return 1
    print(f"memory-check: ok (index {n}/{INDEX_CAP} lines, gotchas {g}/{GOTCHAS_CAP} lines, {len(indexed)} entries indexed)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
