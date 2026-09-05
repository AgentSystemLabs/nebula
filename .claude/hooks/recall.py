#!/usr/bin/env python3
"""RECALL HOOK — a Claude Code `UserPromptSubmit` hook.

Reads the prompt from stdin, maps its words onto TERMS (TERM names and the Alias index in
`TERMS.md`) and onto file / symbol names, scores every MEMORY LOG index line in `.claude/MEMORY.md`
and `.claude/memory/archive.md` (plus the words of each entry's own Asked / Did text), and prints the
Gotchas of the best few entries plus the best-matching lines of `.claude/memory/gotchas.md`. Printed
text becomes additional context for the turn. Prints nothing when nothing matches; any error exits 0
silently so a broken hook never blocks a prompt.

Registered in `.claude/settings.json`. Try it by hand:
    echo '{"prompt":"the stop gate turns green while subagents run"}' | python3 .claude/hooks/recall.py
    python3 .claude/hooks/recall.py "the stop gate turns green while subagents run"
    python3 .claude/hooks/recall.py --diff "<raw prompt>" "<refined prompt>"   # only what the refined prompt adds

`.claude/memory/recall_eval.py` (`make recall-eval`) scores `rank()` against every entry's own prompt
and a curated fixture; change the weights here only with that number in front of you.
"""
import json
import math
import os
import re
import sys

MAX_ENTRIES = 5          # entries injected per prompt
MAX_ENTRY_CHARS = 1400   # per entry
MAX_STANDING = 15        # lines from gotchas.md
MAX_PER_TERM = 4         # standing lines one TERM may take of those
MAX_TOTAL_CHARS = 8000
MIN_PROMPT_LEN = 12      # "yes", "do it", "the second one" carry no nouns
BODY_WEIGHT = 0.12       # weight of a prompt word found in an entry's own text, per unit of idf squared
MIN_ENTRY_SCORE = 2.0    # below this an entry is prose coincidence ("yes do it thanks" scores ~1.5)
MIN_STANDING_SCORE = 1.0 # a lone short alias shared with another TERM (0.5) does not earn a gotcha line
TERM_ROW = re.compile(r"^\| \*\*([A-Z0-9][^|*]*?)\*\* \|(.*)$")
ALIAS_ROW = re.compile(r'^\| ("[^|]*") \| ([A-Z0-9][^|]*?) \|\s*$')
INDEX_ROW = re.compile(r"^- (\d{4}-\d{2}-\d{2}\S*) · \[(.+?)\]\((.+?)\)(?: · TERMS: (.*?))?(?: · files: (.*?))?(?: · gotchas: \d+)?\s*$")
PATHISH = re.compile(r"\b(?:crates/)?[\w-]+(?:/[\w-]+)*\.(?:rs|md|toml|sh|py|json|mdc)\b|\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b|\b\w+::\w+\b")
WORD = re.compile(r"[a-z][a-z0-9]{2,}")
# Words of a prompt that say nothing about which entry it wants.
STOP = set("""the and for with that this those these from into onto over when then than they them its our your
you are was were will would should could can not but all any some also just like have has had does did doing
done make made get got use used using want need let see show only more most very each which what where why
how there here after before while about out off too now still again way thing things something anything
add remove change fix update please can't dont don't isn't its it's i'm i've we're
nice good great think thanks sounds looks ahead second explain changed everything work sure okay yes yeah right""".split())
# Aliases that are function words: they name a TERM only in one sentence, never in a prompt.
ALIAS_STOP = {"this", "that", "these", "those", "it", "its"}


def cells(row):
    return [c.strip() for c in row.strip().strip("|").split("|")]


def clean_term(name):
    """An Alias-index target may carry a note — `FINISHED (read)`, `NEW LINK (retired)`,
    `WORKTREE OPEN PRS GROUP — settle by …` — that is not part of the TERM."""
    return re.split(r"\s+\(|\s+—", name.strip())[0].strip()


def resolve_targets(cell, known):
    """The TERMS an Alias-index target cell names, longest name first at any position — so
    `FOCUS LEFT / FOCUS RIGHT` is the one row of that name, `MODEL / EFFORT` likewise, and an annotated
    target ("the PILL ROW's rail", "WALK EDGE — *not* LOCKED PANE") yields the TERMS written in it."""
    pat = re.compile(r"(?<![A-Z])(?:%s)(?![A-Z])" % "|".join(re.escape(n) for n in sorted(known, key=len, reverse=True)))
    head = re.split(r" — |\s\(|; |: ", cell, maxsplit=1)[0]      # the names come first; the note after them mentions others
    return list(dict.fromkeys(m.group(0) for m in pat.finditer(head))) or list(dict.fromkeys(m.group(0) for m in pat.finditer(cell)))


