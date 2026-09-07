#!/usr/bin/env python3
"""Post-run audit of one arm: diff stats, fmt, clippy, tests.

usage: audit_arm.py <arm-dir> <label> <out-dir>
Writes <out>/<label>.full.diff, <label>.code.diff, <label>.audit.json and the cargo logs.
"""
import json
import os
import re
import subprocess
import sys
import time

arm, label, out = sys.argv[1], sys.argv[2], sys.argv[3]
SCAFFOLD = re.compile(r"^(\.claude/|TERMS\.md$|CLAUDE\.md$|AGENTS\.md$|\.cursor/)")


def run(cmd, log=None, timeout=3600):
    t0 = time.time()
    p = subprocess.run(cmd, cwd=arm, shell=True, capture_output=True, text=True, timeout=timeout)
    text = p.stdout + p.stderr
    if log:
        open(os.path.join(out, log), "w").write(text)
    return p.returncode, text, round(time.time() - t0, 1)


audit = {"label": label}

# --- the diff: stage everything the agent left (scratch clone, so staging is harmless)
run("git add -A")
rc, numstat, _ = run("git diff --cached --numstat HEAD")
files = []
for line in numstat.splitlines():
    parts = line.split("\t")
    if len(parts) != 3:
        continue
    add, dele, path = parts
    add = 0 if add == "-" else int(add)
    dele = 0 if dele == "-" else int(dele)
    files.append({"path": path, "added": add, "deleted": dele, "scaffold": bool(SCAFFOLD.match(path)),
                  "kind": "test" if ("/tests/" in path or path.endswith("_test.rs")) else ("rust" if path.endswith(".rs") else ("doc" if path.endswith(".md") else "other"))})
audit["files"] = files
code = [f for f in files if not f["scaffold"]]
audit["code_files"] = len(code)
audit["code_added"] = sum(f["added"] for f in code)
audit["code_deleted"] = sum(f["deleted"] for f in code)
audit["scaffold_files"] = len(files) - len(code)
audit["scaffold_added"] = sum(f["added"] for f in files if f["scaffold"])
audit["scaffold_deleted"] = sum(f["deleted"] for f in files if f["scaffold"])
audit["new_files"] = [f["path"] for f in files if f["deleted"] == 0 and subprocess.run(["git", "cat-file", "-e", f"HEAD:{f['path']}"], cwd=arm, capture_output=True).returncode != 0]

rc, full, _ = run("git diff --cached HEAD")
open(os.path.join(out, f"{label}.full.diff"), "w").write(full)
excl = " ".join(f"':!{p}'" for p in (".claude", "TERMS.md", "CLAUDE.md", "AGENTS.md", ".cursor"))
rc, codediff, _ = run(f"git diff --cached HEAD -- . {excl}")
open(os.path.join(out, f"{label}.code.diff"), "w").write(codediff)
audit["new_test_fns"] = len(re.findall(r"^\+\s*#\[(?:tokio::)?test\]", codediff, re.M))
audit["rust_lines_added"] = sum(f["added"] for f in code if f["kind"] in ("rust", "test"))
run("git reset -q")  # leave the tree as the agent left it

# --- gates
rc, txt, secs = run("cargo fmt --all -- --check", f"{label}.fmt.log")
audit["fmt"] = {"ok": rc == 0, "seconds": secs, "diff_lines": len([l for l in txt.splitlines() if l.startswith(("+", "-")) and not l.startswith(("+++", "---"))])}

rc, txt, secs = run("cargo clippy --workspace --all-targets -- -D warnings", f"{label}.clippy.log")
audit["clippy"] = {"ok": rc == 0, "seconds": secs, "warnings": len(re.findall(r"^(warning|error)(\[|:)", txt, re.M)), "first_error": next((l for l in txt.splitlines() if l.startswith(("error", "warning")) and "generated" not in l), None)}

rc, txt, secs = run("cargo build --workspace --all-targets", f"{label}.build.log")
audit["build"] = {"ok": rc == 0, "seconds": secs}


def parse_tests(txt):
    passed = failed = ignored = 0
    for m in re.finditer(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", txt):
        passed += int(m.group(2)); failed += int(m.group(3)); ignored += int(m.group(4))
    failed_names = re.findall(r"^test (\S+) \.\.\. FAILED", txt, re.M)
    return passed, failed, ignored, failed_names


if audit["build"]["ok"]:
    rc, txt, secs = run("cargo test --workspace", f"{label}.test.log", timeout=3600)
    passed, failed, ignored, names = parse_tests(txt)
    reran = False
    if failed and "daemon socket never appeared" in txt:
        reran = True
        rc, txt, secs2 = run("cargo test --workspace", f"{label}.test.rerun.log", timeout=3600)
        passed, failed, ignored, names = parse_tests(txt)
        secs += secs2
    audit["test"] = {"ok": rc == 0, "seconds": secs, "passed": passed, "failed": failed, "ignored": ignored, "failed_names": names[:30], "reran_for_cold_exec_flake": reran, "compile_error": "could not compile" in txt}
else:
    audit["test"] = {"ok": False, "skipped": "build failed"}

# stray daemons the suite may have left
rc, txt, _ = run("pgrep -fl 'nebula daemon --foreground' | wc -l")
audit["stray_daemons_after"] = int(txt.strip() or 0)

json.dump(audit, open(os.path.join(out, f"{label}.audit.json"), "w"), indent=1)
print(json.dumps({k: v for k, v in audit.items() if k != "files"}, indent=1))
