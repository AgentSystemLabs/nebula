#!/usr/bin/env python3
"""CLAUDE TRANSCRIPT lookup — every past tool call whose input holds a needle, with what it printed.

Claude Code writes one `.jsonl` per SESSION under `~/.claude/projects/<checkout-slug>/`; a WORKTREE
session lives under its own slug (this checkout's slug plus the worktree path, so a prefix match finds
both). A `tool_use` block carries the call (`input.command` for Bash) and the next user turn carries its
`tool_result`. Together they answer "what did that session actually run, and what came back" — the
evidence for a truncated Bash call, a GUARD HOOK block, or a MEMORY LOG gotcha worth re-checking.

    python3 .claude/memory/transcript.py 'remote tag 22'              # this checkout and its worktrees
    python3 .claude/memory/transcript.py --regex 'for \\w+ in \\$\\('  # a regex over the call text
    python3 .claude/memory/transcript.py --slug -Users-me-proj 'x'    # another project's transcripts
    python3 .claude/memory/transcript.py --chars 0 'x'                # whole results (default 1200 chars)
"""
import argparse
import glob
import json
import os
import re
import sys

PROJECTS = os.path.expanduser("~/.claude/projects")


def call_text(block):
    inp = block.get("input") or {}
    return inp["command"] if isinstance(inp.get("command"), str) else json.dumps(inp, ensure_ascii=False)


def result_text(block):
    c = block.get("content")
    if isinstance(c, list):
        c = "\n".join(x.get("text", "") for x in c if isinstance(x, dict))
    return str(c or "")


def scan(path, matches):
    """Yield (timestamp, tool name, call text, result text) for every matching tool_use in one transcript."""
    uses = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            try:
                d = json.loads(line)
            except ValueError:
                continue
            content = (d.get("message") or {}).get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    text = call_text(block)
                    if matches(text):
                        uses[block.get("id")] = (d.get("timestamp"), block.get("name"), text)
                elif block.get("type") == "tool_result" and block.get("tool_use_id") in uses:
                    ts, name, text = uses.pop(block["tool_use_id"])
                    yield ts, name, text, result_text(block)
    for ts, name, text in uses.values():  # a call with no result yet, or one the transcript cut off
        yield ts, name, text, "(no tool_result recorded)"


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("needle", nargs="?", help="substring of the call text (the Bash command)")
    ap.add_argument("--regex", help="regex over the call text instead of a substring")
    ap.add_argument("--slug", help="project slug prefix (default: this cwd, `/` → `-`)")
    ap.add_argument("--session", help="only sessions whose id starts with this")
    ap.add_argument("--chars", type=int, default=1200, help="result chars to print, 0 = all")
    a = ap.parse_args(argv)
    if not a.needle and not a.regex:
        ap.error("a needle or --regex is required")
    pat = re.compile(a.regex) if a.regex else None
    matches = (lambda t: pat.search(t) is not None) if pat else (lambda t: a.needle in t)
    slug = a.slug or os.getcwd().replace("/", "-")
    dirs = sorted(d for d in glob.glob(os.path.join(PROJECTS, "*")) if os.path.basename(d).startswith(slug))
    if not dirs:
        print("no project dir under %s starts with %s" % (PROJECTS, slug))
        return 1
    hits = 0
    for d in dirs:
        for path in sorted(glob.glob(os.path.join(d, "*.jsonl"))):
            sid = os.path.basename(path)[:-6]
            if a.session and not sid.startswith(a.session):
                continue
            for ts, name, text, res in scan(path, matches):
                hits += 1
                print("=== %s  %s  %s  (%s)" % (sid[:8], ts, name, os.path.relpath(path, PROJECTS)))
                print(text)
                print("--- result%s:" % ("" if a.chars == 0 or len(res) <= a.chars else " (first %d of %d chars)" % (a.chars, len(res))))
                print(res if a.chars == 0 else res[: a.chars])
                print()
    print("%d hit(s) across %d project dir(s)" % (hits, len(dirs)))
    return 0 if hits else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
