//! SETTINGS BUNDLES: one JSON file carrying the portable settings from one
//! machine to another — `config.json`, the AGENT PRESETS and the SSH HOSTS
//! FILE. `nebula config export` writes one, `nebula config import` merges one
//! in, and `nebula ssh` / `nebula tunnel` hand one to the remote nebula in
//! `NEBULA_IMPORT_BUNDLE`, so the remote takes this machine's settings on
//! every connect.
//!
//! ```json
//! { "nebula_bundle": 1, "exported_by": "0.27.0",
//!   "config": { "theme": "ocean" }, "agent_presets": [], "ssh_hosts": [] }
//! ```
//!
//! The two ends are often different releases, so every section travels as
//! the raw JSON its file holds, never through the typed structs: a key or a
//! preset field this build has never heard of is exactly what a newer one
//! wrote, and dropping it on the way through would undo that build's
//! settings. Sections are optional and only ever added; a reader leaves one
//! it doesn't know alone. `nebula_bundle` marks the file as a bundle and is
//! never bumped — a change of meaning ships under a new section name
//! instead. `config.local.json` is never exported, and an import never
//! writes it.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use nebula_core::settings::{self, Object};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The key every bundle carries; its value is [`FORMAT`].
pub const MARKER: &str = "nebula_bundle";
/// Never bumped: see the module docs.
pub const FORMAT: u64 = 1;
/// What `nebula config export <folder>` writes, and what a folder import
/// looks for first.
pub const FILE_NAME: &str = "nebula-settings.json";
/// The biggest encoded bundle `nebula ssh` puts on the remote command line.
/// Linux caps a single argument at 128 KiB and the whole remote command
/// travels as one; a real bundle is a few KiB.
pub const MAX_FORWARD_BYTES: usize = 64 * 1024;

const CONFIG: &str = "config";
const PRESETS: &str = "agent_presets";
const HOSTS: &str = "ssh_hosts";

/// Each section, with the name its file has in a data dir.
const SECTIONS: [(&str, &str); 3] = [
    (CONFIG, "config.json"),
    (PRESETS, "agent_presets.json"),
    (HOSTS, "ssh_hosts.json"),
];

/// The files a bundle is read from and merged into.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    /// Read only, to say which imported keys it still overrides.
    pub local: PathBuf,
    pub presets: PathBuf,
    pub hosts: PathBuf,
}

impl Paths {
    /// This nebula's own settings files.
    pub fn current() -> Self {
        Self {
            config: nebula_core::paths::config_path(),
            local: nebula_core::paths::config_local_path(),
            presets: crate::agent_presets::store_path(),
            hosts: crate::hosts::store_path(),
        }
    }
}

/// Which sections an export carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everything portable: a backup.
    Backup,
    /// What a remote machine takes on over `nebula ssh`. The SSH HOSTS FILE
    /// lists destinations as seen from here, so it stays behind.
    Remote,
}

/// A bundle of the settings at `paths`, and a line for each file that is
/// there but unreadable — its section is left out rather than failing the
/// export. A missing file is simply no section.
pub fn export(paths: &Paths, scope: Scope) -> (Value, Vec<String>) {
    let mut bundle = Object::new();
    bundle.insert(MARKER.into(), json!(FORMAT));
    bundle.insert("exported_by".into(), json!(env!("CARGO_PKG_VERSION")));
    let mut warnings = Vec::new();
    let mut carry = |section: &str, path: &Path, read: Result<Option<Value>, String>| match read {
        Ok(Some(value)) => {
            bundle.insert(section.into(), value);
        }
        Ok(None) => {}
        Err(err) => warnings.push(format!("left {} out: {err}", path.display())),
    };
    carry(
        CONFIG,
        &paths.config,
        settings::read_object(&paths.config).map(|obj| obj.map(Value::Object)),
    );
    carry(
        PRESETS,
        &paths.presets,
        settings::read_array(&paths.presets).map(|list| list.map(Value::Array)),
    );
    if scope == Scope::Backup {
        carry(
            HOSTS,
            &paths.hosts,
            settings::read_array(&paths.hosts).map(|list| list.map(Value::Array)),
        );
    }
    (Value::Object(bundle), warnings)
}

