#!/usr/bin/env python3
"""GUARD HOOK — a Claude Code `PreToolUse` hook on the Bash tool.

Each rule below is a MEMORY LOG gotcha that kept re-hitting, turned into something the harness enforces
instead of something an agent has to remember. A match blocks the command (exit 2) and feeds the reason
back to the agent, which then does it the right way. A rule that lands here should be deleted from
`.claude/memory/gotchas.md` — an enforced gotcha is no longer a gotcha.

Rules match the *argument* that is wrong, not any mention of the pattern, and heredoc bodies are
stripped before matching: a file written through `cat <<'EOF'` that quotes "cargo install --path" is
text, not a command. (Without that, the hook blocks the agent editing this very file, or a TERMS.md
row that describes it.) Project hooks take effect immediately in every running session — no restart.

Registered in `.claude/settings.json`. Try one by hand:
    echo '{"tool_name":"Bash","tool_input":{"command":"make install"}}' | python3 .claude/hooks/guard.py; echo $?
"""
import json
import re
import sys

# `<<EOF` / `<<'EOF'` / `<<-EOF` … up to the terminator on its own line.
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n.*?\n\s*\2\s*(?=\n|$)", re.S)
# A double-quoted or bare `-m` argument on one line that contains a backtick (single quotes are literal in zsh).
COMMIT_MSG_WITH_BACKTICK = re.compile(
    r"\bgit\s+commit\b[^|;&\n]*?\s-(?:a?m|-message)(?:\s+|=)(?:\"[^\"\n]*`[^\"\n]*\"|[^\s\"']*`)"
)
CARGO_INSTALL_PATH = re.compile(r"\bcargo\s+install\b[^|;&\n]*--path\b")
INSTALLED_BIN = r"(?:~|\$HOME|/Users/[\w.-]+)/\.cargo/bin/nebula(?=\s|$|['\"])"
COPY_OVER_INSTALLED_BIN = re.compile(r"\b(?:cp|install)\b[^|;&\n]*" + INSTALLED_BIN)
REDIRECT_OVER_INSTALLED_BIN = re.compile(r">\s*" + INSTALLED_BIN)
# `for f in $(…)` — zsh does not word-split an unquoted expansion, so the loop runs once over one giant "filename".
FOR_IN_COMMAND_SUBSTITUTION = re.compile(r"\bfor\s+\w+\s+in\s+\$\(")

RULES = [
    (
        "for-in-unquoted-command-substitution",
        lambda cmd: FOR_IN_COMMAND_SUBSTITUTION.search(cmd) is not None,
        "Blocked: the harness shell is zsh, which does not word-split an unquoted `$(…)`, so `for f in $(git diff "
        "--name-only)` runs once with every path glued into one filename and silently copies or checks nothing "
        "(MEMORY gotcha 2026-08-26, re-hit 2026-08-28). Pipe instead: `… | while IFS= read -r f; do …; done`.",
    ),
    (
        "backticks-in-commit-message",
        lambda cmd: COMMIT_MSG_WITH_BACKTICK.search(cmd) is not None,
        "Blocked: zsh command-substitutes backticks inside a double-quoted commit message and the commit fails "
        "with a parse error (MEMORY gotcha 2026-08-27). Write the message to a file in the scratchpad and run "
        "`git commit -F <file>`.",
    ),
    (
        "cargo-install-rewrites-in-place",
        lambda cmd: CARGO_INSTALL_PATH.search(cmd) is not None,
        "Blocked: installing with `--path` rewrites `~/.cargo/bin/nebula` on the same inode and macOS SIGKILLs the "
        "next exec with a stale code-signature cache (MEMORY gotcha 2026-08-20). Run `make install` instead.",
    ),
    (
        "in-place-overwrite-of-installed-binary",
        lambda cmd: COPY_OVER_INSTALLED_BIN.search(cmd) is not None or REDIRECT_OVER_INSTALLED_BIN.search(cmd) is not None,
        "Blocked: overwriting `~/.cargo/bin/nebula` in place leaves macOS's code-signature cache stale and the "
        "next exec is SIGKILLed (`zsh: killed`). Run `make install` — it copies to a fresh inode and `mv`s it "
        "into place (MEMORY gotcha 2026-08-20).",
    ),
]


def strip_heredocs(cmd):
    return HEREDOC.sub("<<HEREDOC-BODY>>", cmd)


def main():
    try:
        data = json.load(sys.stdin)
    except Exception:
        return 0
    if data.get("tool_name") != "Bash":
        return 0
    cmd = strip_heredocs((data.get("tool_input") or {}).get("command") or "")
    for name, pred, reason in RULES:
        try:
            if pred(cmd):
                sys.stderr.write("[nebula guard:%s] %s\n" % (name, reason))
                return 2
        except Exception:
            continue
    return 0


if __name__ == "__main__":
    sys.exit(main())
