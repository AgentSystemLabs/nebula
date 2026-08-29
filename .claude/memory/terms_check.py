#!/usr/bin/env python3
"""TERMS CHECK — `make terms-check` (part of `make ci`) and the PROJECT TERMS skill's pruning tool.

Report (default): what in `TERMS.md` is dead weight or broken —
  dead        TERMS never written in caps anywhere outside TERMS.md (entries, gotchas, index, CLAUDE.md,
              README, skills, `//` comments), and TERMS said exactly once
  merge?      TERMS only ever said on a line that also says another TERM (the skill's merge rule)
  collide     aliases shared by 3+ TERMS (they carry no signal for the RECALL HOOK)
  stale       `Where` cells whose file or symbol no longer greps            [CI fails]
  dangling    Alias-index targets that are not TERMS, duplicate TERM rows   [CI fails]
  overdue     Candidates whose newest sighting is older than 30 days        (`--prune` deletes)
  unmentioned Retired rows nothing under .claude/memory/ mentions           (`--prune` deletes)

Actions (each rewrites TERMS.md, the Alias index, the MEMORY LOG index cells and gotchas.md keys, then
runs `recall_eval.py` and fails if retrieval regressed — `git checkout TERMS.md .claude/MEMORY.md
.claude/memory/gotchas.md` undoes it):
  --merge A --into B          A's name and aliases become aliases of B; A's row moves to Retired ("merged
                              into B"); every A in the index cells, gotcha keys, Alias index and other rows'
                              cross-references becomes B. A's meaning is printed for you to fold into B's
                              row by hand if B does not already say it.
  --retire A [--into B]       A's thing is gone: row to Retired; references become B if given, else drop.
  --prune                     delete overdue Candidates and unmentioned Retired rows.
  --dry-run                   with any action: print the plan, write nothing.
  --note TEXT                 with --retire/--merge: appended to the Retired row.
"""
import datetime
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, ".claude/hooks"))
import recall  # noqa: E402  (resolve_targets — one reading of the Alias index for every tool)
TERMS = os.path.join(ROOT, "TERMS.md")
INDEX = os.path.join(ROOT, ".claude/MEMORY.md")
ARCHIVE = os.path.join(ROOT, ".claude/memory/archive.md")
GOTCHAS = os.path.join(ROOT, ".claude/memory/gotchas.md")
ENTRIES = os.path.join(ROOT, ".claude/memory/entries")
CANDIDATE_DAYS = 30
ROW = re.compile(r"^\| \*\*(.+?)\*\* \|")
DATE = re.compile(r"\b(\d{4}-\d{2}-\d{2})\b")
# Bare data-file names in a Where cell live in the DATA DIR, not the repo — never "stale".
DATA_FILES = re.compile(r"^[\w-]+\.json$")


def cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def clean(name):
    return re.split(r"\s+\(|\s+—", name.strip())[0].strip()


def rel(path):
    return os.path.relpath(path, ROOT)


class Glossary:
    """TERMS.md as lines plus the row positions of every table the tool edits."""

    def __init__(self):
        self.lines = open(TERMS, encoding="utf-8").read().split("\n")
        self.terms, self.retired, self.candidates, self.aliases, self.duplicates = {}, {}, {}, [], []
        section, self.section_of = None, {}
        for i, line in enumerate(self.lines):
            if line.startswith("## "):
                section = line[3:].strip()
                continue
            m = ROW.match(line)
            if section is None:                      # the "how to read a row" example table
                continue
            if m and section.startswith("13."):
                self.retired[clean(m.group(1))] = i
            elif m and section.startswith("14."):
                self.candidates[clean(m.group(1))] = i
            elif m and section[0].isdigit():
                name = clean(m.group(1))
                if name in self.terms:
                    self.duplicates.append(name)
                self.terms[name] = i
                self.section_of[name] = section
            elif section == "Alias index" and line.startswith('| "'):
                self.aliases.append(i)

    def row(self, i):
        return cells(self.lines[i])

    def set_row(self, i, parts):
        self.lines[i] = "| " + " | ".join(parts) + " |"

    def alias_targets(self, i):
        known = list(self.terms) + list(self.retired) + list(self.candidates)
        return recall.resolve_targets(self.row(i)[1], known)

    def aliases_of(self, name):
        """Lower-cased quoted aliases from the row's Also-called cell and the Alias index."""
        out = []
        parts = self.row(self.terms[name])
        if len(parts) > 2:
            out += re.findall(r'"([^"]+)"', parts[2])
        for i in self.aliases:
            if name in self.alias_targets(i):
                out += re.findall(r'"([^"]+)"', self.row(i)[0])
        return list(dict.fromkeys(a.strip().lower() for a in out))

    def write(self):
        open(TERMS, "w", encoding="utf-8").write("\n".join(self.lines))


