#!/usr/bin/env python3
"""SKILL AUDIT HOOK — a Claude Code `Stop` hook.

Fires once per closed task: after a turn in which the NEBULA-MEMORY SKILL ran (the SELF-IMPROVING
LOOP's own "a task ended" signal), not again until another task closes, and not within the per-project
cooldown (off by default — every closed task is audited; `NEBULA_SKILL_AUDIT_COOLDOWN_MIN` spaces them
out when several sessions run at once). It judges nothing
itself. It hands the agent — which still holds the whole turn in context — a short brief: which skills
it invoked this session, how long each body is, and what to answer about each (what in the body it
followed, what it ignored, what cost tokens and changed nothing), then asks it to put at most three
proposals to the user as one AskUserQuestion, or, unattended, to write them to
`.claude/memory/skill-audit/`. The user picks; the agent edits; the audit never rewrites a skill on
its own.

Returns `{"decision":"block","reason":<brief>}` to keep the turn going; prints nothing and exits 0
otherwise — and on every error, so a broken hook never traps a session.

Input (stdin, from Claude Code): {"session_id","transcript_path","cwd","hook_event_name":"Stop",
"stop_hook_active"}. `stop_hook_active` is true while the agent is already continuing because of a
Stop hook; that is the audit turn itself, and it is never re-triggered.

State (ephemeral on purpose): /tmp/nebula-skill-audit-<uid>/ — per session, how many memory writes had
happened at the last audit; per project, when the last audit ran.

Env: NEBULA_SKILL_AUDIT=off disables it · NEBULA_SKILL_AUDIT_COOLDOWN_MIN (default 0) ·
NEBULA_SKILL_AUDIT_TRIGGER=any fires after any skill, not only nebula-memory ·
NEBULA_SKILL_AUDIT_STATE overrides the state dir (tests).

Try it by hand:
    python3 .claude/hooks/skill_audit.py --dry-run <transcript.jsonl>     # prints the brief, writes no state
"""
import json
import os
import re
import sys
import time

TRIGGER_SKILL = "nebula-memory"
DEFAULT_COOLDOWN_MIN = 0  # every closed task (the user's pick, 2026-09-05); raise it to space audits out
MAX_PROPOSALS = 3


def state_dir():
    d = os.environ.get("NEBULA_SKILL_AUDIT_STATE") or "/tmp/nebula-skill-audit-%d" % os.getuid()
    os.makedirs(d, mode=0o700, exist_ok=True)
    return d


def read_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except Exception:
        return default


