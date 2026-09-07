#!/usr/bin/env python3
"""usage: make_report.py <ab-dir>

Render <ab-dir>/out/report.html from the two arms' summaries, audits, the blinded review and the narrative."""
import html
import json
import os
import sys

AB = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.getcwd()  # the A/B directory: clones, feature-prompt.md, out/
OUT = os.path.join(AB, "out")


def load(name, default=None):
    p = os.path.join(OUT, name)
    if not os.path.exists(p):
        return default
    return json.load(open(p))


S = {"A": load("a.summary.json", {}), "B": load("b.summary.json", {})}
AU = {"A": load("a.audit.json", {}), "B": load("b.audit.json", {})}
REV = load("review.json", {})
NAR = load("narrative.json", {})
PROMPT = open(os.path.join(AB, "feature-prompt.md")).read()
ARM_NAME = {"A": "A · with skills", "B": "B · bare"}
ARM_LONG = {"A": "Arm A — repo as-is: CLAUDE.md, AGENTS.md, TERMS.md, 9 project skills, 3 hooks, MEMORY LOG",
            "B": "Arm B — bare: all of that deleted, Fable 5.1 with only the code and docs"}

esc = html.escape


def n0(x):
    return 0 if x is None else x


def fmt_int(n):
    return "—" if n is None else f"{int(n):,}"


def fmt_k(n):
    if n is None:
        return "—"
    n = float(n)
    if n >= 1e6:
        return f"{n/1e6:.2f}M"
    if n >= 1e3:
        return f"{n/1e3:.1f}k"
    return f"{n:.0f}"


def fmt_usd(x):
    return "—" if x is None else f"${x:,.2f}"


def fmt_secs(s):
    if s is None:
        return "—"
    s = int(round(s))
    m, sec = divmod(s, 60)
    h, m = divmod(m, 60)
    return f"{h}h {m:02d}m" if h else f"{m}m {sec:02d}s"


def ratio_chip(a, b, lower_is_better=True, label="B vs A"):
    """A small chip saying how B compares to A."""
    if not a or b is None:
        return ""
    r = (b - a) / a * 100
    if abs(r) < 1:
        return f'<span class="chip chip-flat">B ≈ A</span>'
    good = (r < 0) if lower_is_better else (r > 0)
    cls = "chip-good" if good else "chip-bad"
    arrow = "▼" if r < 0 else "▲"
    return f'<span class="chip {cls}" title="{label}">{arrow} {abs(r):.0f}% {"lower" if r < 0 else "higher"} in B</span>'


def usage(arm, key):
    u = (S[arm].get("usage") or {})
    return u.get(key)


def total_tokens(arm):
    u = S[arm].get("usage") or {}
    return sum(n0(u.get(k)) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))


# ---------- charts (inline SVG, drawn to scale) ----------
def small_multiple(title, a, b, fmt=fmt_k, unit=""):
    a, b = n0(a), n0(b)
    mx = max(a, b, 1)
    W, H, pad_l, pad_r = 300, 74, 8, 70
    bw = W - pad_l - pad_r
    rows = ""
    for i, (arm, v) in enumerate((("A", a), ("B", b))):
        y = 14 + i * 28
        w = max(2, bw * v / mx)
        rows += (f'<rect class="bar-{arm}" x="{pad_l}" y="{y}" width="{w:.1f}" height="18" rx="0" '
                 f'data-tip="{esc(ARM_NAME[arm])}: {esc(fmt(v))}{unit}"></rect>'
                 f'<text class="val" x="{pad_l + w + 6:.1f}" y="{y + 13}">{esc(fmt(v))}{unit}</text>')
    chip = ratio_chip(a, b)
    return (f'<figure class="sm"><figcaption>{esc(title)} {chip}</figcaption>'
            f'<svg viewBox="0 0 {W} {H}" width="100%" role="img" aria-label="{esc(title)}: A {esc(fmt(a))}, B {esc(fmt(b))}">'
            f'<line class="baseline" x1="{pad_l}" y1="8" x2="{pad_l}" y2="{H-6}"/>{rows}</svg></figure>')


def paired_bars(rows, title, fmt=fmt_int, note=""):
    """rows: [(label, a, b)] horizontal paired bars, one scale."""
    if not rows:
        return ""
    mx = max(max(n0(a), n0(b)) for _, a, b in rows) or 1
    W, lab_w, val_w = 720, 190, 60
    bw = W - lab_w - val_w - 16
    rh = 44
    H = 10 + rh * len(rows) + 8
    body = ""
    for i, (label, a, b) in enumerate(rows):
        y0 = 10 + i * rh
        body += f'<text class="lab" x="{lab_w - 10}" y="{y0 + 24}" text-anchor="end">{esc(str(label))}</text>'
        for j, (arm, v) in enumerate((("A", a), ("B", b))):
            v = n0(v)
            y = y0 + 4 + j * 18
            w = max(1.5, bw * v / mx) if v else 0
            body += (f'<rect class="bar-{arm}" x="{lab_w}" y="{y}" width="{w:.1f}" height="14" '
                     f'data-tip="{esc(ARM_NAME[arm])} · {esc(str(label))}: {esc(fmt(v))}"></rect>'
                     f'<text class="val small" x="{lab_w + w + 5:.1f}" y="{y + 11}">{esc(fmt(v))}</text>')
    legend = ('<div class="legend"><span><i class="sw sw-A"></i>A · with skills</span>'
              '<span><i class="sw sw-B"></i>B · bare</span></div>')
    table = "".join(f"<tr><td>{esc(str(l))}</td><td>{esc(fmt(n0(a)))}</td><td>{esc(fmt(n0(b)))}</td></tr>" for l, a, b in rows)
    return (f'<figure class="chart"><figcaption>{esc(title)}{(" <span class=note>" + esc(note) + "</span>") if note else ""}</figcaption>{legend}'
            f'<div class="scroll"><svg viewBox="0 0 {W} {H}" width="100%" style="min-width:560px" role="img" aria-label="{esc(title)}">'
            f'<line class="baseline" x1="{lab_w}" y1="6" x2="{lab_w}" y2="{H-4}"/>{body}</svg></div>'
            f'<details><summary>Table view</summary><table class="t"><thead><tr><th></th><th>A</th><th>B</th></tr></thead><tbody>{table}</tbody></table></details></figure>')