def load_terms(path):
    """TERM -> set of lowercase aliases (the TERM itself included). Only rows are TERMS; an Alias-index
    target that names no row is skipped (`make terms-check` reports it)."""
    terms = {}
    alias_rows = []
    alias_index = False
    for line in open(path, encoding="utf-8"):
        if line.startswith("## Alias index"):
            alias_index = True
        if not alias_index:
            m = TERM_ROW.match(line)
            if m:
                term = clean_term(m.group(1))
                parts = cells(line)
                also = parts[2] if len(parts) > 2 else ""
                terms.setdefault(term, {term.lower()})
                terms[term].update(a.strip().lower() for a in re.findall(r'"([^"]+)"', also))
        else:
            m = ALIAS_ROW.match(line)
            if m:
                alias_rows.append(([a.lower() for a in re.findall(r'"([^"]+)"', m.group(1))], m.group(2)))
    for aliases, cell in alias_rows:
        for term in resolve_targets(cell, terms):
            terms[term].update(aliases)
    for term, aliases in terms.items():
        aliases.difference_update(ALIAS_STOP)
    return terms


def contains(hay, needle):
    return re.search(r"(?<![\w-])" + re.escape(needle) + r"(?![\w-])", hay) is not None


def match_terms(prompt_lower, terms):
    """TERM -> weight of its best match in the prompt: 2 for the TERM's own name, else the alias's
    specificity (a multi-word or long alias 1, a short single word 0.5) split between every TERM that
    shares the alias ("done" names three TERMS; "green" six)."""
    share = {}
    for aliases in terms.values():
        for a in aliases:
            share[a] = share.get(a, 0) + 1
    hit = {}
    for term, aliases in terms.items():
        best = 0.0
        for a in aliases:
            if len(a) < 3 or not contains(prompt_lower, a):
                continue
            if a == term.lower():
                w = 2.0
            else:
                w = (1.0 if (" " in a or len(a) >= 8) else 0.5) / share.get(a, 1)
            best = max(best, w)
        if best:
            hit[term] = best
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


def words(text):
    return {w for w in WORD.findall(text.lower()) if w not in STOP}


def split_asked(body):
    """(asked, rest): the user's own prompt — the `**Asked:**` block up to `**Did:**` — and everything else."""
    m = re.search(r"\*\*Asked:\*\*(.*?)(?=\n\*\*Did:\*\*|\Z)", body, re.S)
    if not m:
        return "", body
    return m.group(1), body[:m.start()] + body[m.end():]


def load_bodies(rows):
    """Reads every entry once: body text plus its word sets, for the text-overlap score."""
    for row in rows:
        body = open(row["path"], encoding="utf-8").read() if os.path.exists(row["path"]) else ""
        asked, rest = split_asked(body)
        row["body"] = body
        row["asked_words"] = words(asked)
        row["rest_words"] = words(rest) | words(row["title"])


def idf_table(rows):
    df = {}
    for row in rows:
        for w in row["asked_words"] | row["rest_words"]:
            df[w] = df.get(w, 0) + 1
    n = len(rows)
    return {w: math.log((n + 1) / (c + 1)) for w, c in df.items()}


def overlap(prompt_words, doc_words, idf):
    """idf squared: one word unique to an entry (`zshrc`) outweighs three that half the log uses."""
    return sum(idf.get(w, 0.0) ** 2 for w in prompt_words & doc_words)