/// How many settings sections `bundle` carries.
pub fn section_count(bundle: &Value) -> usize {
    SECTIONS
        .iter()
        .filter(|(section, _)| bundle.get(*section).is_some())
        .count()
}

/// What an import changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// `config.json` keys whose value changed.
    pub config_changed: usize,
    /// Imported keys `config.local.json` holds a different value for, so they
    /// don't take effect on this machine.
    pub held_local: Vec<String>,
    pub presets_added: usize,
    pub presets_replaced: usize,
    pub hosts_added: usize,
    /// Whether the SSH HOSTS FILE changed at all (a newer stamp counts).
    pub hosts_changed: bool,
    /// Sections this build doesn't know, left alone.
    pub unknown_sections: Vec<String>,
}

impl Report {
    pub fn changed(&self) -> bool {
        self.config_changed + self.presets_added + self.presets_replaced > 0 || self.hosts_changed
    }

    /// One line: `config: 3 keys changed · presets: 1 added, 0 replaced`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.config_changed > 0 {
            parts.push(format!(
                "config: {} changed",
                plural(self.config_changed, "key")
            ));
        }
        if self.presets_added + self.presets_replaced > 0 {
            parts.push(format!(
                "presets: {} added, {} replaced",
                self.presets_added, self.presets_replaced
            ));
        }
        if self.hosts_added > 0 {
            parts.push(format!("ssh hosts: {} added", self.hosts_added));
        } else if self.hosts_changed {
            parts.push("ssh hosts: updated".into());
        }
        if parts.is_empty() {
            parts.push("nothing changed".into());
        }
        if !self.held_local.is_empty() {
            parts.push(format!(
                "config.local.json still overrides {}",
                self.held_local.join(", ")
            ));
        }
        if !self.unknown_sections.is_empty() {
            parts.push(format!(
                "left alone, unknown to this nebula: {}",
                self.unknown_sections.join(", ")
            ));
        }
        parts.join(" · ")
    }
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Merge `bundle` into the settings at `paths`. `config` keys it sets
/// replace this machine's and keys it lacks are left alone; presets merge by
/// name, hosts by destination. Every file the import touches is read before
/// any is written, so a file that can't be replaced safely fails the whole
/// import with nothing changed.
pub fn import(paths: &Paths, bundle: &Value) -> Result<Report> {
    let Some(obj) = bundle.as_object().filter(|obj| obj.contains_key(MARKER)) else {
        bail!("not a nebula settings bundle (it has no \"{MARKER}\" key)");
    };
    let incoming_config = match obj.get(CONFIG) {
        None => None,
        Some(Value::Object(config)) => Some(config),
        Some(_) => bail!("the bundle's \"{CONFIG}\" is not a JSON object"),
    };
    let incoming_presets = list_section(obj, PRESETS)?;
    let incoming_hosts = list_section(obj, HOSTS)?;

    let mut config = incoming_config
        .map(|_| current_object(&paths.config))
        .transpose()?;
    let mut presets = incoming_presets
        .map(|_| current_list(&paths.presets))
        .transpose()?;
    let mut hosts = incoming_hosts
        .map(|_| current_list(&paths.hosts))
        .transpose()?;

    let mut report = Report {
        unknown_sections: obj
            .iter()
            .filter(|(key, value)| {
                (value.is_object() || value.is_array())
                    && !SECTIONS.iter().any(|(section, _)| *section == key.as_str())
            })
            .map(|(key, _)| key.clone())
            .collect(),
        ..Report::default()
    };
    if let (Some(incoming), Some(root)) = (incoming_config, config.as_mut()) {
        let held = settings::read_object(&paths.local)
            .ok()
            .flatten()
            .unwrap_or_default();
        for (key, value) in incoming {
            if root.get(key) != Some(value) {
                root.insert(key.clone(), value.clone());
                report.config_changed += 1;
            }
            if held.get(key).is_some_and(|local| local != value) {
                report.held_local.push(key.clone());
            }
        }
    }
    if let (Some(incoming), Some(list)) = (incoming_presets, presets.as_mut()) {
        (report.presets_added, report.presets_replaced) = merge_presets(list, incoming);
    }
    if let (Some(incoming), Some(list)) = (incoming_hosts, hosts.as_mut()) {
        (report.hosts_added, report.hosts_changed) = merge_hosts(list, incoming);
    }

    if let Some(root) = config.filter(|_| report.config_changed > 0) {
        write(&paths.config, Value::Object(root))?;
    }
    if let Some(list) = presets.filter(|_| report.presets_added + report.presets_replaced > 0) {
        write(&paths.presets, Value::Array(list))?;
    }
    if let Some(list) = hosts.filter(|_| report.hosts_changed) {
        write(&paths.hosts, Value::Array(list))?;
    }
    Ok(report)
}