def context_line_chart():
    ta = S["A"].get("timeline") or []
    tb = S["B"].get("timeline") or []
    if not ta and not tb:
        return ""
    W, H, pl, pr, pt, pb = 760, 300, 64, 90, 16, 34
    xmax = max(len(ta), len(tb), 2)
    ymax = max([p["context"] for p in ta + tb] + [1])
    # clean tick: round up to a nice number
    import math
    mag = 10 ** math.floor(math.log10(ymax))
    ytop = math.ceil(ymax / mag) * mag
    if ytop / mag <= 2:
        ytop = math.ceil(ymax / (mag / 4)) * (mag / 4)

    def X(i):
        return pl + (W - pl - pr) * (i - 1) / (xmax - 1)

    def Y(v):
        return pt + (H - pt - pb) * (1 - v / ytop)

    grid = ""
    for k in range(5):
        v = ytop * k / 4
        grid += f'<line class="grid" x1="{pl}" y1="{Y(v):.1f}" x2="{W-pr}" y2="{Y(v):.1f}"/><text class="tick" x="{pl-8}" y="{Y(v)+4:.1f}" text-anchor="end">{esc(fmt_k(v))}</text>'
    xt = ""
    step = max(1, round(xmax / 8))
    for i in range(1, xmax + 1, step):
        xt += f'<text class="tick" x="{X(i):.1f}" y="{H-12}" text-anchor="middle">{i}</text>'
    paths = ""
    dots = ""
    ends = ""
    for arm, tl in (("A", ta), ("B", tb)):
        if not tl:
            continue
        pts = " ".join(f"{X(p['i']):.1f},{Y(p['context']):.1f}" for p in tl)
        paths += f'<polyline class="line-{arm}" points="{pts}"/>'
        for p in tl:
            dots += (f'<circle class="dot-{arm}" cx="{X(p["i"]):.1f}" cy="{Y(p["context"]):.1f}" r="4" '
                     f'data-tip="{esc(ARM_NAME[arm])} · API call {p["i"]}: {esc(fmt_int(p["context"]))} tokens of context"></circle>')
        last = tl[-1]
        ends += f'<text class="endlab" x="{X(last["i"]) + 8:.1f}" y="{Y(last["context"]) + 4:.1f}">{arm} · {esc(fmt_k(last["context"]))}</text>'
    peak = f'A peak {fmt_k(S["A"].get("peak_context_tokens"))} · B peak {fmt_k(S["B"].get("peak_context_tokens"))}'
    legend = ('<div class="legend"><span><i class="sw sw-A"></i>A · with skills</span>'
              '<span><i class="sw sw-B"></i>B · bare</span></div>')
    rows = ""
    for i in range(1, xmax + 1):
        a = next((p["context"] for p in ta if p["i"] == i), None)
        b = next((p["context"] for p in tb if p["i"] == i), None)
        rows += f"<tr><td>{i}</td><td>{fmt_int(a)}</td><td>{fmt_int(b)}</td></tr>"
    return (f'<figure class="chart"><figcaption>Context size per API call <span class="note">tokens sent per request (input + cache read + cache write) · {esc(peak)}</span></figcaption>{legend}'
            f'<div class="scroll"><svg viewBox="0 0 {W} {H}" width="100%" style="min-width:600px" role="img" aria-label="Context size per API call, two lines">'
            f'{grid}<line class="baseline" x1="{pl}" y1="{Y(0):.1f}" x2="{W-pr}" y2="{Y(0):.1f}"/>{xt}'
            f'<text class="tick" x="{(pl + W - pr)/2:.0f}" y="{H-1}" text-anchor="middle">API call №</text>'
            f'{paths}{dots}{ends}</svg></div>'
            f'<details><summary>Table view</summary><div class="scroll"><table class="t"><thead><tr><th>call</th><th>A context</th><th>B context</th></tr></thead><tbody>{rows}</tbody></table></div></details></figure>')


# ---------- sections ----------
def status_pill(ok, text_ok="pass", text_bad="fail", unknown="not run"):
    if ok is None:
        return f'<span class="pill pill-unknown">◌ {unknown}</span>'
    return f'<span class="pill pill-good">✓ {text_ok}</span>' if ok else f'<span class="pill pill-bad">✗ {text_bad}</span>'


def kpi_tile(label, a_text, b_text, chip="", sub_a="", sub_b=""):
    return (f'<div class="kpi"><div class="kpi-label">{esc(label)}</div>'
            f'<div class="kpi-pair"><div class="kpi-v"><i class="sw sw-A"></i><b>{esc(a_text)}</b><small>{esc(sub_a)}</small></div>'
            f'<div class="kpi-v"><i class="sw sw-B"></i><b>{esc(b_text)}</b><small>{esc(sub_b)}</small></div></div>'
            f'<div class="kpi-chip">{chip}</div></div>')


