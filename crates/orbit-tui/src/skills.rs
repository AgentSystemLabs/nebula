//! SKILLS: the Claude skills on this machine, and the set a prompt is told
//! to use.
//!
//! A skill is a folder holding a `SKILL.md` whose YAML frontmatter names it
//! and says what it is for. Claude Code finds them in three places, and so
//! does [`discover`]:
//!
//! - **personal** — `~/.claude/skills/*/SKILL.md`;
//! - **project** — `<checkout>/.claude/skills/*/SKILL.md`;
//! - **plugin** — every plugin `~/.claude/plugins/installed_plugins.json`
//!   lists that is switched on (`enabledPlugins` in `~/.claude/settings.json`,
//!   then the checkout's `.claude/settings.json` and `settings.local.json`
//!   over it) and installed for this project — a `user` install anywhere, a
//!   `project` (or `local`) one only where its `projectPath` is the repo
//!   root. Its skills are `<installPath>/skills/*/SKILL.md`, named
//!   `<plugin>:<skill>` the way Claude's Skill tool names them.
//!
//! With a hundred skills on the machine Claude often reaches for the wrong
//! one, so the SKILL PICKER (`skill_picker.rs`) lets the user tick the ones
//! a task should use, and the [`SkillPick`] rides the prompt: one line in
//! front of the task names them and tells Claude to load each with its
//! Skill tool ([`SkillPick::wrap`]). That line is all there is — nothing
//! reaches the DAEMON but the prompt text.
//!
//! Discovery reads a hundred small files, so it never runs in a key
//! handler: the picker asks through the BACKGROUND READS
//! (`view_jobs::Answer::Skills`) and shows the last answer for the project
//! meanwhile (`JobState::skill_catalogs`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// The line [`SkillPick::wrap`] puts in front of a prompt, before the
/// skill names.
pub const PREAMBLE: &str = "[orbit] Use these skills for this task — load each one with the Skill tool before you start, and follow it: ";

/// Where a skill was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillSource {
    Personal,
    Project,
    Plugin,
}

impl SkillSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Project => "project",
            Self::Plugin => "plugin",
        }
    }
}

/// One skill on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The name Claude's Skill tool knows it by: the frontmatter `name`
    /// (else the folder's), prefixed `<plugin>:` for a plugin's.
    pub name: String,
    /// The frontmatter `description`, on one line; empty when it has none.
    pub description: String,
    pub source: SkillSource,
}

/// A SKILL WORKFLOW from the project file, with its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub name: String,
    pub skills: Vec<String>,
    pub text: Option<String>,
}

/// Everything the SKILL PICKER lists for one checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub workflows: Vec<Workflow>,
    pub skills: Vec<Skill>,
    /// The project file was there and could not be read: its workflows are
    /// missing, and the picker says why.
    pub error: Option<String>,
}

/// The skills a prompt is told to use, and the extra instructions of the
/// workflows they came from. Carried on the QUICK PROMPT's launch
/// (`QuickLaunch::skills`) and on the FOLLOW-UP box (`PromptKind::FollowUp`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPick {
    pub skills: Vec<String>,
    pub text: Option<String>,
}

impl SkillPick {
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The lines that go in front of the prompt: the skills line, then the
    /// workflow text. None with no skills — the prompt goes as typed.
    pub fn preamble(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let names: Vec<String> = self.skills.iter().map(|s| format!("`{s}`")).collect();
        let mut out = format!("{PREAMBLE}{}.", names.join(", "));
        if let Some(text) = self
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            out.push('\n');
            out.push_str(text);
        }
        Some(out)
    }

    /// `prompt` with the [`preamble`](Self::preamble) in front of it, a
    /// blank line between; the preamble alone for an empty prompt, and the
    /// prompt untouched when no skill is picked.
    pub fn wrap(&self, prompt: &str) -> String {
        match self.preamble() {
            None => prompt.to_string(),
            Some(head) if prompt.trim().is_empty() => head,
            Some(head) => format!("{head}\n\n{prompt}"),
        }
    }

    /// What a box's title says about the pick: `skills: tdd, incremental`,
    /// the first three and a count after that. None with nothing picked.
    pub fn label(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let shown: Vec<&str> = self.skills.iter().take(3).map(String::as_str).collect();
        let more = self.skills.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(" +{more}")
        } else {
            String::new()
        };
        Some(format!("skills: {}{tail}", shown.join(", ")))
    }
}