fn list_section<'a>(bundle: &'a Object, section: &str) -> Result<Option<&'a Vec<Value>>> {
    match bundle.get(section) {
        None => Ok(None),
        Some(Value::Array(list)) => Ok(Some(list)),
        Some(_) => bail!("the bundle's \"{section}\" is not a JSON list"),
    }
}

/// What a settings file holds now — nothing when it is missing, an error
/// when it is there but unreadable: an import never replaces settings it
/// could not read.
fn current_object(path: &Path) -> Result<Object> {
    settings::read_object(path)
        .map(Option::unwrap_or_default)
        .map_err(|err| refuse(path, err))
}

fn current_list(path: &Path) -> Result<Vec<Value>> {
    settings::read_array(path)
        .map(Option::unwrap_or_default)
        .map_err(|err| refuse(path, err))
}

fn refuse(path: &Path, err: String) -> anyhow::Error {
    anyhow!(
        "{}: {err} — fix it or move it aside, then import again",
        path.display()
    )
}

fn write(path: &Path, value: Value) -> Result<()> {
    settings::write_json(path, &value).with_context(|| format!("writing {}", path.display()))
}

/// Incoming presets merged by name, case-insensitively as the preset editor
/// keeps names unique: a match is replaced, a new name appended, an entry
/// with no name skipped. Returns (added, replaced).
fn merge_presets(list: &mut Vec<Value>, incoming: &[Value]) -> (usize, usize) {
    use crate::agent_presets::entry_name;
    let (mut added, mut replaced) = (0, 0);
    for entry in incoming {
        let Some(name) = entry_name(entry).map(str::trim).filter(|n| !n.is_empty()) else {
            continue;
        };
        let same = |stored: &&mut Value| {
            entry_name(stored).is_some_and(|n| n.trim().eq_ignore_ascii_case(name))
        };
        match list.iter_mut().find(same) {
            Some(slot) if *slot == *entry => {}
            Some(slot) => {
                *slot = entry.clone();
                replaced += 1;
            }
            None => {
                list.push(entry.clone());
                added += 1;
            }
        }
    }
    (added, replaced)
}