def section_kpis():
    A, B = S["A"], S["B"]
    tiles = [
        kpi_tile("Cost", fmt_usd(A.get("total_cost_usd")), fmt_usd(B.get("total_cost_usd")), ratio_chip(A.get("total_cost_usd"), B.get("total_cost_usd"))),
        kpi_tile("Wall time", fmt_secs(A.get("wall_seconds")), fmt_secs(B.get("wall_seconds")), ratio_chip(A.get("wall_seconds"), B.get("wall_seconds")), "both ran in parallel", "both ran in parallel"),
        kpi_tile("API time", fmt_secs((A.get("duration_api_ms") or 0) / 1000), fmt_secs((B.get("duration_api_ms") or 0) / 1000), ratio_chip(A.get("duration_api_ms"), B.get("duration_api_ms")), "model thinking + generating", "model thinking + generating"),
        kpi_tile("API calls", fmt_int(A.get("api_calls_deduped")), fmt_int(B.get("api_calls_deduped")), ratio_chip(A.get("api_calls_deduped"), B.get("api_calls_deduped")), f'{fmt_int(A.get("num_turns"))} turns', f'{fmt_int(B.get("num_turns"))} turns'),
        kpi_tile("Tokens sent", fmt_k(total_tokens("A") - n0(usage("A", "output_tokens"))), fmt_k(total_tokens("B") - n0(usage("B", "output_tokens"))), ratio_chip(total_tokens("A") - n0(usage("A", "output_tokens")), total_tokens("B") - n0(usage("B", "output_tokens"))), "input + cache", "input + cache"),
        kpi_tile("Tokens generated", fmt_k(usage("A", "output_tokens")), fmt_k(usage("B", "output_tokens")), ratio_chip(usage("A", "output_tokens"), usage("B", "output_tokens"))),
        kpi_tile("Peak context", fmt_k(A.get("peak_context_tokens")), fmt_k(B.get("peak_context_tokens")), ratio_chip(A.get("peak_context_tokens"), B.get("peak_context_tokens"))),
        kpi_tile("Tool calls", fmt_int(A.get("tool_calls_total")), fmt_int(B.get("tool_calls_total")), ratio_chip(A.get("tool_calls_total"), B.get("tool_calls_total")), f'{n0(A.get("tool_errors"))} errored', f'{n0(B.get("tool_errors"))} errored'),
        kpi_tile("Code changed", f'+{fmt_int(AU["A"].get("code_added"))} / −{fmt_int(AU["A"].get("code_deleted"))}', f'+{fmt_int(AU["B"].get("code_added"))} / −{fmt_int(AU["B"].get("code_deleted"))}', "", f'{n0(AU["A"].get("code_files"))} files · {n0(AU["A"].get("new_test_fns"))} new tests', f'{n0(AU["B"].get("code_files"))} files · {n0(AU["B"].get("new_test_fns"))} new tests'),
    ]
    return '<section id="numbers"><h2>The numbers</h2><div class="kpis">' + "".join(tiles) + "</div></section>"


PH = {"A": load("a.phases.json", {}), "B": load("b.phases.json", {})}


def section_phases():
    if not PH["A"] or not PH["B"]:
        return ""
    names = ["explore (before the first edit)", "build + verify", "close the loop (memory, terms, output shape, skill audit)"]

    def get(arm, name, key):
        for p in PH[arm].get("phases", []):
            if p["name"] == name:
                return p.get(key)
        return None

    short = {"explore (before the first edit)": "explore (before first edit)", "build + verify": "build + verify", "close the loop (memory, terms, output shape, skill audit)": "close the loop (A only)"}
    rows = [(short[n], get("A", n, "input_side_cost_usd"), get("B", n, "input_side_cost_usd")) for n in names]
    calls = [(short[n], get("A", n, "calls"), get("B", n, "calls")) for n in names]
    t = "".join(f"<tr><td>{esc(n)}</td><td class=num>{fmt_int(get('A', n, 'calls'))}</td><td class=num>{fmt_usd(get('A', n, 'input_side_cost_usd'))}</td><td class=num>{fmt_int(get('B', n, 'calls'))}</td><td class=num>{fmt_usd(get('B', n, 'input_side_cost_usd'))}</td></tr>" for n in names)
    fa, fb = PH["A"].get("first_call_context"), PH["B"].get("first_call_context")
    return ('<section id="phases"><h2>Where the money went, by phase</h2>'
            '<p>The input side of the bill (uncached input, cache reads and cache writes at Fable 5.1 list prices; it reproduces Claude Code\'s reported total to the cent once output tokens are added) split by what the agent was doing. Output tokens cannot be attributed per call from the stream, so they are left out of this split.</p>'
            f'{paired_bars(rows, "Input-side cost by phase (USD)", fmt=fmt_usd, note="the third phase exists only in A — it is the SELF-IMPROVING LOOP\'s closing chores")}'
            f'{paired_bars(calls, "API calls by phase", fmt=fmt_int)}'
            f'<p>The first request of the run already carried <b>{fmt_k(fa)}</b> tokens of context in A against <b>{fmt_k(fb)}</b> in B: that ~{fmt_k((fa or 0) - (fb or 0))}-token difference is CLAUDE.md, AGENTS.md, the skill index and the recall hook\'s injection, and it is re-read on every one of A\'s {fmt_int(PH["A"].get("api_calls"))} calls (cheaply, as cache reads).</p>'
            f'<details><summary>Table view</summary><table class="t"><thead><tr><th>phase</th><th>A calls</th><th>A input-side $</th><th>B calls</th><th>B input-side $</th></tr></thead><tbody>{t}</tbody></table></details></section>')