/// Claude's own folder: `$CLAUDE_CONFIG_DIR`, else `~/.claude`. Unit tests never see the real one —
/// they get the folder [`with_claude_home`] names, or one that does not
/// exist.
pub fn claude_home() -> PathBuf {
    #[cfg(test)]
    {
        CLAUDE_HOME_OVERRIDE.with(|slot| {
            slot.borrow()
                .clone()
                .unwrap_or_else(|| PathBuf::from("/nonexistent/orbit-test-claude-home"))
        })
    }
    #[cfg(not(test))]
    {
        // `CLAUDE_CONFIG_DIR` when set, as Claude Code itself reads it.
        orbit_core::paths::claude_config_dir().unwrap_or_default()
    }
}

#[cfg(test)]
thread_local! {
    static CLAUDE_HOME_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point [`claude_home`] at `path` for the duration of `f` (tests only).
#[cfg(test)]
pub fn with_claude_home<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    CLAUDE_HOME_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// Everything the picker lists for `checkout`, a checkout of the project
/// whose main checkout (and repo root) is `main`: the workflows of the
/// main checkout's project file with the checkout's own file's laid over
/// them by name — saving writes to the main checkout, and a branch with a
/// file of its own still sees what was saved — then every skill.
pub fn load_catalog(claude_home: &Path, checkout: &Path, main: &Path) -> Catalog {
    let mut workflows: BTreeMap<String, orbit_core::project_file::Workflow> = BTreeMap::new();
    let mut error = None;
    let mut dirs = vec![main];
    if checkout != main {
        dirs.push(checkout);
    }
    for dir in dirs {
        match orbit_core::project_file::read(dir) {
            Ok(Some(file)) => workflows.extend(file.workflows),
            Ok(None) => {}
            Err(e) => error = Some(e),
        }
    }
    Catalog {
        workflows: workflows
            .into_iter()
            .map(|(name, wf)| Workflow {
                name,
                skills: wf.skills,
                text: wf.text.filter(|t| !t.trim().is_empty()),
            })
            .collect(),
        skills: discover(claude_home, checkout, main),
        error,
    }
}

/// Every skill for `checkout` (a checkout of the repo at `repo_root`):
/// personal first, then the project's, then the plugins', each by name.
/// A name found twice keeps its first.
pub fn discover(claude_home: &Path, checkout: &Path, repo_root: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut personal = skills_in(&claude_home.join("skills"), None, SkillSource::Personal);
    sort(&mut personal);
    skills.extend(personal);
    let mut project = skills_in(
        &checkout.join(".claude").join("skills"),
        None,
        SkillSource::Project,
    );
    sort(&mut project);
    skills.extend(project);
    let mut plugins = Vec::new();
    for (plugin, install) in enabled_plugins(claude_home, checkout, repo_root) {
        plugins.extend(skills_in(
            &install.join("skills"),
            Some(&plugin),
            SkillSource::Plugin,
        ));
    }
    sort(&mut plugins);
    skills.extend(plugins);
    let mut seen = HashSet::new();
    skills.retain(|s| seen.insert(s.name.clone()));
    skills
}

fn sort(skills: &mut [Skill]) {
    skills.sort_by_key(|s| s.name.to_lowercase());
}

/// The skills in one `skills/` folder: each subfolder with a `SKILL.md`.
fn skills_in(dir: &Path, plugin: Option<&str>, source: SkillSource) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let folder = entry.path();
        let Ok(text) = std::fs::read_to_string(folder.join("SKILL.md")) else {
            continue;
        };
        let (name, description) = parse_frontmatter(&text);
        let name = name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
        let name = match plugin {
            Some(plugin) => format!("{plugin}:{name}"),
            None => name,
        };
        out.push(Skill {
            name,
            description: description.unwrap_or_default(),
            source,
        });
    }
    out
}