/// Incoming destinations merged by host and start dir, keeping whichever
/// was used last; the list is then re-sorted newest first and capped the way
/// `nebula ssh` keeps it. Returns (added, whether the list changed).
fn merge_hosts(list: &mut Vec<Value>, incoming: &[Value]) -> (usize, bool) {
    fn id(entry: &Value) -> Option<(&str, Option<&str>)> {
        Some((
            entry.get("host")?.as_str()?,
            entry.get("path").and_then(Value::as_str),
        ))
    }
    fn stamp(entry: &Value) -> i64 {
        entry
            .get("last_used_ms")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }
    let before = list.clone();
    let mut added = 0;
    for entry in incoming {
        let Some(key) = id(entry) else {
            continue;
        };
        match list.iter_mut().find(|stored| id(stored) == Some(key)) {
            Some(slot) => {
                if stamp(entry) > stamp(slot) {
                    *slot = entry.clone();
                }
            }
            None => {
                list.push(entry.clone());
                added += 1;
            }
        }
    }
    list.sort_by_key(|entry| std::cmp::Reverse(stamp(entry)));
    list.truncate(crate::hosts::MAX_HOSTS);
    let changed = *list != before;
    (added, changed)
}

/// What `nebula config import` was pointed at, as a bundle: a bundle file; a
/// bare `config.json`, `agent_presets.json` or `ssh_hosts.json`; a folder
/// holding a [`FILE_NAME`] or any of those three (never `config.local.json`);
/// or `-` for stdin.
pub fn read_source(source: &str) -> Result<Value> {
    if source == "-" {
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .context("reading stdin")?;
        return classify(parse(&raw, "stdin")?, None);
    }
    let path = Path::new(source);
    if !path.is_dir() {
        return read_file(path);
    }
    let bundle_file = path.join(FILE_NAME);
    if bundle_file.is_file() {
        return read_file(&bundle_file);
    }
    let mut bundle = bare_bundle();
    for (section, name) in SECTIONS {
        let file = path.join(name);
        if file.is_file() {
            if let Some(value) = read_file(&file)?.get(section) {
                bundle.insert(section.into(), value.clone());
            }
        }
    }
    if section_count(&Value::Object(bundle.clone())) == 0 {
        bail!(
            "{} holds none of {FILE_NAME}, config.json, agent_presets.json or ssh_hosts.json",
            path.display()
        );
    }
    Ok(Value::Object(bundle))
}

fn read_file(path: &Path) -> Result<Value> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    classify(
        parse(&raw, &path.display().to_string())?,
        path.file_name().and_then(|name| name.to_str()),
    )
}

fn parse(raw: &str, source: &str) -> Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("{source} is not valid JSON"))
}

/// A parsed file as a bundle. An object is a bundle when it says so and a
/// `config.json` otherwise; a list's section is whatever its file is named,
/// since presets and hosts are both lists.
fn classify(value: Value, file_name: Option<&str>) -> Result<Value> {
    let section = match (&value, file_name) {
        (Value::Object(obj), _) if obj.contains_key(MARKER) => return Ok(value),
        (Value::Object(_), _) => CONFIG,
        (Value::Array(_), Some("agent_presets.json")) => PRESETS,
        (Value::Array(_), Some("ssh_hosts.json")) => HOSTS,
        (Value::Array(_), _) => bail!(
            "a JSON list could be presets or ssh hosts — name the file agent_presets.json or \
             ssh_hosts.json, or import the folder holding it"
        ),
        _ => bail!("not a settings file: expected a JSON object or list"),
    };
    let mut bundle = bare_bundle();
    bundle.insert(section.into(), value);
    Ok(Value::Object(bundle))
}

fn bare_bundle() -> Object {
    Object::from_iter([(MARKER.to_string(), json!(FORMAT))])
}

/// `bundle` as `NEBULA_IMPORT_BUNDLE` carries it: compact JSON in standard
/// base64, an alphabet that survives single quotes in every login shell —
/// fish reads a `\` inside them, and JSON is full of backslashes.
pub fn encode(bundle: &Value) -> String {
    base64::engine::general_purpose::STANDARD.encode(bundle.to_string())
}

pub fn decode(encoded: &str) -> Result<Value> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("not base64")?;
    serde_json::from_slice(&bytes).context("not JSON")
}