def section_tokens():
    A, B = S["A"], S["B"]
    parts = [
        small_multiple("Cache read tokens", usage("A", "cache_read_input_tokens"), usage("B", "cache_read_input_tokens")),
        small_multiple("Cache write tokens", usage("A", "cache_creation_input_tokens"), usage("B", "cache_creation_input_tokens")),
        small_multiple("Uncached input tokens", usage("A", "input_tokens"), usage("B", "input_tokens")),
        small_multiple("Output tokens", usage("A", "output_tokens"), usage("B", "output_tokens")),
    ]
    mu = ""
    for arm in "AB":
        for model, m in ((S[arm].get("model_usage") or {}).items()):
            mu += (f'<tr><td><i class="sw sw-{arm}"></i>{arm}</td><td>{esc(model)}</td><td>{fmt_int(m.get("inputTokens"))}</td><td>{fmt_int(m.get("outputTokens"))}</td>'
                   f'<td>{fmt_int(m.get("cacheReadInputTokens"))}</td><td>{fmt_int(m.get("cacheCreationInputTokens"))}</td><td>{fmt_usd(m.get("costUSD"))}</td></tr>')
    return ('<section id="tokens"><h2>Where the tokens went</h2>'
            '<p>Each panel has its own scale. Cache reads dominate any long agent run; the scaffolding shows up as the extra context re-read on every call, not as extra generation.</p>'
            f'<div class="sm-grid">{"".join(parts)}</div>'
            f'{context_line_chart()}'
            f'<details><summary>Per-model usage as reported by Claude Code</summary><div class="scroll"><table class="t"><thead><tr><th>arm</th><th>model</th><th>input</th><th>output</th><th>cache read</th><th>cache write</th><th>cost</th></tr></thead><tbody>{mu}</tbody></table></div></details>'
            '</section>')


def section_tools():
    A, B = S["A"], S["B"]
    names = sorted(set((A.get("tool_calls") or {}).keys()) | set((B.get("tool_calls") or {}).keys()), key=lambda n: -max(n0((A.get("tool_calls") or {}).get(n)), n0((B.get("tool_calls") or {}).get(n))))
    rows = [(n, (A.get("tool_calls") or {}).get(n, 0), (B.get("tool_calls") or {}).get(n, 0)) for n in names]
    kinds = sorted(set((A.get("bash_kinds") or {}).keys()) | set((B.get("bash_kinds") or {}).keys()), key=lambda n: -max(n0((A.get("bash_kinds") or {}).get(n)), n0((B.get("bash_kinds") or {}).get(n))))
    krows = [(k, (A.get("bash_kinds") or {}).get(k, 0), (B.get("bash_kinds") or {}).get(k, 0)) for k in kinds]
    skills_a = ", ".join(esc(s) for s in (A.get("skills_invoked") or [])) or "none"
    skills_b = ", ".join(esc(s) for s in (B.get("skills_invoked") or [])) or "none"
    ag = ""
    for arm in "AB":
        for a in (S[arm].get("agent_spawns") or []):
            ag += f'<li><i class="sw sw-{arm}"></i>{arm}: <code>{esc(str(a.get("type")))}</code> — {esc(str(a.get("desc")))}</li>'
    errs = ""
    for arm in "AB":
        for e in (S[arm].get("error_samples") or [])[:6]:
            errs += f'<li><i class="sw sw-{arm}"></i>{arm} · <code>{esc(e["tool"])}</code>: <span class="mono">{esc(e["text"][:220])}</span></li>'
    edited = ""
    for arm in "AB":
        for p, c in (S[arm].get("files_edited") or {}).items():
            edited += f'<tr><td><i class="sw sw-{arm}"></i>{arm}</td><td class="mono">{esc(p.split("nebula-ab-test/", 1)[-1])}</td><td>{c}</td></tr>'
    return ('<section id="behaviour"><h2>How each arm worked</h2>'
            f'{paired_bars(rows, "Tool calls by tool")}'
            f'{paired_bars(krows, "Shell commands by kind")}'
            f'<div class="two"><div><h3>Skills invoked</h3><p><i class="sw sw-A"></i>A: {skills_a}</p><p><i class="sw sw-B"></i>B: {skills_b}</p>'
            f'<p class="note">Both arms share the user-level skills (claude-api, code-review, …); only the nine repo skills, the hooks and the CLAUDE.md/AGENTS.md/TERMS.md layer differ.</p></div>'
            f'<div><h3>Subagents spawned</h3>{("<ul class=plain>" + ag + "</ul>") if ag else "<p>none in either arm</p>"}'
            f'<h3>Guard-hook blocks · classifier denials</h3><p><i class="sw sw-A"></i>A: {n0(A.get("guard_hook_blocks"))} · {n0(A.get("denied_by_classifier"))}</p><p><i class="sw sw-B"></i>B: {n0(B.get("guard_hook_blocks"))} · {n0(B.get("denied_by_classifier"))}</p></div></div>'
            f'<details><summary>Tool errors the agents hit (first few)</summary><ul class="plain">{errs or "<li>none</li>"}</ul></details>'
            f'<details><summary>Files edited through Edit/Write (count of edits)</summary><div class="scroll"><table class="t"><thead><tr><th>arm</th><th>file</th><th>edits</th></tr></thead><tbody>{edited}</tbody></table></div></details>'
            '</section>')


