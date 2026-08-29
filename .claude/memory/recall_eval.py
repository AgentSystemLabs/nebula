#!/usr/bin/env python3
"""RECALL EVAL — `make recall-eval`, part of `make ci`.

Measures whether the RECALL HOOK (`.claude/hooks/recall.py`) finds the right MEMORY LOG entry:

- **self-recall** — every entry's own `**Asked:**` prompt (the user's words, verbatim) is run through
  `rank()` with that entry's Asked text masked out (so the text-overlap score cannot just find the
  quote); is the entry top-1 / top-5 / missed?
- **curated pairs** — `recall_eval.json`: prompts whose right answer is a *different* entry than the one
  written for them (a later "still broken", a release trigger, a question the log already answers), and
  chatter (`"expect": null`) that must inject nothing at all.

Compares the counts against `recall_baseline.json` and fails when top-5 self-recall or curated hits
dropped; `--update` rewrites the baseline after a deliberate change. `--verbose` lists the misses,
`--entry <file>` reports one entry's own rank (the NEBULA-MEMORY SKILL runs it on a fresh entry), and
`--hook <script>` scores an arbitrary hook script by its stdout instead (no masking — for comparing an
old copy). Weights in `recall.py` change only with this number in front of you.
"""
import fnmatch
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.dont_write_bytecode = True  # no __pycache__ beside the hook
sys.path.insert(0, os.path.join(ROOT, ".claude/hooks"))
import recall  # noqa: E402

ENTRIES = os.path.join(ROOT, ".claude/memory/entries")
FIXTURE = os.path.join(ROOT, ".claude/memory/recall_eval.json")
BASELINE = os.path.join(ROOT, ".claude/memory/recall_baseline.json")
ASKED = re.compile(r'\*\*Asked:\*\*\s*"(.*?)"\s*(?:\n→|\n\*\*|\Z)', re.S)
ENTRY_LINE = re.compile(r"^### .* \((\.claude/memory/entries/[^)]+\.md)\)", re.M)


def asked_prompt(path):
    """The verbatim user prompt of an entry, or None when the Asked line is a paraphrase."""
    body = open(path, encoding="utf-8").read()
    m = ASKED.search(body)
    if not m:
        return None
    prompt = " ".join(m.group(1).split())
    return prompt if recall.usable(prompt) else None


def top_paths(prompt, mask=None, hook=None):
    if hook:
        out = subprocess.run([sys.executable, hook], input=json.dumps({"prompt": prompt}), cwd=ROOT,
                             capture_output=True, text=True, env={**os.environ, "CLAUDE_PROJECT_DIR": ROOT}).stdout
        return [os.path.join(ROOT, p) for p in ENTRY_LINE.findall(out)]
    _hit, _paths, scored, _standing = recall.rank(prompt, ROOT, mask_asked=mask)
    return [row["path"] for _s, row in scored[:recall.MAX_ENTRIES]]


def position(paths, target):
    return paths.index(target) + 1 if target in paths else None


def main(argv):
    verbose = "--verbose" in argv
    update = "--update" in argv
    hook = argv[argv.index("--hook") + 1] if "--hook" in argv else None
    if "--entry" in argv:
        path = os.path.abspath(argv[argv.index("--entry") + 1])
        prompt = asked_prompt(path)
        if not prompt:
            print("no verbatim Asked prompt in", os.path.relpath(path, ROOT))
            return 0
        pos = position(top_paths(prompt, mask=path, hook=hook), path)
        print("%s: rank %s for its own prompt %r" % (os.path.relpath(path, ROOT), pos or "> %d" % recall.MAX_ENTRIES, prompt[:80]))
        return 0

    top1 = top5 = 0
    missed = []
    entries = [os.path.join(ENTRIES, n) for n in sorted(os.listdir(ENTRIES)) if n.endswith(".md")]
    prompts = [(p, asked_prompt(p)) for p in entries]
    prompts = [(p, q) for p, q in prompts if q]
    for path, prompt in prompts:
        pos = position(top_paths(prompt, mask=path, hook=hook), path)
        if pos == 1:
            top1 += 1
        if pos:
            top5 += 1
        else:
            missed.append((path, prompt))

    curated = json.load(open(FIXTURE, encoding="utf-8")) if os.path.exists(FIXTURE) else []
    curated_hits, curated_missed = 0, []
    for case in curated:
        paths = [os.path.relpath(p, ROOT) for p in top_paths(case["prompt"], hook=hook)]
        if case["expect"] is None:
            if not paths:
                curated_hits += 1
            else:
                curated_missed.append((case, paths))
            continue
        if any(fnmatch.fnmatch(p, ".claude/memory/entries/" + case["expect"]) for p in paths):
            curated_hits += 1
        else:
            curated_missed.append((case, paths))

    n = len(prompts)
    print("self-recall: %d entries with a verbatim prompt — top-1 %d (%.0f%%), top-5 %d (%.0f%%), missed %d"
          % (n, top1, 100.0 * top1 / max(n, 1), top5, 100.0 * top5 / max(n, 1), n - top5))
    print("curated:     %d prompts — top-5 %d" % (len(curated), curated_hits))
    if verbose:
        for path, prompt in missed:
            print("  miss  %s\n        %r" % (os.path.relpath(path, ROOT), prompt[:110]))
        for case, paths in curated_missed:
            print("  miss  %r → expected %s, got %s" % (case["prompt"][:80], case["expect"], [os.path.basename(p) for p in paths]))

    result = {"self_prompts": n, "self_top1": top1, "self_top5": top5, "curated": len(curated), "curated_top5": curated_hits}
    if hook:
        return 0
    if update or not os.path.exists(BASELINE):
        json.dump(result, open(BASELINE, "w", encoding="utf-8"), indent=2)
        open(BASELINE, "a").write("\n")
        print("baseline written to", os.path.relpath(BASELINE, ROOT))
        return 0
    base = json.load(open(BASELINE, encoding="utf-8"))
    errors = []
    if top5 < base.get("self_top5", 0):
        errors.append("self-recall top-5 fell %d → %d" % (base["self_top5"], top5))
    if curated_hits < base.get("curated_top5", 0):
        errors.append("curated top-5 fell %d → %d" % (base["curated_top5"], curated_hits))
    if errors:
        print("recall-eval: FAIL — " + "; ".join(errors))
        print("  a regression in recall.py, TERMS.md aliases or an index line's TERMS cell; rerun with --verbose,"
              " or `python3 .claude/memory/recall_eval.py --update` after a deliberate change")
        return 1
    if top1 < base.get("self_top1", 0):
        print("note: top-1 fell %d → %d (not gated; --update to accept)" % (base["self_top1"], top1))
    print("recall-eval: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