/// The encoded bundle `nebula ssh` / `nebula tunnel` send along, or `None`
/// when the `ssh_sync_config` setting is off, there is nothing to send, or it
/// is too big for a command line — which is said on stderr, as is a file
/// that couldn't be read. Never stops the connection.
pub fn for_remote() -> Option<String> {
    if !crate::config::Config::load().ssh_sync_config {
        return None;
    }
    let (bundle, warnings) = export(&Paths::current(), Scope::Remote);
    for warning in warnings {
        eprintln!("nebula: settings not sent in full: {warning}");
    }
    if section_count(&bundle) == 0 {
        return None;
    }
    let encoded = encode(&bundle);
    if encoded.len() > MAX_FORWARD_BYTES {
        eprintln!(
            "nebula: settings are {} KiB, too large to send over ssh — connecting without them",
            encoded.len() / 1024
        );
        return None;
    }
    Some(encoded)
}

/// Merge the bundle a `nebula ssh` / `nebula tunnel` from another machine
/// sent in `NEBULA_IMPORT_BUNDLE`, if any, and take the variable out of the
/// environment so the daemon, agent sessions and ttyd's TUIs never inherit
/// it. Call it first thing, before a thread exists. Says what changed on
/// stderr (the TUI's screen covers that line, and restores it on quit) and
/// never fails the launch.
pub fn apply_forwarded() {
    let Some(encoded) = std::env::var_os(nebula_core::env::IMPORT_BUNDLE) else {
        return;
    };
    std::env::remove_var(nebula_core::env::IMPORT_BUNDLE);
    let encoded = encoded.to_string_lossy();
    if encoded.is_empty() {
        return;
    }
    match decode(&encoded).and_then(|bundle| import(&Paths::current(), &bundle)) {
        Ok(report) if report.changed() || !report.held_local.is_empty() => eprintln!(
            "nebula: settings from the connecting machine applied — {}",
            report.summary()
        ),
        Ok(_) => {}
        Err(err) => eprintln!("nebula: ignored the settings the connecting machine sent: {err:#}"),
    }
}

/// `nebula config <command>`, as the CLI parsed it.
#[derive(Debug, Clone)]
pub enum ConfigOp {
    Path,
    Export { path: Option<String> },
    Import { source: String },
}