def section_gates():
    def row(label, key, sub=lambda d: ""):
        cells = ""
        for arm in "AB":
            d = (AU[arm].get(key) or {})
            ok = d.get("ok") if d else None
            cells += f'<td>{status_pill(ok)} <small>{esc(sub(d))}</small></td>'
        return f"<tr><th>{esc(label)}</th>{cells}</tr>"

    t = ("<table class='t gates'><thead><tr><th></th><th><i class='sw sw-A'></i>A · with skills</th><th><i class='sw sw-B'></i>B · bare</th></tr></thead><tbody>"
         + row("rustfmt --check", "fmt", lambda d: f'{d.get("diff_lines", 0)} lines off' if d and not d.get("ok") else "")
         + row("clippy -D warnings", "clippy", lambda d: (f'{d.get("warnings", 0)} warnings' if d and not d.get("ok") else ""))
         + row("cargo build --all-targets", "build", lambda d: f'{d.get("seconds", "")}s' if d else "")
         + row("cargo test --workspace", "test", lambda d: (f'{d.get("passed", 0)} passed · {d.get("failed", 0)} failed' + (" · reran once for the cold-exec flake" if d.get("reran_for_cold_exec_flake") else "") + (" · compile error" if d.get("compile_error") else "")) if d and "passed" in d else (d.get("skipped", "") if d else ""))
         + "</tbody></table>")
    fails = ""
    for arm in "AB":
        names = (AU[arm].get("test") or {}).get("failed_names") or []
        if names:
            fails += f'<p><i class="sw sw-{arm}"></i>{arm} failing tests: <span class="mono">{esc(", ".join(names[:12]))}</span></p>'
        fe = (AU[arm].get("clippy") or {}).get("first_error")
        if fe and not (AU[arm].get("clippy") or {}).get("ok"):
            fails += f'<p><i class="sw sw-{arm}"></i>{arm} first clippy message: <span class="mono">{esc(fe[:200])}</span></p>'
    return f'<section id="gates"><h2>Did it build?</h2><p>Run by me after each arm finished, from the tree it left behind, in that arm\'s own clone.</p>{t}{fails}</section>'


CRITERIA = ["Spec coverage", "Transcript parsing", "Cost model", "Architecture fit", "Persistence & liveness", "Tests", "Code quality", "Diff economy"]


def section_review():
    if not REV:
        return '<section id="review"><h2>Blinded code review</h2><p>Not available.</p></section>'
    reviews = REV.get("reviews", [])  # each: {"reviewer", "mapping": {"X": "A", "Y": "B"}, "scores", "evidence", "bugs", "winner", "verdict"}

    def lab(r, arm):
        return {v: k for k, v in r.get("mapping", {}).items()}.get(arm)

    rows = ""
    tot = {"A": 0, "B": 0}
    for i, c in enumerate(CRITERIA, 1):
        cells = ""
        for arm in "AB":
            vals = []
            evs = []
            for r in reviews:
                l = lab(r, arm)
                if not l:
                    continue
                v = (r.get("scores", {}).get(l) or {}).get(str(i))
                if isinstance(v, (int, float)):
                    vals.append(v)
                evs.append(f'<li><b>{esc(r.get("reviewer", ""))}</b> ({v}): {esc(str((r.get("evidence", {}).get(l) or {}).get(str(i), "")))}</li>')
            avg = sum(vals) / len(vals) if vals else None
            if avg is not None:
                tot[arm] += avg
            cells += (f'<td><div class="score"><b>{"—" if avg is None else f"{avg:.1f}"}</b><span class="meter"><i class="fill-{arm}" style="width:{0 if avg is None else avg/5*100:.0f}%"></i></span></div>'
                      f'<details><summary>evidence</summary><ul class="plain small">{"".join(evs)}</ul></details></td>')
        rows += f"<tr><th>{i}. {esc(c)}</th>{cells}</tr>"
    winners = [r.get("mapping", {}).get(r.get("winner"), r.get("winner")) for r in reviews]
    bugs = ""
    for arm in "AB":
        items = []
        for r in reviews:
            l = lab(r, arm)
            items += [(r.get("reviewer", ""), b) for b in ((r.get("bugs", {}) or {}).get(l) or [])] if l else []
        if items:
            bugs += f'<div><h3><i class="sw sw-{arm}"></i>{esc(ARM_NAME[arm])} — issues the reviewers found</h3><ul>{"".join(f"<li><b>{esc(who)}</b>: {esc(str(b))}</li>" for who, b in items)}</ul></div>'
    verdicts = "".join(f'<blockquote><b>{esc(r.get("reviewer", "reviewer"))}</b> (saw {esc(ARM_NAME.get(r.get("mapping", {}).get("X"), "?"))} as X) picked <b>{esc(str(r.get("mapping", {}).get(r.get("winner"), r.get("winner"))))}</b>: {esc(str(r.get("verdict", "")))}</blockquote>' for r in reviews)
    return ('<section id="review"><h2>Blinded code review</h2>'
            f'<p>{len(reviews)} independent reviewers (fresh Fable 5.1 sessions with no knowledge of the experiment) scored the two code diffs labelled X and Y against an eight-point rubric, with the labels swapped between reviewers to cancel label-order bias. Scores are 1–5 per criterion; the table shows the average, and each cell opens to the reviewers\' evidence.</p>'
            f'<div class="scroll"><table class="t review"><thead><tr><th></th><th><i class="sw sw-A"></i>A · with skills</th><th><i class="sw sw-B"></i>B · bare</th></tr></thead><tbody>{rows}'
            f'<tr class="total"><th>Total / 40</th><td><b>{tot["A"]:.1f}</b></td><td><b>{tot["B"]:.1f}</b></td></tr></tbody></table></div>'
            f'<p>Reviewer picks: <b>{esc(", ".join(str(w) for w in winners) or "—")}</b></p>{verdicts}<div class="two">{bugs}</div></section>')