def caps_pattern(names):
    alt = "|".join(re.escape(n) for n in sorted(names, key=len, reverse=True))
    return re.compile(r"(?<![A-Z])(?:%s)(?![A-Z])" % alt)


def corpus_lines():
    """Every line outside TERMS.md where a TERM may be written in caps, tagged by file."""
    files = [INDEX, ARCHIVE, GOTCHAS, os.path.join(ROOT, "CLAUDE.md"), os.path.join(ROOT, "README.md"),
             os.path.join(ROOT, "AGENTS.md")]
    if os.path.isdir(ENTRIES):
        files += [os.path.join(ENTRIES, n) for n in sorted(os.listdir(ENTRIES)) if n.endswith(".md")]
    skills = os.path.join(ROOT, ".claude/skills")
    if os.path.isdir(skills):
        for d in sorted(os.listdir(skills)):
            files.append(os.path.join(skills, d, "SKILL.md"))
    out = []
    for f in files:
        if os.path.exists(f):
            for line in open(f, encoding="utf-8"):
                out.append((f, line))
    for dirpath, _dirs, names in os.walk(os.path.join(ROOT, "crates")):
        if "/target" in dirpath:
            continue
        for n in names:
            if n.endswith(".rs"):
                for line in open(os.path.join(dirpath, n), encoding="utf-8", errors="replace"):
                    if "//" in line:
                        out.append((os.path.join(dirpath, n), line))
    return out


def mentions(names, lines):
    """TERM -> list of (file, line) that write it in caps; longest name wins at any position."""
    pat = caps_pattern(names)
    hits = {n: [] for n in names}
    for f, line in lines:
        for m in pat.finditer(line):
            hits[m.group(0)].append((f, line))
    return hits


_FILES = None


def find_files(token):
    """Every repo file a Where pointer may mean: the exact relative path if it exists, else all files
    with that basename (three crates have a `config.rs`)."""
    global _FILES
    if _FILES is None:
        _FILES = {}
        for dirpath, dirs, names in os.walk(ROOT):
            dirs[:] = [d for d in dirs if d not in ("target", ".git", "node_modules")]
            for n in names:
                _FILES.setdefault(n, []).append(os.path.join(dirpath, n))
    for start in (ROOT, os.path.join(ROOT, "crates"), os.path.join(ROOT, ".claude"), os.path.join(ROOT, "vendor")):
        if os.path.exists(os.path.join(start, token)):
            return [os.path.join(start, token)]
    return _FILES.get(os.path.basename(token), [])


def stale_where(where):
    """The backticked pointers in a Where cell that no longer resolve."""
    bad = []
    for tok in re.findall(r"`([^`]+)`", where):
        m = re.match(r"^((?:[\w.-]+/)*[\w-]+\.(?:rs|py|sh|md|toml|json|mdc))(?:::(\w+))?$", tok)
        if not m:
            continue
        path, symbol = m.group(1), m.group(2)
        if DATA_FILES.match(path) and "/" not in path:
            continue
        files = find_files(path)
        if not files:
            bad.append(tok)
        elif symbol and not any(re.search(r"\b" + re.escape(symbol) + r"\b", open(f, encoding="utf-8", errors="replace").read()) for f in files):
            bad.append(tok)
    return bad


