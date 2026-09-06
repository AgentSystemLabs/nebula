# Removed Global NEBULA And Kept The DEV INSTANCE - 2026-09-06

**Asked:** "Please remove the glboal nebula instealled that I have to avoid confusion, lets keep only dev"
→ refined: Remove the globally installed NEBULA executable so this machine uses only the development build. Preserve the DEV INSTANCE, its SESSIONS and data, and the repository’s build outputs. Verify that a fresh shell no longer resolves the global executable.

**Did:** Found the only installed copy at `~/.local/bin/nebula` (0.21.0), matching INSTALL.SH's default destination, with no Cargo registration or additional common-prefix installation. Its DAEMON, PID 99284, used `/tmp/nebula-501` and had no descendant processes. Stopped it through the installed binary with explicit global runtime/data paths, then removed that executable. The active DEV INSTANCE retained PID 44951 and `target/debug/nebula` remained version 0.22.0. Verified `command -v nebula` fails in a fresh interactive login shell, no process uses the removed installation, and both global and development databases still exist. Archived the oldest index line to keep MEMORY CHECK within its cap. No application code, shell configuration, DEV INSTANCE restart, or build changes.

**Gotchas:**
- An executable's location does not select its DAEMON. This SESSION inherits development `NEBULA_RUNTIME_DIR` / `NEBULA_DATA_DIR`; invoking the global binary with those values would target the DEV INSTANCE. Global shutdown explicitly used `/tmp/nebula-501` and the app-support DATA DIR, and checked the development PID before and after.
- The user's machine now intentionally has no bare `nebula` command on PATH. Use MAKE DEV to launch the per-checkout instance and the checkout's `target/debug/nebula` for CLI calls from its SESSION. Running that development binary from an ordinary terminal without the development environment would use the default runtime/data location. MANAGED WORKFLOW STARTING PROMPTS already use the DAEMON executable's absolute path.
