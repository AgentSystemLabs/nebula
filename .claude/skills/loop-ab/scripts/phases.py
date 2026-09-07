#!/usr/bin/env python3
"""usage: phases.py <out-dir>

Split each arm's run into phases by API-call index and price the input side of each.
Fable 5.1 list prices per Mtok: input 10, output 50, cache read 0.25; Claude Code writes 1h cache (2x input)."""
import json, os, sys
OUT = sys.argv[1] if len(sys.argv) > 1 else "out"
IN, OUT_, CR, CW = 10.0, 50.0, 0.25, 20.0
for arm in 'ab':
    per = {}; skill_at = {}; first_edit = None
    for raw in open(os.path.join(OUT, f'{arm}.stream.jsonl')):
        try: d = json.loads(raw)
        except: continue
        if d.get('type') != 'assistant': continue
        m = d['message']; mid = m.get('id')
        if mid not in per: per[mid] = {'i': len(per) + 1, 'u': {}}
        u = m.get('usage') or {}
        rec = per[mid]['u']
        for k in ('input_tokens', 'cache_read_input_tokens', 'cache_creation_input_tokens'):
            rec[k] = max(rec.get(k, 0), int(u.get(k) or 0))
        for c in m.get('content', []):
            if c.get('type') != 'tool_use': continue
            i = per[mid]['i']
            if c['name'] == 'Skill': skill_at.setdefault(c['input'].get('skill'), i)
            if c['name'] in ('Write', 'Edit') and 'nebula-ab-test' in str(c['input'].get('file_path', '')) and first_edit is None: first_edit = i
            if c['name'] == 'Bash' and first_edit is None:
                cmd = c['input'].get('command', '')
                if 'sed -i' in cmd or ('<<' in cmd and ('python3' in cmd or 'cat >' in cmd or 'apply' in cmd)): first_edit = i
    calls = sorted(per.values(), key=lambda r: r['i'])
    n = len(calls)
    close = skill_at.get('nebula-memory')
    bounds = [('explore (before the first edit)', 1, (first_edit or n + 1) - 1), ('build + verify', first_edit or n + 1, (close - 1) if close else n)]
    if close: bounds.append(('close the loop (memory, terms, output shape, skill audit)', close, n))
    phases = []
    for name, lo, hi in bounds:
        sel = [r for r in calls if lo <= r['i'] <= hi]
        ctx = sum(r['u'].get('input_tokens', 0) + r['u'].get('cache_read_input_tokens', 0) + r['u'].get('cache_creation_input_tokens', 0) for r in sel)
        cost = sum(r['u'].get('input_tokens', 0) * IN + r['u'].get('cache_read_input_tokens', 0) * CR + r['u'].get('cache_creation_input_tokens', 0) * CW for r in sel) / 1e6
        phases.append({'name': name, 'calls': len(sel), 'from': lo, 'to': hi, 'context_tokens': ctx, 'input_side_cost_usd': round(cost, 2)})
    tot = {k: sum(r['u'].get(k, 0) for r in calls) for k in ('input_tokens', 'cache_read_input_tokens', 'cache_creation_input_tokens')}
    res = json.load(open(os.path.join(OUT, f'{arm}.summary.json')))
    ru = res['usage']
    first_ctx = calls[0]['u']; first_ctx = first_ctx.get('input_tokens', 0) + first_ctx.get('cache_read_input_tokens', 0) + first_ctx.get('cache_creation_input_tokens', 0)
    out = {'phases': phases, 'first_call_context': first_ctx, 'skill_at': skill_at, 'first_edit_call': first_edit, 'api_calls': n,
           'stream_vs_result': {k: (tot[k], ru.get(k)) for k in tot},
           'input_side_cost_usd': round(sum(p['input_side_cost_usd'] for p in phases), 2), 'output_cost_usd': round(ru['output_tokens'] * OUT_ / 1e6, 2), 'reported_cost_usd': res['total_cost_usd']}
    json.dump(out, open(os.path.join(OUT, f'{arm}.phases.json'), 'w'), indent=1)
    print(arm, json.dumps({k: v for k, v in out.items() if k != 'skill_at'}, indent=None))
