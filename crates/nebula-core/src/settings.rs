//! The settings files both halves of nebula read, and the rules that keep
//! them readable across releases.
//!
//! Settings come in two layers, merged key by key: `config.json`, the
//! portable file — what a backup or `nebula ssh` carries to another machine,
//! and what `NEBULA_CONFIG_FILE` can move into a dotfiles checkout — and
//! `config.local.json` beside the database, for what only makes sense on
//! this machine. The local layer wins, and is never exported, forwarded or
//! overwritten by an import. The TUI's `Config` and the daemon's each
//! deserialize their own fields out of the merged object.
//!
//! Two nebula releases routinely share these files: a remote a few versions
//! behind the machine that sshes into it, a backup restored into a newer
//! build. So a key one build cannot read must cost only that key.
//! [`parse_lenient`] drops what fails and keeps everything else, where a
//! whole-file parse turns one mistyped value into every default — and the
//! next save writes those defaults over the lot.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// A JSON object, as the settings files hold one.
pub type Object = Map<String, Value>;

/// The merged settings, and what went into them.
#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    /// Top-level keys `T` could not read, from either layer. They take
    /// their defaults in `value`; a writer should leave their stored JSON
    /// alone rather than write that default back over it.
    pub skipped: BTreeSet<String>,
    /// A layer that exists but could not be used, one line each. That layer
    /// is ignored as a whole; the other still applies.
    pub problems: Vec<String>,
}

/// Read both layers and deserialize `T` from their merge, `local` over
/// `config`. `T` must be `#[serde(default)]`: a missing key is its default.
pub fn load<T: DeserializeOwned + Default>(config: &Path, local: &Path) -> Loaded<T> {
    let mut problems = Vec::new();
    let mut merged = read_layer(config, &mut problems);
    merged.extend(read_layer(local, &mut problems));
    let (value, skipped) = parse_lenient(&merged);
    Loaded {
        value,
        skipped,
        problems,
    }
}

fn read_layer(path: &Path, problems: &mut Vec<String>) -> Object {
    read_object(path)
        .unwrap_or_else(|err| {
            problems.push(format!("ignoring {}: {err}", path.display()));
            None
        })
        .unwrap_or_default()
}

/// `T` out of `obj`, one top-level key at a time: a key whose value `T`
/// rejects is left out, so it takes its default, and is named in the second
/// half. Keys `T` has no field for are ignored, as serde always ignores them.
pub fn parse_lenient<T: DeserializeOwned + Default>(obj: &Object) -> (T, BTreeSet<String>) {
    if let Ok(value) = serde_json::from_value(Value::Object(obj.clone())) {
        return (value, BTreeSet::new());
    }
    let mut skipped: BTreeSet<String> = obj
        .iter()
        .filter(|(key, value)| {
            let single = Object::from_iter([((*key).clone(), (*value).clone())]);
            serde_json::from_value::<T>(Value::Object(single)).is_err()
        })
        .map(|(key, _)| key.clone())
        .collect();
    let readable: Object = obj
        .iter()
        .filter(|(key, _)| !skipped.contains(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Ok(value) = serde_json::from_value(Value::Object(readable.clone())) {
        return (value, skipped);
    }
    // Every key reads alone, yet not all together: two of them clash — a
    // field under its name and under its `alias` both, the old key an
    // older build wrote still beside the new one. Add the keys back one at
    // a time and leave out each that breaks what already reads, so the
    // clash costs the later key of the pair rather than every value.
    let mut kept = Object::new();
    for (key, value) in readable {
        kept.insert(key.clone(), value);
        if serde_json::from_value::<T>(Value::Object(kept.clone())).is_err() {
            kept.remove(&key);
            skipped.insert(key);
        }
    }
    let value = serde_json::from_value(Value::Object(kept)).unwrap_or_default();
    (value, skipped)
}

/// A JSON object file: `Ok(None)` when there is no file, `Err` when it can't
/// be read or holds something other than an object.
pub fn read_object(path: &Path) -> Result<Option<Object>, String> {
    match read_json(path)? {
        None => Ok(None),
        Some(Value::Object(obj)) => Ok(Some(obj)),
        Some(_) => Err("not a JSON object".into()),
    }
}

/// A JSON list file, on the same terms as [`read_object`].
pub fn read_array(path: &Path) -> Result<Option<Vec<Value>>, String> {
    match read_json(path)? {
        None => Ok(None),
        Some(Value::Array(entries)) => Ok(Some(entries)),
        Some(_) => Err("not a JSON list".into()),
    }
}

fn read_json(path: &Path) -> Result<Option<Value>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|err| format!("not valid JSON ({err})"))
}

/// A list file read entry by entry: the entries `T` reads, then the raw ones
/// it doesn't — most likely a newer nebula's, and worth keeping on the next
/// save. A missing or unreadable file reads as empty: the list stores are
/// conveniences, never load-bearing.
pub fn read_list<T: DeserializeOwned>(path: &Path) -> (Vec<T>, Vec<Value>) {
    let mut parsed = Vec::new();
    let mut unread = Vec::new();
    for entry in read_array(path).ok().flatten().unwrap_or_default() {
        match serde_json::from_value::<T>(entry.clone()) {
            Ok(value) => parsed.push(value),
            Err(_) => unread.push(entry),
        }
    }
    (parsed, unread)
}

/// Pretty-print `value` into `path` through [`write_atomic`], ending in a
/// newline.
pub fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Replace a file's contents atomically: write `<name>.tmp` beside it and
/// rename that into place, so a crash mid-write never leaves half a file. A
/// symlink at `path` is written through rather than replaced — a dotfiles
/// setup links `config.json` into a repo, and the rename would otherwise
/// swap the link for a plain file (#50).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_links(path);
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name = target.file_name().map(OsString::from).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = target.with_file_name(tmp_name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &target)
}