def section_diffs():
    out = ""
    for arm in "AB":
        files = AU[arm].get("files") or []
        rows = ""
        for f in sorted(files, key=lambda f: (f["scaffold"], f["path"])):
            cls = ' class="dim"' if f["scaffold"] else ""
            new = " <span class='tag'>new</span>" if f["path"] in (AU[arm].get("new_files") or []) else ""
            rows += f'<tr{cls}><td class="mono">{esc(f["path"])}{new}</td><td class="num">+{f["added"]}</td><td class="num">−{f["deleted"]}</td></tr>'
        cp = os.path.join(OUT, f"{arm.lower()}.closing.txt")
        summ = open(cp).read() if os.path.exists(cp) else (S[arm].get("final_text") or "")
        out += (f'<div class="arm-block"><h3><i class="sw sw-{arm}"></i>{esc(ARM_LONG[arm])}</h3>'
                f'<p class="note">{n0(AU[arm].get("code_files"))} code/doc files, +{fmt_int(AU[arm].get("code_added"))}/−{fmt_int(AU[arm].get("code_deleted"))}'
                + (f' · plus {n0(AU[arm].get("scaffold_files"))} loop files (memory entries, TERMS, index) +{fmt_int(AU[arm].get("scaffold_added"))}/−{fmt_int(AU[arm].get("scaffold_deleted"))}, dimmed below' if AU[arm].get("scaffold_files") else "")
                + f' · <a href="file://{esc(OUT)}/{arm.lower()}.code.diff">code diff</a> · <a href="file://{esc(OUT)}/{arm.lower()}.full.diff">full diff</a> · clone: <span class="mono">{esc(os.path.join(AB, "a-with-skills" if arm == "A" else "b-bare"))}</span></p>'
                f'<details><summary>The agent\'s own closing summary</summary><div class="summary-text">{esc(summ)}</div></details>'
                f'<details><summary>Files changed ({len(files)})</summary><div class="scroll"><table class="t files"><thead><tr><th>file</th><th>+</th><th>−</th></tr></thead><tbody>{rows}</tbody></table></div></details></div>')
    return f'<section id="built"><h2>What each arm built</h2>{out}</section>'


def section_method():
    A, B = S["A"], S["B"]
    return ('<section id="method"><h2>Method and caveats</h2>'
            '<ul>'
            '<li><b>Two shallow clones</b> of <span class="mono">main</span> at <span class="mono">ec7c200</span> (depth 1, so neither arm could mine git history). Arm B additionally had <span class="mono">.claude/</span> (skills, hooks, memory, rules, settings), CLAUDE.md, AGENTS.md, TERMS.md and the Cursor rule deleted, and the Makefile\'s memory/terms gates removed from <span class="mono">make ci</span>, committed before the run so both trees started clean.</li>'
            '<li><b>Same prompt, same flags.</b> Both ran <span class="mono">claude -p</span> with <span class="mono">--model claude-fable-5-1</span>, default effort, <span class="mono">--permission-mode auto</span> (the same classifier-backed mode your own sessions use), Claude Code '
            f'{esc(str(A.get("claude_code_version") or B.get("claude_code_version") or ""))}. Cargo build and clippy artifacts were pre-warmed in both clones so neither paid a cold build.</li>'
            '<li><b>Ran in parallel</b> on the same machine, so wall time includes some CPU contention from each other\'s cargo runs; API time is the cleaner effort measure.</li>'
            '<li><b>One run each (n = 1).</b> Agent runs vary; a difference under roughly 20–30% on cost or time is inside the noise of a single sample. The quality read (gates + blinded review) is the sturdier signal.</li>'
            '<li><b>User-level config was constant</b> in both arms: your global skills (fleet, skill-forge, claude-api, code-review, …), your global settings and hooks. Only the repository layer was varied.</li>'
            '<li><b>The feature was chosen to give the memory layer a fair shot</b>: it touches the protocol, the store, the daemon\'s hook path and the TUI overlays, all of which the MEMORY LOG carries gotchas for.</li>'
            '</ul>'
            f'<details><summary>The feature prompt both arms received</summary><pre class="prompt">{esc(PROMPT)}</pre></details>'
            '</section>')


def section_verdict():
    A, B = S["A"], S["B"]
    head = NAR.get("headline") or "Results"
    lead = NAR.get("lead") or ""
    findings = NAR.get("findings") or []
    rec = NAR.get("recommendation") or ""
    tone = NAR.get("tone", "tie")
    return (f'<section id="verdict" class="verdict verdict-{esc(tone)}"><div class="eyebrow">Verdict</div><h2>{esc(head)}</h2>'
            f'<p class="lead">{lead}</p><ul class="findings">{"".join(f"<li>{f}</li>" for f in findings)}</ul>'
            + (f'<div class="rec"><div class="eyebrow">Recommendation</div><p>{rec}</p></div>' if rec else "") + '</section>')


