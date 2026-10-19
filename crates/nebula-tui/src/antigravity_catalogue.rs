//! Runtime model ids from `agy models`, cached for one day and refreshed in the background.
use crate::config::DEFAULT_CHOICE;
use nebula_core::clock::now_secs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_FILE: &str = "antigravity_models.json";
static CURRENT: OnceLock<RwLock<&'static [&'static str]>> = OnceLock::new();

/// Keep complete ids: their suffixes are not Cursor-style effort variants.
pub fn parse_models(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text.lines() {
        let Some((id, label)) = line.trim().split_once('\t') else {
            continue;
        };
        let id = id.trim();
        if !id.is_empty()
            && !id.contains(char::is_whitespace)
            && !label.trim().is_empty()
            && !ids.iter().any(|existing| existing == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

pub fn models() -> &'static [&'static str] {
    *CURRENT
        .get_or_init(|| RwLock::new(&[DEFAULT_CHOICE]))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn install(ids: &[String]) {
    let mut models = vec![DEFAULT_CHOICE];
    for id in ids {
        if id != DEFAULT_CHOICE && !models.contains(&id.as_str()) {
            models.push(Box::leak(id.clone().into_boxed_str()));
        }
    }
    let installed = Box::leak(models.into_boxed_slice());
    *CURRENT
        .get_or_init(|| RwLock::new(&[DEFAULT_CHOICE]))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = installed;
}

// --- cache and refresh -------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    fetched_at: u64,
    ids: Vec<String>,
}

fn cache_path() -> PathBuf {
    nebula_core::paths::data_dir().join(CACHE_FILE)
}

fn read_cache(path: &std::path::Path) -> Option<Cache> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: &std::path::Path, ids: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let cache = Cache {
        fetched_at: now_secs(),
        ids: ids.to_vec(),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&cache)?)?;
    std::fs::rename(tmp, path)
}

/// Run `agy models` and parse it; None when the CLI is
/// missing, fails (not logged in), or lists nothing.
pub fn fetch_ids() -> Option<Vec<String>> {
    let output = Command::new(nebula_core::AgentKind::Antigravity.cli_program())
        .arg("models")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ids = parse_models(&String::from_utf8_lossy(&output.stdout));
    (!ids.is_empty()).then_some(ids)
}

/// TUI startup: install the cached catalogue at once and, when the cache is
/// missing or older than [`CACHE_TTL`], refresh it on a background thread.
/// Skipped when Antigravity is switched off (HARNESS TOGGLE) and under
/// `NEBULA_AGENT_CMD` (tests, STUB AGENTS), so no test ever shells out.
pub fn bootstrap(antigravity_enabled: bool) {
    if !antigravity_enabled || nebula_core::env::non_empty(nebula_core::env::AGENT_CMD).is_some() {
        return;
    }
    let path = cache_path();
    let cached = read_cache(&path);
    if let Some(cache) = &cached {
        install(&cache.ids);
    }
    let fresh =
        cached.is_some_and(|c| now_secs().saturating_sub(c.fetched_at) < CACHE_TTL.as_secs());
    if fresh {
        return;
    }
    std::thread::spawn(move || {
        if let Some(ids) = fetch_ids() {
            if let Err(err) = write_cache(&path, &ids) {
                tracing::warn!("antigravity catalogue cache not written: {err}");
            }
            install(&ids);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_tab_separated_ids_without_splitting_effort_suffixes() {
        assert_eq!(
            parse_models(
                "Fetching available models...\n gemini-4-argon-high\tGemini 4 Argon (High)\r\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\ngemini-4-argon-high\tDuplicate\n"
            ),
            vec!["gemini-4-argon-high", "claude-opus-5-5-low"]
        );
    }
    #[test]
    fn ignores_headers_blank_and_malformed_lines() {
        assert!(parse_models(
            "Fetching available models...\n\nno tabs\n\tLabel\nbad id\tLabel\nid\t   \n"
        )
        .is_empty());
        assert!(parse_models("").is_empty());
    }
    #[test]
    fn cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(CACHE_FILE);
        assert!(read_cache(&path).is_none());
        write_cache(&path, &["gemini-4-argon-high".into()]).unwrap();
        let cache = read_cache(&path).unwrap();
        assert_eq!(cache.ids, vec!["gemini-4-argon-high"]);
        assert!(now_secs().saturating_sub(cache.fetched_at) < CACHE_TTL.as_secs());
    }
}
