# Crash Logging — 2026-08-19

**Asked:** "make sure all errors in nebula are logged into a .log file somewhere so that I can debug when
it crashes. so far i've seen nebula randomly close out and crash twice now when trying to create a new
claude session, but I'm not sure how to debug"

**Did:** `71e62c7` — panic logging for both the TUI and the daemon.

**Gotchas:**
- Worth knowing that the "random crashes on new claude session" the user was chasing here were most likely
  the two separate problems diagnosed the next day: the stale code signature and the controlling-terminal
  fight. Crash logging is what made both findable.