CSS = r"""
:root{
  --plane:#f9f9f7; --surface:#fcfcfb; --ink:#0b0b0b; --ink2:#52514e; --muted:#898781; --grid:#e1e0d9; --axis:#c3c2b7; --ring:rgba(11,11,11,.10);
  --A:#2a78d6; --B:#eb6834; --good:#0ca30c; --warn:#fab219; --crit:#d03b3b; --good-text:#006300;
  --wash:#f1f0ec; --sel:#eef4fc;
  color-scheme:light;
}
@media (prefers-color-scheme: dark){ :root:not([data-theme="light"]){
  --plane:#0d0d0d; --surface:#1a1a19; --ink:#ffffff; --ink2:#c3c2b7; --muted:#898781; --grid:#2c2c2a; --axis:#383835; --ring:rgba(255,255,255,.10);
  --A:#3987e5; --B:#d95926; --good-text:#0ca30c; --wash:#232321; --sel:#1c2431; color-scheme:dark; } }
:root[data-theme="dark"]{
  --plane:#0d0d0d; --surface:#1a1a19; --ink:#ffffff; --ink2:#c3c2b7; --muted:#898781; --grid:#2c2c2a; --axis:#383835; --ring:rgba(255,255,255,.10);
  --A:#3987e5; --B:#d95926; --good-text:#0ca30c; --wash:#232321; --sel:#1c2431; color-scheme:dark; }
*{box-sizing:border-box}
body{margin:0;background:var(--plane);color:var(--ink);font:15px/1.55 "IBM Plex Sans",system-ui,-apple-system,"Segoe UI",sans-serif;}
main{max-width:1080px;margin:0 auto;padding:40px 28px 80px}
h1,h2{font-family:"IBM Plex Serif",Georgia,"Times New Roman",serif;font-weight:600;text-wrap:balance;letter-spacing:-.01em}
h1{font-size:36px;line-height:1.15;margin:0 0 6px}
h2{font-size:24px;margin:0 0 12px}
h3{font-size:15px;margin:18px 0 6px;font-weight:600}
p{max-width:68ch}
section{margin:44px 0;padding-top:28px;border-top:1px solid var(--grid)}
.eyebrow{font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--muted);margin-bottom:6px}
.sub{color:var(--ink2);margin:0;max-width:72ch}
.meta{display:flex;flex-wrap:wrap;gap:8px 20px;margin-top:14px;color:var(--ink2);font-size:13px}
.mono,code,pre,.num{font-family:"IBM Plex Mono",ui-monospace,SFMono-Regular,Menlo,monospace;font-variant-numeric:tabular-nums}
code{font-size:.9em}
.sw{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:6px;vertical-align:baseline}
.sw-A{background:var(--A)} .sw-B{background:var(--B)}
.bar-A{fill:var(--A)} .bar-B{fill:var(--B)}
.line-A{fill:none;stroke:var(--A);stroke-width:2;stroke-linejoin:round;stroke-linecap:round}
.line-B{fill:none;stroke:var(--B);stroke-width:2;stroke-linejoin:round;stroke-linecap:round}
.dot-A{fill:var(--A);stroke:var(--surface);stroke-width:2} .dot-B{fill:var(--B);stroke:var(--surface);stroke-width:2}
.fill-A{background:var(--A)} .fill-B{background:var(--B)}
.baseline{stroke:var(--axis);stroke-width:1} .grid{stroke:var(--grid);stroke-width:1}
svg text{fill:var(--ink2);font:12px "IBM Plex Sans",system-ui,sans-serif}
svg .val{font:12px "IBM Plex Mono",ui-monospace,monospace;fill:var(--ink)} svg .val.small{font-size:11px}
svg .lab{fill:var(--ink)} svg .tick{fill:var(--muted);font-size:11px} svg .endlab{fill:var(--ink);font-weight:600;font-size:12px}
svg rect,svg circle{cursor:default}
/* verdict */
.verdict{border-top:0;background:var(--surface);border:1px solid var(--grid);border-left:4px solid var(--A);padding:24px 28px;margin-top:28px}
.verdict-B{border-left-color:var(--B)} .verdict-tie{border-left-color:var(--muted)}
.verdict h2{font-size:26px} .lead{font-size:17px;color:var(--ink);max-width:72ch}
.findings{padding-left:20px;max-width:80ch} .findings li{margin:6px 0}
.rec{margin-top:18px;padding-top:14px;border-top:1px solid var(--grid)} .rec p{margin:0}
/* kpis */
.kpis{display:grid;grid-template-columns:repeat(3,1fr);gap:12px} @media (max-width:720px){.kpis{grid-template-columns:1fr 1fr}}
.kpi{background:var(--surface);border:1px solid var(--grid);padding:12px 14px 10px;display:flex;flex-direction:column;gap:6px}
.kpi-label{font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--muted)}
.kpi-pair{display:grid;grid-template-columns:1fr 1fr;gap:8px}
.kpi-v b{display:block;font-size:22px;font-weight:600;font-variant-numeric:tabular-nums;line-height:1.1}
.kpi-v small{display:block;color:var(--muted);font-size:11px;margin-top:2px;min-height:14px}
.kpi-chip{min-height:20px}
.chip{display:inline-block;font-size:11px;padding:2px 7px;border-radius:999px;background:var(--wash);color:var(--ink2);font-variant-numeric:tabular-nums}
.chip-good{color:var(--good-text)} .chip-bad{color:var(--crit)} .chip-flat{color:var(--muted)}
/* small multiples */
.sm-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:14px;margin:14px 0 22px}
figure{margin:0} figure.sm{background:var(--surface);border:1px solid var(--grid);padding:10px 12px}
figcaption{font-size:13px;font-weight:600;margin-bottom:6px;display:flex;flex-wrap:wrap;gap:8px;align-items:center}
figcaption .note{font-weight:400;color:var(--muted)}
figure.chart{background:var(--surface);border:1px solid var(--grid);padding:14px 16px;margin:16px 0}
.legend{display:flex;gap:16px;font-size:12px;color:var(--ink2);margin:2px 0 8px}
.scroll{overflow-x:auto}
details{margin-top:8px} summary{cursor:pointer;color:var(--ink2);font-size:13px}
table.t{border-collapse:collapse;width:100%;font-size:13px;margin-top:8px}
table.t th,table.t td{text-align:left;padding:6px 10px;border-bottom:1px solid var(--grid);vertical-align:top}
table.t thead th{color:var(--muted);font-weight:500;font-size:12px;letter-spacing:.04em}
table.t td.num{text-align:right} tr.dim td{color:var(--muted)}
table.gates th{width:220px} table.review th{width:200px} table.review td{width:40%}
.pill{display:inline-block;font-size:12px;padding:2px 8px;border-radius:999px;background:var(--wash);white-space:nowrap}
.pill-good{color:var(--good-text)} .pill-bad{color:var(--crit)} .pill-unknown{color:var(--muted)}
.tag{font-size:10px;letter-spacing:.08em;text-transform:uppercase;background:var(--wash);padding:1px 6px;border-radius:3px;color:var(--ink2);margin-left:6px}
.score{display:flex;align-items:center;gap:10px} .score b{font-size:18px;min-width:34px;font-variant-numeric:tabular-nums}
.meter{flex:1;height:6px;background:var(--wash);border-radius:3px;overflow:hidden;max-width:180px} .meter i{display:block;height:100%}
.two{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:24px}
ul.plain{list-style:none;padding:0;margin:6px 0} ul.plain li{margin:4px 0} ul.small{font-size:13px;color:var(--ink2)}
blockquote{margin:12px 0;padding:10px 14px;border-left:3px solid var(--grid);background:var(--surface);color:var(--ink2);max-width:80ch}
.arm-block{margin:18px 0 26px} .summary-text{white-space:pre-wrap;font-size:13px;color:var(--ink2);max-width:90ch;margin-top:8px}
pre.prompt{white-space:pre-wrap;font-size:12.5px;background:var(--surface);border:1px solid var(--grid);padding:14px;max-width:none}
a{color:var(--A)} a:focus-visible,summary:focus-visible{outline:2px solid var(--A);outline-offset:2px}
#tip{position:fixed;pointer-events:none;background:var(--ink);color:var(--plane);font:12px "IBM Plex Sans",system-ui,sans-serif;padding:6px 9px;border-radius:4px;opacity:0;transition:opacity .08s;max-width:320px;z-index:9}
@media (prefers-reduced-motion:reduce){#tip{transition:none}}
nav.toc{display:flex;flex-wrap:wrap;gap:6px 18px;font-size:13px;margin-top:18px} nav.toc a{color:var(--ink2);text-decoration:none;border-bottom:1px solid var(--grid)}
"""

