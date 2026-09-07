#!/usr/bin/env python3
"""Summarize one `claude -p --output-format stream-json --verbose` run.

usage: analyze_stream.py <label> <stream.jsonl> [wall_seconds_file] > summary.json
"""
import collections
import json
import re
import sys

label = sys.argv[1]
path = sys.argv[2]
wall = None
if len(sys.argv) > 3:
    try:
        wall = int(open(sys.argv[3]).read().strip())
    except Exception:
        wall = None

lines = open(path).read().splitlines()
init = None
result = None
per_msg = {}
api_calls = 0
usage_sum = collections.Counter()
peak_context = 0
tool_calls = collections.Counter()
skills = []
agent_spawns = []
bash_cmds = []
bash_kinds = collections.Counter()
edited = collections.OrderedDict()
read_paths = collections.OrderedDict()
thinking_blocks = 0
text_blocks = 0
tool_errors = 0
denied = 0
guard_blocks = 0
error_samples = []
timeline = []  # per deduped assistant message: cumulative output tokens, context size
turn_idx = 0
cum_out = 0
cum_cost_proxy = 0.0
final_text = ""
rate_limit_events = 0
compactions = 0
tool_use_ids = {}


def classify_bash(cmd):
    c = cmd.strip()
    head = c.split("&&")[0].strip()
    m = re.match(r"^(cd\s+\S+\s*(?:&&|;)\s*)?(\S+)", head)
    first = (m.group(2) if m else head.split(" ")[0]) if head else ""
    if "cargo test" in c:
        return "cargo test"
    if "cargo clippy" in c:
        return "cargo clippy"
    if "cargo build" in c or "cargo check" in c:
        return "cargo build/check"
    if "cargo fmt" in c:
        return "cargo fmt"
    if re.search(r"\bmake\b", c):
        return "make"
    if re.match(r"^(git)\b", first):
        return "git"
    if re.match(r"^(grep|rg|find|ls|cat|head|tail|sed -n|wc|awk|tree)\b", first) or re.search(r"\b(grep|rg)\b", head):
        return "read/search"
    if re.match(r"^python3?\b", first):
        return "python"
    if re.search(r"\bsed -i\b|<<'?EOF|cat >", c):
        return "write via shell"
    return "other"


for raw in lines:
    try:
        d = json.loads(raw)
    except Exception:
        continue
    t = d.get("type")
    if t == "system":
        sub = d.get("subtype")
        if sub == "init":
            init = d
        elif sub == "rate_limit_event":
            rate_limit_events += 1
        elif sub in ("compact_boundary", "compaction"):
            compactions += 1
        continue
    if t == "rate_limit_event":
        rate_limit_events += 1
        continue
    if t == "result":
        result = d
        final_text = d.get("result") or ""
        continue
    if t == "assistant":
        m = d.get("message", {})
        mid = m.get("id")
        content = m.get("content", []) or []
        for c in content:
            ct = c.get("type")
            if ct == "thinking":
                thinking_blocks += 1
            elif ct == "text":
                text_blocks += 1
            elif ct == "tool_use":
                name = c.get("name", "?")
                inp = c.get("input", {}) or {}
                tool_calls[name] += 1
                tool_use_ids[c.get("id")] = name
                if name == "Skill":
                    skills.append(inp.get("skill"))
                elif name == "Agent":
                    agent_spawns.append({"type": inp.get("subagent_type"), "desc": inp.get("description")})
                elif name == "Bash":
                    cmd = inp.get("command", "")
                    bash_cmds.append(cmd)
                    bash_kinds[classify_bash(cmd)] += 1
                elif name in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
                    p = inp.get("file_path")
                    if p:
                        edited[p] = edited.get(p, 0) + 1
                elif name == "Read":
                    p = inp.get("file_path")
                    if p:
                        read_paths[p] = read_paths.get(p, 0) + 1
        u = m.get("usage") or {}
        if u and mid:
            # every record of one API response repeats its usage, with output_tokens
            # growing across the streamed chunks: keep the max of each field per id
            rec = per_msg.setdefault(mid, {"model": m.get("model"), "order": len(per_msg)})
            for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"):
                rec[k] = max(rec.get(k, 0), int(u.get(k) or 0))
        continue
    if t == "user":
        m = d.get("message", {})
        content = m.get("content")
        if isinstance(content, list):
            for c in content:
                if c.get("type") == "tool_result":
                    body = c.get("content")
                    text = ""
                    if isinstance(body, str):
                        text = body
                    elif isinstance(body, list):
                        text = " ".join(x.get("text", "") for x in body if isinstance(x, dict))
                    if c.get("is_error"):
                        tool_errors += 1
                        low = text.lower()
                        if "denied" in low and ("classifier" in low or "permission" in low):
                            denied += 1
                        if "nebula guard" in low or "pretooluse:bash hook error" in low:
                            guard_blocks += 1
                        if len(error_samples) < 12:
                            error_samples.append({"tool": tool_use_ids.get(c.get("tool_use_id"), "?"), "text": text[:300]})

for mid, rec in sorted(per_msg.items(), key=lambda kv: kv[1]["order"]):
    api_calls += 1
    for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"):
        usage_sum[k] += rec.get(k, 0)
    ctx = rec.get("input_tokens", 0) + rec.get("cache_read_input_tokens", 0) + rec.get("cache_creation_input_tokens", 0)
    peak_context = max(peak_context, ctx)
    cum_out += rec.get("output_tokens", 0)
    turn_idx += 1
    timeline.append({"i": turn_idx, "context": ctx, "output": rec.get("output_tokens", 0), "cum_output": cum_out, "model": rec.get("model")})

summary = {
    "label": label,
    "model": (init or {}).get("model"),
    "permission_mode": (init or {}).get("permissionMode"),
    "claude_code_version": (init or {}).get("claude_code_version"),
    "skills_available": (init or {}).get("skills") if isinstance((init or {}).get("skills"), int) else len((init or {}).get("skills") or []),
    "finished": result is not None,
    "result_subtype": (result or {}).get("subtype"),
    "is_error": (result or {}).get("is_error"),
    "num_turns": (result or {}).get("num_turns"),
    "duration_ms": (result or {}).get("duration_ms"),
    "duration_api_ms": (result or {}).get("duration_api_ms"),
    "wall_seconds": wall,
    "total_cost_usd": (result or {}).get("total_cost_usd"),
    "usage": (result or {}).get("usage"),
    "model_usage": (result or {}).get("modelUsage"),
    "stream_usage_sum": dict(usage_sum),
    "api_calls_deduped": api_calls,
    "peak_context_tokens": peak_context,
    "tool_calls": dict(tool_calls),
    "tool_calls_total": sum(tool_calls.values()),
    "skills_invoked": skills,
    "agent_spawns": agent_spawns,
    "bash_kinds": dict(bash_kinds),
    "bash_commands": bash_cmds,
    "files_edited": edited,
    "files_read": read_paths,
    "thinking_blocks": thinking_blocks,
    "text_blocks": text_blocks,
    "tool_errors": tool_errors,
    "denied_by_classifier": denied,
    "guard_hook_blocks": guard_blocks,
    "error_samples": error_samples,
    "rate_limit_events": rate_limit_events,
    "compactions": compactions,
    "timeline": timeline,
    "final_text": final_text,
}
json.dump(summary, sys.stdout, indent=1)
print()