/// The plugins switched on for this project, as (plugin name, install
/// folder): listed in `installed_plugins.json`, `true` in the merged
/// `enabledPlugins`, and installed for everyone (`user`) or for this repo.
fn enabled_plugins(
    claude_home: &Path,
    checkout: &Path,
    repo_root: &Path,
) -> Vec<(String, PathBuf)> {
    let read_json = |path: PathBuf| -> Option<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let mut enabled: BTreeMap<String, bool> = BTreeMap::new();
    for settings in [
        claude_home.join("settings.json"),
        checkout.join(".claude").join("settings.json"),
        checkout.join(".claude").join("settings.local.json"),
    ] {
        let Some(value) = read_json(settings) else {
            continue;
        };
        if let Some(map) = value.get("enabledPlugins").and_then(|v| v.as_object()) {
            for (key, on) in map {
                if let Some(on) = on.as_bool() {
                    enabled.insert(key.clone(), on);
                }
            }
        }
    }
    let Some(installed) = read_json(claude_home.join("plugins").join("installed_plugins.json"))
    else {
        return Vec::new();
    };
    let Some(plugins) = installed.get("plugins").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let same_dir = |a: &Path, b: &Path| {
        a == b
            || matches!(
                (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                (Ok(a), Ok(b)) if a == b
            )
    };
    let mut out = Vec::new();
    for (key, installs) in plugins {
        if enabled.get(key) != Some(&true) {
            continue;
        }
        let name = key.split('@').next().unwrap_or(key).to_string();
        let Some(installs) = installs.as_array() else {
            continue;
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for install in installs {
            let Some(path) = install.get("installPath").and_then(|v| v.as_str()) else {
                continue;
            };
            let scope = install
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("user");
            let here = scope == "user"
                || install
                    .get("projectPath")
                    .and_then(|v| v.as_str())
                    .is_some_and(|p| same_dir(Path::new(p), repo_root));
            let path = PathBuf::from(path);
            if here && !paths.contains(&path) {
                paths.push(path);
            }
        }
        out.extend(paths.into_iter().map(|path| (name.clone(), path)));
    }
    out
}

/// The `name` and `description` of a `SKILL.md`'s YAML frontmatter — the
/// block between the opening `---` line and the next. Enough YAML for what
/// skills write: plain, single- and double-quoted scalars, plain scalars
/// folded over indented lines, and `|` / `>` block scalars. A field that is
/// missing (or a file with no frontmatter) is None. The description comes
/// back on one line.
pub fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    let (mut name, mut description) = (None, None);
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut more = Vec::new();
        while i < body.len() && (body[i].starts_with([' ', '\t']) || body[i].trim().is_empty()) {
            more.push(body[i]);
            i += 1;
        }
        let slot = match key.trim() {
            "name" => &mut name,
            "description" => &mut description,
            _ => continue,
        };
        let value = one_line(&scalar(rest.trim(), &more));
        *slot = (!value.is_empty()).then_some(value);
    }
    (name, description)
}

