---

## 💬 ==== YOU ASKED ====

"Several SESSIONS in the SESSIONS PANEL are still named `agent-1`, `agent-2`, `agent-3` after I've
sent Claude a prompt in them, so AUTO-TITLE didn't retitle them. Find out why and tell me before
changing anything — is AUTO-TITLE broken, or is something else (VERSION SKEW, the hook) blocking the
`nebula rename` it runs? (Assuming this is the DEV INSTANCE started with `make dev` at 20:19 today.)"

---

## 📋 ==== OVERVIEW ====

No code changed. AUTO-TITLE is intact; this is VERSION SKEW. The DEV INSTANCE daemon started at 20:19
speaks PROTOCOL VERSION 32, and at 21:09 another session's MAKE INSTALL replaced the `nebula` on PATH
with a v33 build, so every `nebula rename` the agents have run since is refused by the handshake — and
the three untitled rows are exactly the ones created after 21:09. The SQLITE STORE agrees: all three
still carry the pending flag.

---

## 🔧 ==== DETAILS ====

**The tell is the row set, not a log** — the DAEMON LOG records nothing for a refused handshake (the
client refuses before sending); `select name, auto_title_pending, created_at from agents` against the
install's mtime dates the break.

**Why a DEV INSTANCE is exposed** — its daemon runs from `current_exe()` and stays put, but AUTO-TITLE's
bare `nebula rename` follows whatever PATH holds now, so a sibling session's MAKE INSTALL silently
untitles every SESSION this instance starts afterwards.

**Proposed enforcement** — route `nebula rename` over the unversioned HOOK RECEIVER (the AGENT ENV
already carries the BEARER TOKEN), so VERSION SKEW can never break it again.

Logged the entry (three gotchas); promoted nothing.

---

## 👉 ==== NEXT STEPS ====

1. Cut the DEV INSTANCE over so both binaries are v34: quit the dev TUI (that kills its seven live
   SESSIONS), then `make install && make dev`. The pending rows self-title on their next prompt, or
   `r` renames them now.
2. Decide on the enforcement — `nebula rename` over the HOOK RECEIVER, or leave it on the socket. Say
   which and I'll do it.
