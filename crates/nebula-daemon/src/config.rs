//! User settings, read from `paths::config_path()` (JSON). Loaded fresh at
//! each use so edits apply without restarting the daemon. A missing file or
//! unknown fields fall back to defaults; a malformed file is logged and
//! ignored rather than failing the operation that read it.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Run `git init` after AddProject creates a missing directory.
    pub git_init_on_create: bool,
    /// Pre-spawn agent CLIs while the user is still naming the session so
    /// creation feels instant. Costs one idle CLI process per warm slot.
    pub prewarm_agents: bool,
    /// Pre-spawn a worktree's dead sessions when the user's selection rests
    /// on it, so attaching shows an already-booted screen instead of a
    /// booting shell. Costs idle shell/CLI processes for sessions the user
    /// may never open.
    pub prewarm_sessions: bool,
    /// Kill idle session PTYs in worktrees no client is looking at once
    /// they've gone unwatched this long: "1m" | "5m" | "15m" | "30m" | "1h"
    /// ("off" disables reaping entirely; any `<n>s`/`<n>m`/`<n>h` works).
    /// Bounds what prewarming and walked-away-from sessions cost. Pinned
    /// agents, running or feedback-waiting agents, and terminals with a
    /// command running are spared; a reaped session revives on the next
    /// attach or prewarm (agents resume their conversation). Malformed
    /// values fall back to the 5m default.
    pub session_idle_timeout: String,
    /// The branch every new WORKTREE nobody named a base for starts from
    /// — `n` in the WORKTREES PANEL, a bare `nebula worktree`, the QUICK
    /// PROMPT's auto-created one. Empty (the default) means origin's own
    /// default branch, `origin/HEAD` as freshly fetched; a name (`master`,
    /// `develop`) means origin's fetched copy of that branch when origin
    /// has one, else the checkout's local ref of that name, else — a repo
    /// with no such branch at all — the default again, with a warning in
    /// the daemon log. Read through [`Config::worktree_base_branch`].
    pub worktree_base_branch: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            git_init_on_create: true,
            prewarm_agents: true,
            prewarm_sessions: true,
            session_idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT.into(),
            worktree_base_branch: String::new(),
        }
    }
}

/// Fallback for `session_idle_timeout` when the value is malformed.
pub const DEFAULT_SESSION_IDLE_TIMEOUT: &str = "5m";

impl Config {
    pub fn load() -> Self {
        let path = nebula_core::paths::config_path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_else(|err| {
            tracing::warn!("ignoring malformed {}: {err}", path.display());
            Self::default()
        })
    }

    /// `session_idle_timeout` parsed to a duration; None = reaping disabled.
    pub fn session_idle_timeout(&self) -> Option<std::time::Duration> {
        parse_timeout(&self.session_idle_timeout)
            .unwrap_or_else(|| parse_timeout(DEFAULT_SESSION_IDLE_TIMEOUT).expect("default parses"))
    }

    /// The configured WORKTREE BASE BRANCH, or None for the default
    /// (origin's own default branch). Whitespace is trimmed and a leading
    /// `origin/` dropped: `origin/master` means the same as `master` —
    /// origin's fetched copy when it has one — and spelling it out must
    /// not turn into a branch that tracks `origin/master` and aims its
    /// first push there.
    pub fn worktree_base_branch(&self) -> Option<&str> {
        let name = self.worktree_base_branch.trim();
        let name = name.strip_prefix("origin/").unwrap_or(name).trim();
        (!name.is_empty()).then_some(name)
    }
}

/// "off"/"0" → Some(None); "<n>s"/"<n>m"/"<n>h" → Some(Some(d));
/// malformed → None (caller falls back to the default).
#[allow(clippy::option_option)]
fn parse_timeout(s: &str) -> Option<Option<std::time::Duration>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") || s == "0" {
        return Some(None);
    }
    let (digits, unit_secs) = match s.strip_suffix(['s', 'S']) {
        Some(d) => (d, 1),
        None => match s.strip_suffix(['m', 'M']) {
            Some(d) => (d, 60),
            None => (s.strip_suffix(['h', 'H'])?, 3_600),
        },
    };
    let n: u64 = digits.trim().parse().ok()?;
    if n == 0 {
        return Some(None);
    }
    Some(Some(std::time::Duration::from_secs(n * unit_secs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_git_init() {
        assert!(Config::default().git_init_on_create);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.git_init_on_create);
        let cfg: Config = serde_json::from_str(r#"{"git_init_on_create": false}"#).unwrap();
        assert!(!cfg.git_init_on_create);
    }

    #[test]
    fn defaults_enable_prewarm_and_allow_opt_out() {
        assert!(Config::default().prewarm_agents);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.prewarm_agents);
        let cfg: Config = serde_json::from_str(r#"{"prewarm_agents": false}"#).unwrap();
        assert!(!cfg.prewarm_agents);
    }

    #[test]
    fn session_idle_timeout_parses_and_falls_back() {
        use std::time::Duration;
        let timeout = |v: &str| {
            let cfg: Config =
                serde_json::from_str(&format!(r#"{{"session_idle_timeout": "{v}"}}"#)).unwrap();
            cfg.session_idle_timeout()
        };
        assert_eq!(timeout("1m"), Some(Duration::from_secs(60)));
        assert_eq!(timeout("5m"), Some(Duration::from_secs(300)));
        assert_eq!(timeout("15m"), Some(Duration::from_secs(900)));
        assert_eq!(timeout("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(timeout("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(
            timeout("2s"),
            Some(Duration::from_secs(2)),
            "seconds for tests"
        );
        assert_eq!(timeout("off"), None);
        assert_eq!(timeout("0"), None);
        // Malformed values fall back to the default, not to disabled.
        assert_eq!(timeout("soon"), Some(Duration::from_secs(300)));
        assert_eq!(
            Config::default().session_idle_timeout(),
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn worktree_base_branch_defaults_to_none_and_normalizes() {
        assert_eq!(Config::default().worktree_base_branch(), None);
        let base = |v: &str| {
            let cfg: Config =
                serde_json::from_str(&format!(r#"{{"worktree_base_branch": "{v}"}}"#)).unwrap();
            cfg.worktree_base_branch().map(str::to_string)
        };
        assert_eq!(base(""), None);
        assert_eq!(base("   "), None, "blank is unset, not a branch called ' '");
        assert_eq!(base("master"), Some("master".into()));
        assert_eq!(base("  develop "), Some("develop".into()));
        assert_eq!(
            base("origin/master"),
            Some("master".into()),
            "origin/x is x: origin's copy is what the name already means"
        );
        assert_eq!(base("origin/"), None);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.worktree_base_branch(), None);
    }

    #[test]
    fn defaults_enable_session_prewarm_and_allow_opt_out() {
        assert!(Config::default().prewarm_sessions);
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.prewarm_sessions);
        let cfg: Config = serde_json::from_str(r#"{"prewarm_sessions": false}"#).unwrap();
        assert!(!cfg.prewarm_sessions);
    }
}
