//! User settings, read from `paths::config_path()` with
//! `paths::config_local_path()` over it (JSON; see `nebula_core::settings`).
//! Loaded fresh at each use so edits apply without restarting the daemon. A
//! missing file or unknown fields fall back to defaults; a malformed file is
//! logged and ignored rather than failing the operation that read it, and a
//! value this build can't read costs only its own key.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// Bounds what prewarming and walked-away-from sessions cost. Running
    /// or feedback-waiting agents, agents with a backgrounded tool call
    /// still running, and terminals with a command running are spared; a
    /// reaped session revives on the next attach or prewarm (agents resume
    /// their conversation). Malformed values fall back to the 5m default.
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
    /// User-defined harnesses from the `custom_harnesses` key, shared with
    /// the TUI's picker. The daemon resolves programs, model flags and
    /// respawns from this list; entries that fail validation are refused
    /// at create time with the reason, never launched.
    pub custom_harnesses: Vec<nebula_core::harness::CustomHarness>,
    /// Per-harness deltas over the compiled-in registry (`harnesses` in
    /// config.json): repoint a program, rename a flag, switch a harness
    /// off, or define a whole new CLI. Merged by
    /// [`nebula_core::harness::registry`]; a broken entry refuses its
    /// launches with the reason, never the whole daemon.
    pub harnesses: BTreeMap<String, nebula_core::harness::HarnessOverride>,
    /// PROJECT SETTINGS: one entry per project set up differently from the
    /// rest, keyed by the project's repo path as the store holds it. The
    /// TUI's Project tab owns the map; the daemon reads the one key in it
    /// that is its to act on, through [`Config::run_command`].
    pub projects: BTreeMap<PathBuf, ProjectConfig>,
}

/// One project's entry under `projects` — the rows of the TUI's Project
/// tab, of which the daemon reads one. The rest (`hide_root_worktree`)
/// are the TUI's and pass through unread.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// The RUN COMMAND `r` starts in this project's worktrees, typed into
    /// Settings → Project. Empty means the checkout's `.nebula.json`
    /// `run`, as before the row existed.
    pub run_command: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            git_init_on_create: true,
            prewarm_agents: true,
            prewarm_sessions: true,
            session_idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT.into(),
            worktree_base_branch: String::new(),
            custom_harnesses: Vec::new(),
            harnesses: BTreeMap::new(),
            projects: BTreeMap::new(),
        }
    }
}

/// Fallback for `session_idle_timeout` when the value is malformed.
pub const DEFAULT_SESSION_IDLE_TIMEOUT: &str = "5m";

impl Config {
    pub fn load() -> Self {
        let loaded = nebula_core::settings::load::<Self>(
            &nebula_core::paths::config_path(),
            &nebula_core::paths::config_local_path(),
        );
        for problem in &loaded.problems {
            tracing::warn!("{problem}");
        }
        if !loaded.skipped.is_empty() {
            tracing::warn!(keys = ?loaded.skipped, "settings this build can't read keep their defaults");
        }
        loaded.value
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

    /// The RUN COMMAND set for the project checked out at `repo_path` in
    /// Settings → Project, trimmed; None when the project has no entry or
    /// the row is empty, which means the checkout's `.nebula.json` decides.
    pub fn run_command(&self, repo_path: &Path) -> Option<&str> {
        self.projects
            .get(repo_path)
            .map(|p| p.run_command.trim())
            .filter(|c| !c.is_empty())
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

    /// One mistyped key — or a key a newer nebula changed the type of —
    /// takes its default without dragging every other setting down with it.
    #[test]
    fn an_unreadable_key_costs_only_that_key() {
        let obj = serde_json::json!({
            "prewarm_agents": "nope",
            "session_idle_timeout": "1h",
            "git_init_on_create": false,
        });
        let (cfg, skipped) =
            nebula_core::settings::parse_lenient::<Config>(obj.as_object().unwrap());
        assert!(cfg.prewarm_agents, "the unreadable key takes its default");
        assert_eq!(cfg.session_idle_timeout, "1h");
        assert!(!cfg.git_init_on_create);
        assert_eq!(skipped.len(), 1);
    }

    /// The daemon's half of the TUI's fixture test: every daemon key an
    /// earlier release wrote still reads as written.
    #[test]
    fn config_files_from_earlier_releases_still_load_the_daemon_keys() {
        let raw = include_str!("../../nebula-core/fixtures/config-0.28.0.json");
        let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
        let (cfg, skipped) = nebula_core::settings::parse_lenient::<Config>(&obj);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(!cfg.git_init_on_create && !cfg.prewarm_agents && !cfg.prewarm_sessions);
        assert_eq!(cfg.session_idle_timeout, "30m");
        assert_eq!(cfg.worktree_base_branch, "develop");
        assert_eq!(cfg.custom_harnesses.len(), 1, "the legacy list reads");
        assert!(cfg.harnesses.contains_key("grok"), "the registry map reads");
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

    /// The Project tab's `run_command`, under the project's repo path:
    /// trimmed, blank is unset, and the entry's other rows — the TUI's —
    /// don't stop the daemon reading it.
    #[test]
    fn run_command_is_read_per_project_and_blank_means_the_file() {
        assert_eq!(Config::default().run_command(Path::new("/tmp/demo")), None);
        let cfg: Config = serde_json::from_str(
            r#"{"projects": {
                "/tmp/demo": { "hide_root_worktree": true, "run_command": "  npm run dev " },
                "/tmp/blank": { "run_command": "   " },
                "/tmp/other": { "hide_root_worktree": false, "future_row": 1 }
            }}"#,
        )
        .unwrap();
        assert_eq!(cfg.run_command(Path::new("/tmp/demo")), Some("npm run dev"));
        assert_eq!(cfg.run_command(Path::new("/tmp/blank")), None);
        assert_eq!(cfg.run_command(Path::new("/tmp/other")), None);
        assert_eq!(cfg.run_command(Path::new("/tmp/unknown")), None);
        // A `projects` this build can't read costs the map, not the rest.
        let obj = serde_json::json!({
            "projects": { "/tmp/demo": { "run_command": ["npm", "run", "dev"] } },
            "worktree_base_branch": "develop",
        });
        let (cfg, skipped) =
            nebula_core::settings::parse_lenient::<Config>(obj.as_object().unwrap());
        assert_eq!(cfg.run_command(Path::new("/tmp/demo")), None);
        assert_eq!(cfg.worktree_base_branch, "develop");
        assert_eq!(skipped.into_iter().collect::<Vec<_>>(), ["projects"]);
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