def rank(prompt, root, mask_asked=None):
    """Score the prompt against the MEMORY LOG. Returns (hit, paths, scored, standing):
    hit is TERM -> weight, paths the file/symbol names in the prompt, scored the index rows best first
    as (score, row), standing the matching gotchas.md lines best first as (score, group, line).
    `mask_asked` names one entry path whose own Asked text is ignored — the eval's leave-one-out."""
    low = prompt.lower()
    terms = load_terms(os.path.join(root, "TERMS.md"))
    hit = match_terms(low, terms)
    paths = {p for p in PATHISH.findall(prompt) if len(p) >= 5}
    pwords = words(prompt)

    rows = index_rows(root)
    load_bodies(rows)
    idf = idf_table(rows)
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
        score += 2 * sum(1 for p in paths if p in row["body"])
        text_words = row["rest_words"] if row["path"] == mask_asked else row["rest_words"] | row["asked_words"]
        score += BODY_WEIGHT * overlap(pwords, text_words, idf)
        if score >= MIN_ENTRY_SCORE:
            scored.append((score, row))
    scored.sort(key=lambda s: (s[0], s[1]["date"]), reverse=True)

    standing = []
    gpath = os.path.join(root, ".claude/memory/gotchas.md")
    if os.path.exists(gpath) and (hit or paths):
        hit_low = {t.lower(): w for t, w in hit.items()}
        group, per_term = "", {}
        candidates = []
        for i, line in enumerate(open(gpath, encoding="utf-8")):
            if line.startswith("## "):
                group = line[3:].strip()
                continue
            m = re.match(r"- \*\*(.+?)\*\* — ", line)
            if not m:
                continue
            term = m.group(1).strip().lower()
            score = 2.0 * hit_low.get(term, 0.0) + 2.0 * sum(1 for p in paths if p in line)
            if score <= 0:
                continue
            score += BODY_WEIGHT * overlap(pwords, words(line), idf)
            if score < MIN_STANDING_SCORE:
                continue
            candidates.append((score, -i, term, group, line[2:].rstrip()))
        candidates.sort(reverse=True)
        for score, _i, term, group, text in candidates:
            if per_term.get(term, 0) >= MAX_PER_TERM:
                continue
            per_term[term] = per_term.get(term, 0) + 1
            standing.append((score, group, text))
            if len(standing) >= MAX_STANDING:
                break
    return hit, paths, scored, standing


def render(root, hit, paths, scored, standing, skip_entries=(), skip_lines=()):
    """The `[nebula recall]` text for a `rank()` result; `skip_*` drops items already injected."""
    out = []
    label = ", ".join(sorted(hit)) or ", ".join(sorted(paths))
    keep = [(s, g, t) for s, g, t in standing if t not in skip_lines]
    if keep:
        out.append("[nebula recall] Standing gotchas for %s (.claude/memory/gotchas.md):\n%s"
                   % (label, "\n".join("- [%s] %s" % (g, t) for _s, g, t in keep)))
    entries = [(s, r) for s, r in scored[:MAX_ENTRIES] if r["path"] not in skip_entries]
    if entries:
        out.append("\n[nebula recall] MEMORY LOG entries matching this prompt (TERMS: %s). Open the file for the full Asked / Did / Gotchas."
                   % label)
        for _score, row in entries:
            rel = os.path.relpath(row["path"], root)
            g = gotchas_of(row["body"])
            if len(g) > MAX_ENTRY_CHARS:
                g = g[:MAX_ENTRY_CHARS].rsplit("\n", 1)[0] + "\n  …"
            out.append("\n### %s — %s (%s)\n%s" % (row["title"], row["date"], rel, g))
    return "\n".join(out).strip()


def usable(prompt):
    """Skip what carries no nouns of the user's: short replies, slash commands, and the harness's own
    `<task-notification>` / `<command-name>` / `<local-command-*>` records."""
    return len(prompt) >= MIN_PROMPT_LEN and not prompt.startswith(("/", "<"))


def main(argv):
    root = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    if argv and argv[0] == "--diff" and len(argv) >= 3:
        raw, refined = argv[1].strip(), argv[2].strip()
        _h, _p, r_scored, r_standing = rank(raw, root) if usable(raw) else ({}, set(), [], [])
        hit, paths, scored, standing = rank(refined, root)
        text = render(root, hit, paths, scored, standing,
                      skip_entries={r["path"] for _s, r in r_scored[:MAX_ENTRIES]},
                      skip_lines={t for _s, _g, t in r_standing})
    else:
        if argv:
            prompt = " ".join(argv)
        else:
            raw = sys.stdin.read()
            try:
                prompt = json.loads(raw).get("prompt", "")
            except Exception:
                prompt = raw
        prompt = (prompt or "").strip()
        if not usable(prompt):
            return
        hit, paths, scored, standing = rank(prompt, root)
        if not hit and not paths and not scored:
            return
        text = render(root, hit, paths, scored, standing)
    if text:
        sys.stdout.write(text[:MAX_TOTAL_CHARS] + "\n")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except Exception:
        if os.environ.get("RECALL_DEBUG"):
            raise
    sys.exit(0)