/// `path` followed through any symlinks to the file they name, which need
/// not exist yet. Gives up after 40 hops — the kernel's own loop bound — and
/// leaves a loop to fail at the write.
fn resolve_links(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        let is_link = std::fs::symlink_metadata(&current)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        let Some(link) = is_link.then(|| std::fs::read_link(&current).ok()).flatten() else {
            break;
        };
        current = match current.parent() {
            Some(parent) if link.is_relative() => parent.join(link),
            _ => link,
        };
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(default)]
    struct Sample {
        on: bool,
        count: u32,
        /// Renamed: a file an older build wrote says `old_name`.
        #[serde(alias = "old_name")]
        name: String,
    }

    impl Default for Sample {
        fn default() -> Self {
            Self {
                on: true,
                count: 3,
                name: "x".into(),
            }
        }
    }

    fn object(value: Value) -> Object {
        value.as_object().expect("an object").clone()
    }

    #[test]
    fn one_unreadable_key_costs_only_that_key() {
        let obj = object(json!({
            "on": false,
            "count": "three",
            "name": "y",
            "a_newer_key": [1, 2],
        }));
        let (sample, skipped) = parse_lenient::<Sample>(&obj);
        assert_eq!(
            sample,
            Sample {
                on: false,
                count: 3,
                name: "y".into()
            },
            "the mistyped count takes its default, the rest keep their values"
        );
        assert_eq!(skipped, BTreeSet::from(["count".to_string()]));

        let (_, skipped) = parse_lenient::<Sample>(&object(json!({"on": false})));
        assert!(skipped.is_empty(), "a readable object skips nothing");
    }

    /// A file holding a field under both its name and its old alias —
    /// the old key left behind by an older build, the new one patched in
    /// beside it — is a `duplicate field` to serde as a whole, though each
    /// key reads fine alone. That must cost the one key, not every value.
    #[test]
    fn a_renamed_key_beside_its_old_name_costs_only_the_old_name() {
        let obj = object(json!({
            "on": false,
            "count": 7,
            "name": "new",
            "old_name": "stale",
        }));
        let (sample, skipped) = parse_lenient::<Sample>(&obj);
        assert_eq!(
            sample,
            Sample {
                on: false,
                count: 7,
                name: "new".into()
            },
            "the other values survive the clash"
        );
        assert_eq!(skipped, BTreeSet::from(["old_name".to_string()]));

        let (sample, skipped) = parse_lenient::<Sample>(&object(json!({"old_name": "old"})));
        assert_eq!(sample.name, "old", "the old name alone still reads");
        assert!(skipped.is_empty());
    }

    #[test]
    fn the_local_layer_wins_key_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let (config, local) = (
            dir.path().join("config.json"),
            dir.path().join("config.local.json"),
        );
        std::fs::write(&config, r#"{"on": false, "count": 5}"#).unwrap();
        std::fs::write(&local, r#"{"count": 9}"#).unwrap();
        let loaded = load::<Sample>(&config, &local);
        assert_eq!(
            loaded.value,
            Sample {
                on: false,
                count: 9,
                name: "x".into()
            }
        );
        assert!(loaded.skipped.is_empty() && loaded.problems.is_empty());
    }

    #[test]
    fn a_broken_layer_is_reported_and_the_other_still_applies() {
        let dir = tempfile::tempdir().unwrap();
        let (config, local) = (
            dir.path().join("config.json"),
            dir.path().join("config.local.json"),
        );
        std::fs::write(&config, "{ not json").unwrap();
        std::fs::write(&local, r#"{"count": 9}"#).unwrap();
        let loaded = load::<Sample>(&config, &local);
        assert_eq!(loaded.value.count, 9);
        assert!(loaded.value.on, "the broken layer's keys are defaults");
        assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
        assert!(loaded.problems[0].contains("config.json"));

        std::fs::write(&local, "[1, 2]").unwrap();
        let loaded = load::<Sample>(&config, &local);
        assert_eq!(loaded.value, Sample::default());
        assert_eq!(loaded.problems.len(), 2, "{:?}", loaded.problems);
    }

    #[test]
    fn missing_layers_are_all_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load::<Sample>(&dir.path().join("a.json"), &dir.path().join("b.json"));
        assert_eq!(loaded.value, Sample::default());
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn a_write_goes_through_a_symlink_instead_of_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("nebula.json"), "{}\n").unwrap();
        let link = dir.path().join("config.json");
        std::os::unix::fs::symlink("dotfiles/nebula.json", &link).unwrap();

        write_json(&link, &json!({"theme": "ocean"})).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link survives the save"
        );
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(real.join("nebula.json")).unwrap())
                .unwrap();
        assert_eq!(saved["theme"], "ocean", "the file it names took the write");
        assert!(!real.join("nebula.json.tmp").exists());
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn a_write_creates_missing_directories_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("list.json");
        write_atomic(&path, b"[]\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]\n");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_list_keeps_the_entries_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.json");
        std::fs::write(&path, r#"[{"count": 1}, {"count": "many"}, {"on": false}]"#).unwrap();
        let (parsed, unread) = read_list::<Sample>(&path);
        assert_eq!(parsed.len(), 2);
        assert_eq!(unread, vec![json!({"count": "many"})]);

        std::fs::write(&path, "not json").unwrap();
        let (parsed, unread) = read_list::<Sample>(&path);
        assert!(parsed.is_empty() && unread.is_empty());
    }
}