def report(g):
    today = datetime.date.today()
    names = list(g.terms)
    lines = corpus_lines()
    hits = mentions(names, lines)
    dead = sorted(n for n in names if not hits[n])
    once = sorted(n for n in names if len(hits[n]) == 1)
    merge = []
    for a in names:
        if len(hits[a]) < 2:
            continue
        others = None
        for _f, line in hits[a]:
            here = {m.group(0) for m in caps_pattern(names).finditer(line)} - {a}
            others = here if others is None else others & here
        for b in sorted(others or ()):
            if len(hits[b]) > len(hits[a]):
                merge.append((a, b, len(hits[a])))
    share = {}
    for n in names:
        for a in g.aliases_of(n):
            share.setdefault(a, set()).add(n)
    collide = sorted((a, sorted(ts)) for a, ts in share.items() if len(ts) >= 3)
    stale = []
    for n, i in g.terms.items():
        parts = g.row(i)
        if len(parts) > 3:
            for tok in stale_where(parts[3]):
                stale.append((n, tok))
    dangling = []
    for i in g.aliases:
        for t in g.alias_targets(i):
            if t not in g.terms and t not in g.retired and t not in g.candidates:
                dangling.append((i + 1, t))
    overdue = []
    for n, i in g.candidates.items():
        dates = [datetime.date.fromisoformat(d) for d in DATE.findall(g.row(i)[2])] if len(g.row(i)) > 2 else []
        if dates and (today - max(dates)).days > CANDIDATE_DAYS:
            overdue.append((n, max(dates).isoformat()))
    memory_lines = [(f, l) for f, l in lines if "/.claude/memory/" in f or f == INDEX]
    ret_hits = mentions(list(g.retired), memory_lines) if g.retired else {}
    unmentioned = sorted(n for n in g.retired if not ret_hits[n])

    def show(label, items, fmt):
        print("%-12s %d" % (label, len(items)))
        for it in items:
            print("             " + fmt(it))
    show("dead", dead, lambda n: n)
    show("once", once, lambda n: "%s  (%s)" % (n, rel(hits[n][0][0])))
    show("merge?", merge, lambda t: "%s is only said with %s (%d lines) → --merge %r --into %r" % (t[0], t[1], t[2], t[0], t[1]))
    show("collide", collide, lambda t: '"%s" → %s' % (t[0], ", ".join(t[1])))
    show("stale", stale, lambda t: "%s → `%s`" % t)
    show("dangling", dangling + [(0, "duplicate row " + d) for d in g.duplicates], lambda t: "TERMS.md:%d → %s" % t if t[0] else t[1])
    show("overdue", overdue, lambda t: "%s (last seen %s) → --prune" % t)
    show("unmentioned", unmentioned, lambda n: "%s (retired, nothing in .claude/memory/ says it) → --prune" % n)
    print("TERMS %d · retired %d · candidates %d · aliases %d" % (len(g.terms), len(g.retired), len(g.candidates), len(share)))
    return not (stale or dangling or g.duplicates)


# ----------------------------------------------------------------------------------------------- actions

def replace_caps(text, old, new):
    """`old` → `new` where old stands alone in caps (not inside a longer TERM)."""
    return re.sub(r"(?<![A-Z])" + re.escape(old) + r"(?![A-Z])", new, text)


def retire(g, name, into, note, merge, dry):
    if name not in g.terms:
        sys.exit("%s is not a TERM row (retired: %s, candidate: %s)" % (name, name in g.retired, name in g.candidates))
    if into and into not in g.terms:
        sys.exit("--into %s is not a TERM row" % into)
    today = datetime.date.today().isoformat()
    parts = g.row(g.terms[name])
    meaning, also, where = (parts + ["", "", ""])[1:4]
    plan = []
    # 1. B's Also-called gains A's name and aliases.
    if into:
        b = g.row(g.terms[into])
        have = set(re.findall(r'"([^"]+)"', b[2])) if len(b) > 2 else set()
        add = [a for a in dict.fromkeys([name.lower()] + g.aliases_of(name)) if a not in {h.lower() for h in have}]
        if add:
            b[2] = ", ".join([b[2]] * bool(b[2].strip()) + ['"%s"' % a for a in add])
            plan.append("%s Also called += %s" % (into, ", ".join(add)))
            if not dry:
                g.set_row(g.terms[into], b)
    # 2. Alias-index targets: A → B, or A dropped (row deleted when empty).
    drop = []
    for i in g.aliases:
        targets = g.alias_targets(i)
        if name not in targets:
            continue
        new = [t for t in targets if t != name]
        if into and into not in new:
            new.append(into)
        if not new:
            drop.append(i)
            plan.append("Alias index: delete %r" % g.row(i)[0][:60])
        else:
            plan.append("Alias index: %r → %s" % (g.row(i)[0][:60], " / ".join(new)))
            if not dry:
                g.set_row(i, [g.row(i)[0], " / ".join(new)])
    # 3. Cross-references from other rows' meaning cells.
    xrefs = 0
    for n, i in list(g.terms.items()) + list(g.candidates.items()):
        if n == name:
            continue
        line = g.lines[i]
        if re.search(r"(?<![A-Z])" + re.escape(name) + r"(?![A-Z])", line):
            xrefs += 1
            if into and not dry:
                # B's own row cannot cite itself: there the old name becomes plain words.
                g.lines[i] = replace_caps(line, name, name.lower() if n == into else into)
    if xrefs:
        plan.append("%d rows cross-reference %s%s" % (xrefs, name, " → " + into if into else " (left as is — they now point at a Retired row)"))
    # 4. Row → Retired.
    what = meaning + (" Merged into %s." % into if merge else (" Replaced by %s." % into if into else "")) + (" " + note if note else "")
    what += " Was: %s" % where if where else ""
    retired_row = "| **%s** | %s | %s |" % (name, what, today)
    plan.append("Retired: " + retired_row[:100] + "…")
    if not dry:
        insert_at = max(g.retired.values()) + 1 if g.retired else None
        if insert_at is None:
            j = [k for k, l in enumerate(g.lines) if l.startswith("## 13.")][0]
            while not g.lines[j].startswith("|---"):
                j += 1
            insert_at = j + 1
        for i in [g.terms[name]] + drop:      # mark, insert, then filter — no index arithmetic
            g.lines[i] = None
        g.lines.insert(insert_at, retired_row)
        g.lines = [l for l in g.lines if l is not None]
        g.write()
    # 5. MEMORY LOG index cells and gotchas.md keys.
    for path in (INDEX, ARCHIVE):
        if not os.path.exists(path):
            continue
        text = open(path, encoding="utf-8").read()
        n = 0
        out = []
        for line in text.split("\n"):
            m = re.match(r"^(- \d{4}-\d{2}-\d{2}\S* · \[.+?\]\(.+?\) · TERMS: )(.*?)((?: · files: .*?)?(?: · gotchas: \d+)?)$", line)
            if m and re.search(r"(?<![A-Z])" + re.escape(name) + r"(?![A-Z])", m.group(2)):
                terms = [t.strip() for t in m.group(2).split(";")]
                terms = list(dict.fromkeys(into if t == name else t for t in terms if t != name or into))
                line = m.group(1) + "; ".join(terms) + m.group(3)
                n += 1
            out.append(line)
        if n:
            plan.append("%s: %d index cells %s" % (rel(path), n, "→ " + into if into else "drop " + name))
            if not dry:
                open(path, "w", encoding="utf-8").write("\n".join(out))
    if into and os.path.exists(GOTCHAS):
        text = open(GOTCHAS, encoding="utf-8").read()
        new, n = re.subn(r"^- \*\*" + re.escape(name) + r"\*\* — ", "- **%s** — " % into, text, flags=re.M)
        if n:
            plan.append("gotchas.md: %d keys **%s** → **%s**" % (n, name, into))
            if not dry:
                open(GOTCHAS, "w", encoding="utf-8").write(new)
    print(("DRY RUN — " if dry else "") + ("merge %s into %s" % (name, into) if merge else "retire %s" % name))
    for p in plan:
        print("  " + p)
    if merge:
        print("  fold by hand if %s's row does not already say it: %s" % (into, meaning))
    return True