def write_json(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(data, f)
    os.replace(tmp, path)


def skills_invoked(transcript_path):
    """Every Skill tool call in the transcript, in order — the skill names as the agent typed them."""
    names = []
    with open(transcript_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if '"Skill"' not in line:
                continue
            try:
                rec = json.loads(line)
            except Exception:
                continue
            if rec.get("type") != "assistant":
                continue
            for block in (rec.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == "Skill":
                    skill = (block.get("input") or {}).get("skill")
                    if skill:
                        names.append(skill.split(":")[-1])
    return names


def skill_file(name, project):
    for root in (os.path.join(project, ".claude", "skills"), os.path.expanduser("~/.claude/skills")):
        path = os.path.join(root, name, "SKILL.md")
        if os.path.isfile(path):
            return path
    return None


def size_of(path):
    try:
        text = open(path, encoding="utf-8", errors="replace").read()
    except Exception:
        return (0, 0)
    return (text.count("\n") + 1, len(re.findall(r"\S+", text)))


def project_skills(project):
    root = os.path.join(project, ".claude", "skills")
    try:
        return sorted(n for n in os.listdir(root) if os.path.isfile(os.path.join(root, n, "SKILL.md")))
    except Exception:
        return []


def brief(used, project, session_id):
    """The block reason: the facts the hook can compute, then the questions only the agent can answer."""
    counts = {}
    for n in used:
        counts[n] = counts.get(n, 0) + 1
    rows = []
    for name, n in counts.items():
        path = skill_file(name, project)
        if path:
            lines, words = size_of(path)
            rows.append("  - %s — invoked %d×, body %d lines / %d words (%s)" % (name, n, lines, words, os.path.relpath(path, project) if path.startswith(project) else path))
        else:
            rows.append("  - %s — invoked %d×, no SKILL.md found (a built-in?)" % (name, n))
    unused = [s for s in project_skills(project) if s not in counts]
    unused_line = ""
    if unused:
        unused_line = "\nNot invoked this session (out of scope unless a proposal below is *about* one): " + ", ".join(unused) + "."
    date = time.strftime("%Y-%m-%d")
    report = ".claude/memory/skill-audit/%s-%s.md" % (date, session_id[:8])
    return (
        "SKILL AUDIT (nebula Stop hook — fires once per closed task). A task just closed and this session "
        "invoked these skills:\n" + "\n".join(rows) + unused_line + "\n\n"
        "Before you stop, audit each one from your own context of this session — do not re-read the "
        "bodies unless you must quote a passage — answering, per skill: (1) which sections you actually "
        "followed; (2) which you did not need — a rule that never applied, an example that taught nothing "
        "new, a paragraph restating another; (3) what it cost (tool calls, a question, a correction from "
        "the user, reply length) against what it changed in the result.\n\n"
        "Then propose at most %d concrete edits, each one of: a CUT (name the heading or the first words of "
        "the passage and the lines it saves), a MERGE of two passages, a TIGHTENED rule (old sentence → new "
        "sentence), or a NEW SKILL for a workflow you repeated by hand this session and would again. A "
        "proposal must name a moment in this session it would have changed. Put them to the user as ONE "
        "AskUserQuestion (multiSelect; one option per proposal with the estimated lines saved or added in "
        "its description; a final 'None of these' option). Apply exactly what they pick, keeping every "
        "skill's frontmatter and trigger phrases intact, and stop. If nothing is worth proposing, say so in "
        "one line and stop — never invent a cut to have something to show. If no user can answer (a -p "
        "run), write the proposals to %s instead and stop.\n\n"
        "The audit is not a task: no MEMORY LOG entry, no PROJECT TERMS pass, no OUTPUT DOCTOR layout — a "
        "short plain message and the question." % (MAX_PROPOSALS, report)
    )


def decide(data, now=None, dry_run=False):
    """None to let the stop through, or the brief to block with."""
    if os.environ.get("NEBULA_SKILL_AUDIT", "").lower() in ("off", "0", "false", "no"):
        return None
    if data.get("stop_hook_active"):
        return None
    transcript = data.get("transcript_path")
    session_id = data.get("session_id") or "unknown"
    project = os.environ.get("CLAUDE_PROJECT_DIR") or data.get("cwd") or os.getcwd()
    if not transcript or not os.path.isfile(transcript):
        return None
    used = skills_invoked(transcript)
    if not used:
        return None
    trigger_any = os.environ.get("NEBULA_SKILL_AUDIT_TRIGGER", "").lower() == "any"
    closes = len(used) if trigger_any else used.count(TRIGGER_SKILL)
    if closes == 0:
        return None
    if dry_run:
        # "What would the brief say" — no state read or written, no cooldown.
        return brief(used, project, session_id)
    now = now or time.time()
    sdir = state_dir()
    session_state = os.path.join(sdir, "session-%s.json" % re.sub(r"[^\w.-]", "_", session_id))
    seen = read_json(session_state, {}).get("closes", 0)
    if closes <= seen:
        return None
    project_key = re.sub(r"[^\w.-]", "_", project.strip("/"))[-120:]
    project_state = os.path.join(sdir, "project-%s.json" % project_key)
    last = read_json(project_state, {}).get("last_audit", 0)
    cooldown = float(os.environ.get("NEBULA_SKILL_AUDIT_COOLDOWN_MIN") or DEFAULT_COOLDOWN_MIN) * 60
    # This task is spoken for either way: audited now, or skipped under the cooldown.
    write_json(session_state, {"closes": closes})
    if now - last < cooldown:
        return None
    write_json(project_state, {"last_audit": now})
    return brief(used, project, session_id)


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "--dry-run":
        data = {"session_id": "dry-run", "transcript_path": sys.argv[2], "cwd": os.getcwd(), "stop_hook_active": False}
        reason = decide(data, dry_run=True)
        print(reason or "(would not fire)")
        return 0
    try:
        data = json.load(sys.stdin)
    except Exception:
        return 0
    try:
        reason = decide(data)
    except Exception:
        return 0
    if reason:
        print(json.dumps({"decision": "block", "reason": reason}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