JS = r"""
(function(){var t=document.getElementById('tip');function show(e){var el=e.target.closest('[data-tip]');if(!el){t.style.opacity=0;return;}t.textContent=el.getAttribute('data-tip');t.style.opacity=1;move(e);}function move(e){var x=e.clientX+14,y=e.clientY+14;var r=t.getBoundingClientRect();if(x+r.width>window.innerWidth-8)x=e.clientX-r.width-10;if(y+r.height>window.innerHeight-8)y=e.clientY-r.height-10;t.style.left=x+'px';t.style.top=y+'px';}
document.addEventListener('mouseover',show);document.addEventListener('mousemove',function(e){if(t.style.opacity==='1')move(e);});document.addEventListener('mouseout',function(e){if(!e.relatedTarget||!e.relatedTarget.closest||!e.relatedTarget.closest('[data-tip]'))t.style.opacity=0;});})();
"""


def main():
    A, B = S["A"], S["B"]
    title = NAR.get("title") or "Skills A/B Verdict"
    page = f"""<title>{esc(title)}</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Serif:wght@600&family=IBM+Plex+Sans:wght@400;600&family=IBM+Plex+Mono&display=swap">
<style>{CSS}</style>
<main>
<div class="eyebrow">Experiment · {esc(NAR.get("date", ""))}</div>
<h1>{esc(NAR.get("h1") or "Do the repo skills earn their tokens?")}</h1>
<p class="sub">{NAR.get("sub") or "One feature, two identical shallow clones of nebula, one Fable 5.1 headless run each: arm A with the full SELF-IMPROVING LOOP scaffolding (skills, hooks, CLAUDE.md, AGENTS.md, TERMS.md, MEMORY LOG), arm B with all of it deleted."}</p>
<div class="meta"><span><i class="sw sw-A"></i>{esc(ARM_LONG["A"])}</span><span><i class="sw sw-B"></i>{esc(ARM_LONG["B"])}</span></div>
<nav class="toc"><a href="#verdict">Verdict</a><a href="#numbers">Numbers</a><a href="#phases">Phases</a><a href="#tokens">Tokens</a><a href="#behaviour">Behaviour</a><a href="#gates">Gates</a><a href="#review">Blinded review</a><a href="#built">What was built</a><a href="#method">Method</a></nav>
{section_verdict()}
{section_kpis()}
{section_phases()}
{section_tokens()}
{section_tools()}
{section_gates()}
{section_review()}
{section_diffs()}
{section_method()}
</main>
<div id="tip" role="tooltip"></div>
<script>{JS}</script>
"""
    open(os.path.join(OUT, "report.html"), "w").write(page)
    print(os.path.join(OUT, "report.html"), len(page), "bytes")


if __name__ == "__main__":
    main()