pub fn run(op: ConfigOp) -> Result<()> {
    let paths = Paths::current();
    match op {
        ConfigOp::Path => {
            for (label, path) in [
                ("config.json", &paths.config),
                ("config.local.json", &paths.local),
                ("agent_presets.json", &paths.presets),
                ("ssh_hosts.json", &paths.hosts),
            ] {
                println!("{label:<20}{}", path.display());
            }
            Ok(())
        }
        ConfigOp::Export { path } => {
            let (bundle, warnings) = export(&paths, Scope::Backup);
            for warning in &warnings {
                eprintln!("nebula: {warning}");
            }
            let mut text = serde_json::to_string_pretty(&bundle)?;
            text.push('\n');
            let Some(dest) = path.as_deref().filter(|p| *p != "-") else {
                return std::io::stdout()
                    .lock()
                    .write_all(text.as_bytes())
                    .context("writing to stdout");
            };
            let dest = Path::new(dest);
            let file = if dest.is_dir() {
                dest.join(FILE_NAME)
            } else {
                dest.to_path_buf()
            };
            settings::write_atomic(&file, text.as_bytes())
                .with_context(|| format!("writing {}", file.display()))?;
            let carried: Vec<&str> = SECTIONS
                .iter()
                .filter(|(section, _)| bundle.get(*section).is_some())
                .map(|(_, name)| *name)
                .collect();
            println!(
                "settings written to {} ({})",
                file.display(),
                if carried.is_empty() {
                    "no settings files yet".into()
                } else {
                    carried.join(", ")
                }
            );
            Ok(())
        }
        ConfigOp::Import { source } => {
            let bundle = read_source(&source)?;
            let report = import(&paths, &bundle)?;
            println!("{}", report.summary());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config: dir.join("config.json"),
            local: dir.join("config.local.json"),
            presets: dir.join("agent_presets.json"),
            hosts: dir.join("ssh_hosts.json"),
        }
    }

    fn put(path: &Path, value: Value) {
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn get(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn an_export_carries_the_files_as_written_and_never_the_local_layer() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        put(
            &p.config,
            json!({"theme": "ocean", "from_a_newer_nebula": {"x": 1}}),
        );
        put(&p.local, json!({"editor": "nano"}));
        put(
            &p.presets,
            json!([{"name": "future", "kind": "antigravity"}]),
        );
        put(&p.hosts, json!([{"host": "a@b"}]));

        let (bundle, warnings) = export(&p, Scope::Backup);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(bundle[MARKER], FORMAT);
        assert_eq!(
            bundle["config"],
            json!({"theme": "ocean", "from_a_newer_nebula": {"x": 1}})
        );
        assert_eq!(bundle["agent_presets"][0]["kind"], "antigravity");
        assert_eq!(bundle["ssh_hosts"][0]["host"], "a@b");
        assert!(
            !bundle.to_string().contains("nano"),
            "the local layer stays"
        );

        let (remote, _) = export(&p, Scope::Remote);
        assert!(
            remote.get("ssh_hosts").is_none(),
            "destinations as seen from here stay here"
        );
        assert_eq!(section_count(&remote), 2);
    }

    #[test]
    fn missing_files_are_no_section_and_a_broken_one_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let (bundle, warnings) = export(&p, Scope::Backup);
        assert_eq!(section_count(&bundle), 0);
        assert!(warnings.is_empty());

        std::fs::write(&p.config, "{ not json").unwrap();
        let (bundle, warnings) = export(&p, Scope::Backup);
        assert_eq!(section_count(&bundle), 0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("config.json"), "{warnings:?}");
    }

    #[test]
    fn an_import_patches_the_keys_it_carries_and_leaves_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        put(&p.config, json!({"theme": "default", "animations": false}));
        put(&p.local, json!({"theme": "rose"}));
        let bundle = json!({
            "nebula_bundle": 1,
            "exported_by": "9.9.9",
            "config": {"theme": "ocean", "focus_tint": false},
            "projects": [],
        });

        let report = import(&p, &bundle).unwrap();
        assert_eq!(
            get(&p.config),
            json!({"theme": "ocean", "animations": false, "focus_tint": false})
        );
        assert_eq!(report.config_changed, 2);
        assert_eq!(report.held_local, vec!["theme"]);
        assert_eq!(report.unknown_sections, vec!["projects"]);
        assert_eq!(get(&p.local), json!({"theme": "rose"}), "never written");
        assert!(report.summary().contains("config: 2 keys changed"));
        assert!(!p.presets.exists() && !p.hosts.exists());

        let again = import(&p, &bundle).unwrap();
        assert!(!again.changed(), "{again:?}");
    }

    #[test]
    fn presets_merge_by_name_and_hosts_by_destination() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        put(
            &p.presets,
            json!([{"name": "Reviewer", "kind": "claude"}, {"name": "mine", "kind": "codex"}]),
        );
        put(
            &p.hosts,
            json!([{"host": "a@one", "last_used_ms": 5}, {"host": "b@two", "last_used_ms": 1}]),
        );
        let bundle = json!({
            "nebula_bundle": 1,
            "agent_presets": [
                {"name": "reviewer", "kind": "antigravity"},
                {"name": "new", "kind": "pi"},
                {"kind": "claude"},
            ],
            "ssh_hosts": [
                {"host": "b@two", "last_used_ms": 9},
                {"host": "a@one", "last_used_ms": 2},
                {"host": "c@three", "path": "/srv", "last_used_ms": 3},
            ],
        });

        let report = import(&p, &bundle).unwrap();
        let presets = get(&p.presets);
        let names: Vec<&str> = presets
            .as_array()
            .unwrap()
            .iter()
            .filter_map(crate::agent_presets::entry_name)
            .collect();
        assert_eq!(names, ["reviewer", "mine", "new"]);
        assert_eq!(presets[0]["kind"], "antigravity");
        assert_eq!((report.presets_added, report.presets_replaced), (1, 1));

        let hosts = get(&p.hosts);
        let order: Vec<&str> = hosts
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["host"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["b@two", "a@one", "c@three"]);
        assert_eq!(hosts[1]["last_used_ms"], 5, "the newer stamp is kept");
        assert_eq!(report.hosts_added, 1);
    }

    #[test]
    fn an_unreadable_target_fails_the_import_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        put(&p.presets, json!([]));
        std::fs::write(&p.config, "{ broken").unwrap();
        let bundle = json!({
            "nebula_bundle": 1,
            "config": {"theme": "ocean"},
            "agent_presets": [{"name": "p"}],
        });
        let err = import(&p, &bundle).unwrap_err();
        assert!(err.to_string().contains("config.json"), "{err}");
        assert_eq!(get(&p.presets), json!([]), "the presets were not written");
        assert_eq!(std::fs::read_to_string(&p.config).unwrap(), "{ broken");

        let err = import(&p, &json!({"theme": "ocean"})).unwrap_err();
        assert!(err.to_string().contains("not a nebula settings bundle"));
    }

    #[test]
    fn import_takes_a_bundle_a_bare_file_or_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = |name: &str| dir.path().join(name).display().to_string();

        put(
            &dir.path().join("backup.json"),
            json!({"nebula_bundle": 1, "config": {"theme": "ocean"}}),
        );
        assert_eq!(
            read_source(&src("backup.json")).unwrap()["config"]["theme"],
            "ocean"
        );

        put(
            &dir.path().join("my-settings.json"),
            json!({"theme": "forest"}),
        );
        let bundle = read_source(&src("my-settings.json")).unwrap();
        assert_eq!(
            bundle["config"]["theme"], "forest",
            "an object is a config.json"
        );

        put(&dir.path().join("ssh_hosts.json"), json!([{"host": "a@b"}]));
        assert_eq!(
            read_source(&src("ssh_hosts.json")).unwrap()["ssh_hosts"][0]["host"],
            "a@b"
        );
        put(&dir.path().join("list.json"), json!([]));
        let err = read_source(&src("list.json")).unwrap_err();
        assert!(err.to_string().contains("presets or ssh hosts"), "{err}");

        let folder = dir.path().join("dotfiles");
        std::fs::create_dir(&folder).unwrap();
        let err = read_source(&folder.display().to_string()).unwrap_err();
        assert!(err.to_string().contains("holds none"), "{err}");
        put(&folder.join("config.json"), json!({"theme": "amber"}));
        put(&folder.join("config.local.json"), json!({"editor": "nano"}));
        put(&folder.join("agent_presets.json"), json!([{"name": "p"}]));
        let bundle = read_source(&folder.display().to_string()).unwrap();
        assert_eq!(bundle["config"], json!({"theme": "amber"}));
        assert_eq!(bundle["agent_presets"][0]["name"], "p");
        assert!(
            !bundle.to_string().contains("nano"),
            "never the local layer"
        );

        put(
            &folder.join(FILE_NAME),
            json!({"nebula_bundle": 1, "config": {"theme": "rose"}}),
        );
        let bundle = read_source(&folder.display().to_string()).unwrap();
        assert_eq!(bundle["config"], json!({"theme": "rose"}), "an export wins");
    }

    #[test]
    fn the_environment_form_round_trips_and_needs_no_escaping() {
        let bundle = json!({
            "nebula_bundle": 1,
            "agent_presets": [{"name": "p", "prefix": "line one\nsay \"hi\" \\ at 5 o'clock"}],
        });
        let encoded = encode(&bundle);
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
            "{encoded}"
        );
        assert_eq!(decode(&encoded).unwrap(), bundle);
        assert!(decode("%%% not base64").is_err());
    }
}
