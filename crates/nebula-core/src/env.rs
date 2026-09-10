//! Names of the environment variables nebula reads and sets, so the daemon,
//! the TUI, the CLI, the hook installers and the e2e tests all spell them
//! from one place — a typo here fails to build instead of silently falling
//! back to a default.

use std::path::PathBuf;

/// Id of the agent a hook or CLI invocation is running inside. Set on every
/// agent PTY, scrubbed from plain terminals.
pub const AGENT_ID: &str = "NEBULA_AGENT_ID";
/// Base URL of the daemon's hook receiver, set on agent PTYs.
pub const API_URL: &str = "NEBULA_API_URL";
/// Bearer token the hook receiver expects, set on agent PTYs.
pub const API_TOKEN: &str = "NEBULA_API_TOKEN";
/// Overrides the runtime dir holding the socket and pidfile.
pub const RUNTIME_DIR: &str = "NEBULA_RUNTIME_DIR";
/// Overrides the data dir holding the database, config and logs.
pub const DATA_DIR: &str = "NEBULA_DATA_DIR";
/// Replaces every agent CLI with one command line, taken verbatim (tests
/// stand in `/bin/sh` or a stub script for `claude`).
pub const AGENT_CMD: &str = "NEBULA_AGENT_CMD";
/// Idle-session reaper sweep period in ms; tests shorten it.
pub const IDLE_REAP_MS: &str = "NEBULA_IDLE_REAP_MS";
/// External-worktree sync probe period in ms; tests shorten it.
pub const WORKTREE_SYNC_MS: &str = "NEBULA_WORKTREE_SYNC_MS";
/// Cloud-mirror refresh cadence in seconds; `0` turns it off.
pub const CLOUD_MIRROR_SECS: &str = "NEBULA_CLOUD_MIRROR_SECS";
/// How long a WORKTREE HOOK may run before the daemon kills it, in ms
/// (default 30s); tests shorten it.
pub const HOOK_TIMEOUT_MS: &str = "NEBULA_HOOK_TIMEOUT_MS";
/// `RUST_LOG`-style tracing filter for both the daemon and the TUI.
pub const LOG: &str = "NEBULA_LOG";
/// Overrides the install script URL `nebula upgrade` / `nebula ssh` fetch.
pub const INSTALL_URL: &str = "NEBULA_INSTALL_URL";
/// Editor command the file modals open, ahead of the config's `editor`.
pub const EDITOR: &str = "NEBULA_EDITOR";
/// Cadence in seconds of the TUI's check for a newer published release
/// (the footer's `⇡ vX.Y.Z` update indicator); `0` turns it off, as the
/// e2e tests do so their footers never depend on what GitHub has published.
pub const UPDATE_CHECK_SECS: &str = "NEBULA_UPDATE_CHECK_SECS";

/// Env vars that identify an agent session to the daemon. They are set on
/// every agent PTY and must never leak into plain terminals.
pub const AGENT_SESSION_VARS: &[&str] = &[AGENT_ID, API_URL, API_TOKEN];

/// `TERM` every PTY child is given. The pane is nebula's own grid — a vt100
/// parser the TUI repaints through ratatui — and that grid keeps 24-bit
/// colour whatever terminal nebula itself runs in, so the child never
/// hears the host's `TERM` (`foot`, `xterm-ghostty`, `tmux-256color`).
pub const PANE_TERM: &str = "xterm-256color";
/// `COLORTERM` for the same grid: every `38;2;r;g;b` the child sends is
/// kept, so chalk-style detection may pick truecolor over the 256-colour
/// downsample `TERM` alone allows.
pub const PANE_COLORTERM: &str = "truecolor";
/// Colour overrides scrubbed from every PTY child. A `NO_COLOR` or a
/// `FORCE_COLOR=0` describes the shell the daemon happened to be started
/// from — an agent's tool shell, a CI job — or a login-only profile, not
/// the pane; Claude Code reads either as "no colour at all" and paints its
/// whole UI in the default foreground while the TUI around it stays
/// coloured (#37). The TUI still honours its own `NO_COLOR` on the way
/// out, so a user who wants none keeps none.
pub const PANE_COLOR_OVERRIDES: &[&str] = &["NO_COLOR", "FORCE_COLOR"];

/// The value of `var`, treating unset and empty the same way — an empty
/// override is how a caller says "use the default".
pub fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// `$HOME`, when the environment has one. Read as an `OsString` so a
/// non-UTF-8 home still resolves — every `~/` expansion goes through here.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_treats_unset_and_empty_alike() {
        let var = format!("NEBULA_TEST_NON_EMPTY_{}", std::process::id());
        assert_eq!(non_empty(&var), None);
        std::env::set_var(&var, "");
        assert_eq!(non_empty(&var), None);
        std::env::set_var(&var, "x");
        assert_eq!(non_empty(&var).as_deref(), Some("x"));
        std::env::remove_var(&var);
    }
}