/// Whitespace runs, line breaks included, as single spaces.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One YAML scalar: `first` is what follows the key's colon, `more` the
/// indented (or blank) lines after it.
fn scalar(first: &str, more: &[&str]) -> String {
    let rest = || more.iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
    if first.starts_with('|') || first.starts_with('>') {
        return rest();
    }
    let joined = std::iter::once(first)
        .chain(more.iter().map(|l| l.trim()))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(quoted) = joined.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        return out;
    }
    if let Some(quoted) = joined.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = quoted.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push('\'');
                    continue;
                }
                break;
            }
            out.push(c);
        }
        return out;
    }
    // A plain scalar ends at a ` #` comment.
    match joined.find(" #") {
        Some(at) => joined[..at].to_string(),
        None => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill_md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n")
    }

    #[test]
    fn frontmatter_reads_plain_and_quoted_values() {
        assert_eq!(
            parse_frontmatter("---\nname: tdd\ndescription: Red, green, refactor.\n---\nbody"),
            (Some("tdd".into()), Some("Red, green, refactor.".into()))
        );
        assert_eq!(
            parse_frontmatter(
                "---\nname: \"quoted\"\ndescription: \"Says \\\"hi\\\": twice\"\n---\n"
            ),
            (Some("quoted".into()), Some("Says \"hi\": twice".into()))
        );
        assert_eq!(
            parse_frontmatter("---\nname: 'it''s'\ndescription: 'a: b'\n---\n"),
            (Some("it's".into()), Some("a: b".into()))
        );
    }

    #[test]
    fn frontmatter_folds_long_descriptions_onto_one_line() {
        let text = "---\nname: x\ndescription: >\n  Folded over\n  two lines.\nlicense: MIT\n---\n";
        assert_eq!(
            parse_frontmatter(text),
            (Some("x".into()), Some("Folded over two lines.".into()))
        );
        let text = "---\ndescription: plain start\n  continued here # note\nname: y\n---\n";
        assert_eq!(
            parse_frontmatter(text),
            (Some("y".into()), Some("plain start continued here".into()))
        );
        let text = "---\nname: z\ndescription: |\n  kept\n  lines\n---\n";
        assert_eq!(parse_frontmatter(text).1.as_deref(), Some("kept lines"));
    }

    #[test]
    fn frontmatter_tolerates_missing_fields_and_files_without_one() {
        assert_eq!(parse_frontmatter("# just markdown\n"), (None, None));
        assert_eq!(
            parse_frontmatter("---\ndescription: only this\n---\n"),
            (None, Some("only this".into()))
        );
        assert_eq!(parse_frontmatter("---\nname:\n---\n"), (None, None));
        assert_eq!(
            parse_frontmatter("\u{feff}---\r\nname: bom\r\n---\r\n")
                .0
                .as_deref(),
            Some("bom")
        );
    }

    #[test]
    fn the_pick_goes_in_front_of_the_prompt() {
        let none = SkillPick::default();
        assert_eq!(none.wrap("do it"), "do it", "no skills: untouched");
        assert_eq!(none.label(), None);

        let pick = SkillPick {
            skills: vec!["tdd".into(), "incremental".into()],
            text: None,
        };
        assert_eq!(
            pick.wrap("Fix login"),
            format!("{PREAMBLE}`tdd`, `incremental`.\n\nFix login")
        );
        assert_eq!(pick.wrap("  "), format!("{PREAMBLE}`tdd`, `incremental`."));
        assert_eq!(pick.label().as_deref(), Some("skills: tdd, incremental"));

        let with_text = SkillPick {
            text: Some("Keep each slice small.".into()),
            ..pick
        };
        assert_eq!(
            with_text.wrap("Fix login"),
            format!("{PREAMBLE}`tdd`, `incremental`.\nKeep each slice small.\n\nFix login")
        );
        let many = SkillPick {
            skills: ["a", "b", "c", "d", "e"].map(String::from).to_vec(),
            text: None,
        };
        assert_eq!(many.label().as_deref(), Some("skills: a, b, c +2"));
    }

    /// The three sources, from temp folders standing in for `~/.claude`
    /// and the checkout: a disabled plugin and a plugin installed for
    /// another project are left out, and the folder name stands in for a
    /// missing frontmatter name.
    #[test]
    fn discovery_reads_personal_project_and_enabled_plugin_skills() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (home, repo) = (home.path(), repo.path());

        write(
            &home.join("skills/tdd/SKILL.md"),
            &skill_md("test-driven-development", "\"Red, green: refactor.\""),
        );
        write(&home.join("skills/nameless/SKILL.md"), "no frontmatter");
        write(&home.join("skills/empty-dir/README.md"), "not a skill");
        write(
            &repo.join(".claude/skills/deploy/SKILL.md"),
            &skill_md("deploy", "Ship it."),
        );

        let install = |name: &str| home.join(format!("plugins/cache/{name}"));
        for name in ["on", "off", "elsewhere", "mine"] {
            write(
                &install(name).join("skills/review/SKILL.md"),
                &skill_md("review", &format!("{name} review")),
            );
        }
        write(
            &home.join("plugins/installed_plugins.json"),
            &serde_json::json!({
                "version": 2,
                "plugins": {
                    "on@market": [{"scope": "user", "installPath": install("on")}],
                    "off@market": [{"scope": "user", "installPath": install("off")}],
                    "elsewhere@market": [{
                        "scope": "project",
                        "installPath": install("elsewhere"),
                        "projectPath": other.path(),
                    }],
                    "mine@market": [{
                        "scope": "project",
                        "installPath": install("mine"),
                        "projectPath": repo,
                    }],
                    "missing@market": [{"scope": "user", "installPath": "/nonexistent"}],
                }
            })
            .to_string(),
        );
        write(
            &home.join("settings.json"),
            r#"{"enabledPlugins": {"on@market": true, "off@market": false,
                "elsewhere@market": true, "mine@market": true, "missing@market": true}}"#,
        );

        let skills = discover(home, repo, repo);
        let rows: Vec<(&str, SkillSource, &str)> = skills
            .iter()
            .map(|s| (s.name.as_str(), s.source, s.description.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("nameless", SkillSource::Personal, ""),
                (
                    "test-driven-development",
                    SkillSource::Personal,
                    "Red, green: refactor."
                ),
                ("deploy", SkillSource::Project, "Ship it."),
                ("mine:review", SkillSource::Plugin, "mine review"),
                ("on:review", SkillSource::Plugin, "on review"),
            ]
        );
    }

    /// A project's own settings can switch a plugin on, or off again.
    #[test]
    fn the_checkout_settings_layer_over_the_user_ones() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let (home, repo) = (home.path(), repo.path());
        let install = home.join("plugins/cache/p");
        write(
            &install.join("skills/s/SKILL.md"),
            &skill_md("s", "a skill"),
        );
        write(
            &home.join("plugins/installed_plugins.json"),
            &serde_json::json!({"version": 2, "plugins": {
                "p@m": [{"scope": "user", "installPath": install}]
            }})
            .to_string(),
        );
        assert!(
            discover(home, repo, repo).is_empty(),
            "not enabled anywhere"
        );
        write(
            &repo.join(".claude/settings.json"),
            r#"{"enabledPlugins": {"p@m": true}}"#,
        );
        assert_eq!(discover(home, repo, repo).len(), 1);
        write(
            &repo.join(".claude/settings.local.json"),
            r#"{"enabledPlugins": {"p@m": false}}"#,
        );
        assert!(discover(home, repo, repo).is_empty());
    }

    #[test]
    fn the_catalog_merges_the_main_checkouts_workflows_with_the_worktrees() {
        let home = tempfile::tempdir().unwrap();
        let main = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        write(
            &main.path().join(".orbit/config.json"),
            r#"{"run": "x", "workflows": {"feature": {"skills": ["a"], "text": "go"},
                "fix": {"skills": ["b"]}}}"#,
        );
        write(
            &worktree.path().join(".orbit/config.json"),
            r#"{"workflows": {"fix": {"skills": ["c"]}}}"#,
        );
        let catalog = load_catalog(home.path(), worktree.path(), main.path());
        assert_eq!(catalog.error, None);
        assert_eq!(
            catalog.workflows,
            vec![
                Workflow {
                    name: "feature".into(),
                    skills: vec!["a".into()],
                    text: Some("go".into()),
                },
                Workflow {
                    name: "fix".into(),
                    skills: vec!["c".into()],
                    text: None,
                },
            ]
        );
        write(&worktree.path().join(".orbit/config.json"), "{");
        let broken = load_catalog(home.path(), worktree.path(), main.path());
        assert!(broken.error.is_some());
        assert_eq!(broken.workflows.len(), 2, "the main checkout's still list");
    }
}
