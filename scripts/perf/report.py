#!/usr/bin/env python3
"""Reads a LATENCY HARNESS run (`perf.jsonl` from the TUI's probe + `steps.tsv` from run.sh) and prints
one row per scripted step:

  handler   the longest any of the step's input handlers held the event loop — nothing else (no paint,
            no PTY output, no other key) happens while it runs
  paint     the longest any of the step's inputs waited from arrival to the end of the frame that
            showed it: what the user feels as "did the key register"
  settle    from the step's first input to the first frame that already shows what the step ends on
            (same overlay, same session in the pane, its screen painted, nothing still loading): what
            the user feels as "is it there yet"
  echo      in a locked pane: from the key to the end of the first frame after the PTY answered it —
            the full trip through the daemon and back
  frames    frames painted during the step, and the slowest draw among them

Above the table, `startup` is from the driver launching the binary to its first frame, and to the first
frame that had the daemon's snapshot in hand.

All in milliseconds. `report.py A B` prints two runs side by side (before / after).
"""
import json
import sys
from pathlib import Path


def load(run):
    run = Path(run)
    events, epoch = [], 0
    for line in (run / "perf.jsonl").read_text().splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue  # a line cut short by a kill
        if ev["k"] == "start":
            epoch = ev["epoch_us"]
        else:
            events.append(ev)
    steps = []
    for line in (run / "steps.tsv").read_text().splitlines():
        at, label, key = (line.split("\t") + ["", ""])[:3]
        steps.append((int(at) - epoch, label, key))
    return events, steps


def state(frame):
    return (frame["overlay"], frame["pane"], frame["painted"], frame["booting"], frame.get("busy", False))


def startup(run):
    """(launch → first frame, launch → first frame after the Snapshot), in µs."""
    events, steps = load(run)
    launch = next((at for at, label, _ in steps if label == "launch"), None)
    frames = [e for e in events if e["k"] == "frame"]
    if launch is None or not frames:
        return None, None
    first = frames[0]["t"] + frames[0]["draw_us"] - launch
    snap = next((e["t"] for e in events if e["k"] == "server" and e["ev"] == "Snapshot"), None)
    full = next((f["t"] + f["draw_us"] - launch for f in frames if snap is not None and f["t"] >= snap), None)
    return first, full


def memory(run):
    """Peak RSS in MB of (the TUI, the daemon) over the run, from run.sh's twice-a-second samples."""
    path = Path(run) / "rss.tsv"
    if not path.exists():
        return None, None
    tui, daemon = [], []
    for line in path.read_text().splitlines():
        a, _, b = line.partition("\t")
        if a.isdigit():
            tui.append(int(a))
        if b.isdigit():
            daemon.append(int(b))
    peak = lambda xs: max(xs) / 1024 if xs else None
    return peak(tui), peak(daemon)


def rows(run):
    events, steps = load(run)
    steps = [s for s in steps if s[1] != "launch"]
    out = []
    for (start, label, key), (end, _, _) in zip(steps, steps[1:]):
        mine = [e for e in events if start <= e["t"] < end]
        inputs = [e for e in mine if e["k"] == "input"]
        frames = [e for e in mine if e["k"] == "frame"]
        if not inputs:
            out.append((label, key, None, None, None, len(frames), 0, None))
            continue
        echo = None
        if inputs[-1]["focus"] == "Terminal" and len(inputs) == 1:
            t0 = inputs[0]["t"]
            answered = next((e["t"] for e in mine if e["k"] == "server" and e["ev"] == "Output" and e["t"] >= t0), None)
            if answered is not None:
                echo = next((f["t"] + f["draw_us"] - t0 for f in frames if f["t"] >= answered), None)
        handler = max(e["handler_us"] for e in inputs)
        # A button's release changes nothing on screen; the press is what the user is waiting on.
        paint = max((i["latency_us"] for f in frames for i in f["inputs"] if i["ev"] != "mouse:up"), default=None)
        first = inputs[0]["t"]
        after = [f for f in frames if f["t"] >= first]
        settle = None
        if after:
            final = state(after[-1])
            # The first frame from which the state never leaves `final` again.
            idx = len(after) - 1
            while idx > 0 and state(after[idx - 1]) == final:
                idx -= 1
            f = after[idx]
            settle = f["t"] + f["draw_us"] - first
        draw = max((f["draw_us"] for f in frames), default=0)
        out.append((label, key, handler, paint, settle, len(frames), draw, echo))
    return out


def ms(us):
    return "     -" if us is None else f"{us / 1000:6.1f}"


def main(argv):
    runs = [rows(a) for a in argv]
    for a in argv:
        first, full = startup(a)
        print(f"startup ({Path(a).name}): first frame {ms(first).strip()} ms, with the daemon's snapshot {ms(full).strip()} ms")
        tui, daemon = memory(a)
        if tui is not None:
            print(f"memory  ({Path(a).name}): peak RSS — TUI {tui:.1f} MB, daemon {daemon or 0:.1f} MB")
    if len(runs) == 1:
        print(f"{'step':<34}{'key':<14}{'handler':>8}{'paint':>8}{'settle':>8}{'echo':>8}{'frames':>8}{'max draw':>10}")
        for label, key, handler, paint, settle, n, draw, echo in runs[0]:
            print(f"{label:<34}{key[:12]:<14}{ms(handler):>8}{ms(paint):>8}{ms(settle):>8}{ms(echo):>8}{n:>8}{ms(draw):>10}")
        worst = [r for r in runs[0] if r[3] is not None]
        if worst:
            paints = sorted(r[3] for r in worst)
            print(f"\npaint  p50 {ms(paints[len(paints) // 2])} ms   max {ms(paints[-1])} ms   "
                  f"steps over 16 ms: {sum(p > 16000 for p in paints)}/{len(paints)}")
        return 0
    before, after = runs[0], runs[1]
    print(f"{'step':<34}{'key':<12}{'handler before':>15}{'after':>8}{'paint before':>14}{'after':>8}{'settle before':>15}{'after':>8}")
    after_by = {a[0]: a for a in after}
    for b in before:
        a = after_by.get(b[0])
        if a is None:
            continue
        print(f"{b[0]:<34}{b[1][:10]:<12}{ms(b[2]):>15}{ms(a[2]):>8}{ms(b[3]):>14}{ms(a[3]):>8}{ms(b[4]):>15}{ms(a[4]):>8}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
