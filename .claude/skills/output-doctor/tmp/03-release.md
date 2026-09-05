---

## 📋 ==== OVERVIEW ====

v0.21.0 is out: QUICK PROMPT FOCUS, so `p` no longer enters and locks the TERMINAL PANE. Cut from a
RELEASE WORKTREE off `origin/main` with the CARRY SET brought over from the SHARED CHECKOUT; 819 tests
green there, the RELEASE WORKFLOW green on all four targets, the RELEASE NOTES written over the
auto-generated list. PROTOCOL VERSION stays 34, so no NEBULA KILL is needed. The SHARED CHECKOUT is
untouched and still 8 commits behind `origin/main`.

---

## 🔧 ==== DETAILS ====

**The CARRY SET split two ways** — the three TUI files and the README were strictly ahead of
`origin/main`, so a `cp` was right; the three MEMORY LOG files had moved on both sides, so they were
merged with `git diff v0.20.0 -- <paths> | git apply --3way` and two keep-both conflicts resolved
newest-first. `git diff HEAD` was the wrong base: it spans both of today's releases.

**The notes say what the code says** — the REFINED PROMPT called the SETTING "Focus new session"; the
shipped label is "Quick prompt focus" (`crates/nebula-tui/src/config.rs`), so the notes use that.

Logged the entry (six gotchas, the patch-base one folded into the carry STANDING GOTCHA); promoted
nothing.

---

## 👉 ==== NEXT STEPS ====

1. Take the build when you want it: `nebula upgrade` — with live SESSIONS the DAEMON keeps the old
   binary until NEBULA KILL, so run that from a terminal outside nebula.