def prune(g, dry):
    today = datetime.date.today()
    lines = corpus_lines()
    memory_lines = [(f, l) for f, l in lines if "/.claude/memory/" in f or f == INDEX]
    ret_hits = mentions(list(g.retired), memory_lines) if g.retired else {}
    kill = []
    for n, i in g.candidates.items():
        dates = [datetime.date.fromisoformat(d) for d in DATE.findall(g.row(i)[2])] if len(g.row(i)) > 2 else []
        if dates and (today - max(dates)).days > CANDIDATE_DAYS:
            kill.append((i, "candidate %s (last seen %s)" % (n, max(dates))))
    for n in g.retired:
        if not ret_hits[n]:
            kill.append((g.retired[n], "retired %s (unmentioned)" % n))
    print(("DRY RUN — " if dry else "") + "prune %d rows" % len(kill))
    for _i, what in kill:
        print("  " + what)
    if not dry:
        for i, _what in sorted(kill, reverse=True):
            del g.lines[i]
        g.write()
    return True


def eval_ok():
    r = subprocess.run([sys.executable, os.path.join(ROOT, ".claude/memory/recall_eval.py")], capture_output=True, text=True)
    print(r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr.strip())
    return r.returncode == 0


def main(argv):
    def opt(flag):
        return argv[argv.index(flag) + 1] if flag in argv else None
    dry = "--dry-run" in argv
    g = Glossary()
    if "--merge" in argv:
        into = opt("--into") or sys.exit("--merge A --into B")
        retire(g, opt("--merge"), into, opt("--note"), True, dry)
    elif "--retire" in argv:
        retire(g, opt("--retire"), opt("--into"), opt("--note"), False, dry)
    elif "--prune" in argv:
        prune(g, dry)
    else:
        ok = report(g)
        print("terms-check: " + ("ok" if ok else "FAIL — fix the stale / dangling rows above"))
        return 0 if ok else 1
    if dry:
        return 0
    if not eval_ok():
        print("terms-check: recall regressed — `git checkout TERMS.md .claude/MEMORY.md .claude/memory/gotchas.md` to undo, or `recall_eval.py --update` if the change is deliberate")
        return 1
    print("terms-check: written")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
