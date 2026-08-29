# nebula ssh And Remote Hosts — 2026-08-19 → 08-21

**Asked:** "add a way for someone to launch nebula from the cli into a remote ssh. assume ssh keys already
allow access to the remachine. so something like nebula ssh HOST and when we get into the machine it
should install nebula if it doesn't already exist on the machine (remote exec of a script)…" Later: "add a
built in way so that nebula remembers the hosts you've recently done `nebula ssh` with so that a user can
press h to view all the hosts…"

**Did:** `8ddad36` (remote hosts, user config with settings overlay, fuzzy diff filtering) and the host
picker in `4bea626`.

**Gotchas:**
- The user also had to enable inbound ssh on this laptop to test it, and explicitly asked to confirm it
  was **local-network only, nothing from the public internet**. Don't widen that.
