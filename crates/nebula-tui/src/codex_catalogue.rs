//! Codex owns its model catalogue. Hydrate from its local cache for an
//! immediate picker, then ask `codex app-server` for `model/list` in the
//! background and periodically while Nebula stays open. No model ids or
//! reasoning levels are inferred from model names.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use nebula_core::harness::{HarnessCatalog, HarnessDescriptor};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::DEFAULT_CHOICE;

const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Model {
    id: String,
    efforts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Catalogue(Vec<Model>);

impl Catalogue {
    // Both Codex's cache and the RPC carry capability strings; preserve
    // unknown future levels and the server's ordering verbatim.
    fn parse(entries: &[Value], cached: bool) -> Self {
        let mut models = Vec::<Model>::new();
        for entry in entries {
            if entry.get("hidden").and_then(Value::as_bool) == Some(true)
                || entry
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v != "list")
            {
                continue;
            }
            let Some(id) = entry
                .get(if cached { "slug" } else { "model" })
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty() && !id.eq_ignore_ascii_case(DEFAULT_CHOICE))
            else {
                continue;
            };
            if models.iter().any(|m| m.id.eq_ignore_ascii_case(id)) {
                continue;
            }
            let mut efforts = Vec::<String>::new();
            if let Some(levels) = entry
                .get(if cached {
                    "supported_reasoning_levels"
                } else {
                    "supportedReasoningEfforts"
                })
                .and_then(Value::as_array)
            {
                for level in levels {
                    if let Some(effort) = level
                        .get(if cached { "effort" } else { "reasoningEffort" })
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(DEFAULT_CHOICE))
                    {
                        if !efforts.iter().any(|e| e.eq_ignore_ascii_case(effort)) {
                            efforts.push(effort.into());
                        }
                    }
                }
            }
            models.push(Model {
                id: id.into(),
                efforts,
            });
        }
        Self(models)
    }

    fn models(&self) -> Vec<String> {
        std::iter::once(DEFAULT_CHOICE.to_string())
            .chain(self.0.iter().map(|m| m.id.clone()))
            .collect()
    }

    fn efforts(&self, model: Option<&str>) -> Option<Vec<String>> {
        // `isDefault` is the catalog's recommendation, not necessarily
        // the user's configured CLI model. With no explicit model, let
        // the descriptor supply conservative fallback effort choices.
        let selected = model
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(DEFAULT_CHOICE))?;
        self.0
            .iter()
            .find(|m| m.id.eq_ignore_ascii_case(selected))
            .map(|m| m.efforts.clone())
    }
}

static CURRENT: OnceLock<RwLock<Catalogue>> = OnceLock::new();

fn current() -> &'static RwLock<Catalogue> {
    CURRENT.get_or_init(|| RwLock::new(Catalogue::default()))
}

pub fn models() -> Vec<String> {
    with_current(Catalogue::models)
}

/// None means an unknown model: keep the descriptor's fallback effort
/// list. Some(empty) means the model explicitly offers no reasoning.
pub fn efforts(model: Option<&str>) -> Option<Vec<String>> {
    with_current(|catalogue| catalogue.efforts(model))
}

fn with_current<T>(f: impl FnOnce(&Catalogue) -> T) -> T {
    #[cfg(test)]
    if let Some(catalogue) = TEST_CATALOGUE.with(|c| c.borrow().clone()) {
        return f(&catalogue);
    }
    f(&current().read().unwrap_or_else(|p| p.into_inner()))
}

fn install(catalogue: Catalogue) {
    *current().write().unwrap_or_else(|p| p.into_inner()) = catalogue;
}

#[cfg(test)]
thread_local! {
    static TEST_CATALOGUE: std::cell::RefCell<Option<Catalogue>> = const { std::cell::RefCell::new(None) };
}

/// Keep UI tests independent of both the installed CLI and parallel tests.
#[cfg(test)]
pub(crate) fn with_test_catalogue<T>(entries: &[Value], f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Catalogue>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_CATALOGUE.with(|c| c.replace(self.0.take()));
        }
    }
    let _restore =
        Restore(TEST_CATALOGUE.with(|c| c.replace(Some(Catalogue::parse(entries, false)))));
    f()
}

fn read_cache(home: &Path) -> Option<Catalogue> {
    let doc: Value =
        serde_json::from_slice(&std::fs::read(home.join("models_cache.json")).ok()?).ok()?;
    let catalogue = Catalogue::parse(doc.get("models")?.as_array()?, true);
    (!catalogue.0.is_empty()).then_some(catalogue)
}

async fn rpc(
    input: &mut tokio::process::ChildStdin,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    input
        .write_all(
            format!(
                "{}\n",
                json!({"id": id, "method": method, "params": params})
            )
            .as_bytes(),
        )
        .await?;
    input.flush().await?;
    while let Some(line) = output.next_line().await? {
        let message: Value = serde_json::from_str(&line).context("invalid app-server response")?;
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue; // Notifications may arrive between responses.
        }
        if message.get("error").is_some() {
            bail!("Codex {method} failed");
        }
        return message
            .get("result")
            .cloned()
            .context("missing app-server result");
    }
    bail!("Codex closed the model catalog connection")
}

async fn fetch(command: &mut tokio::process::Command, timeout: Duration) -> Result<Catalogue> {
    let mut child = command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start Codex model discovery")?;
    let result = tokio::time::timeout(timeout, async {
        let mut input = child.stdin.take().context("missing Codex stdin")?;
        let mut output = BufReader::new(child.stdout.take().context("missing Codex stdout")?).lines();
        rpc(&mut input, &mut output, 0, "initialize", json!({
            "clientInfo": {"name": "nebula", "title": "Nebula", "version": env!("CARGO_PKG_VERSION")}
        })).await?;
        input.write_all(b"{\"method\":\"initialized\",\"params\":{}}\n").await?;
        let mut entries = Vec::new();
        let mut cursor = None::<String>;
        let mut seen = HashSet::new();
        for id in 1.. {
            let page = rpc(&mut input, &mut output, id, "model/list", json!({
                "limit": 100, "cursor": cursor, "includeHidden": false
            })).await?;
            entries.extend(page.get("data").and_then(Value::as_array)
                .context("missing Codex model list")?.iter().cloned());
            cursor = page.get("nextCursor").and_then(Value::as_str).map(String::from);
            match &cursor {
                None => break,
                Some(next) if !seen.insert(next.clone()) => bail!("repeated Codex model cursor"),
                _ => {}
            }
        }
        let catalogue = Catalogue::parse(&entries, false);
        if catalogue.0.is_empty() {
            bail!("Codex returned no visible models");
        }
        Ok(catalogue)
    }).await.unwrap_or_else(|_| Err(anyhow!("Codex model discovery timed out")));
    // Reap on success, malformed output, disconnect and timeout alike.
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

/// Uses the same CLI program and CODEX_HOME as launched sessions. Never
/// opens a conversation or starts inference. A failed refresh preserves
/// the last good catalogue; without one, `default` still launches Codex.
pub fn bootstrap(descriptor: &HarnessDescriptor) {
    if !descriptor.enabled
        || descriptor.model.catalog != Some(HarnessCatalog::Codex)
        || nebula_core::env::non_empty(nebula_core::env::AGENT_CMD).is_some()
    {
        return;
    }
    let home = nebula_core::env::non_empty(nebula_core::env::CODEX_HOME)
        .map(std::path::PathBuf::from)
        .or_else(|| nebula_core::env::home_dir().map(|p| p.join(".codex")));
    if let Some(catalogue) = home.as_deref().and_then(read_cache) {
        install(catalogue);
    }
    let program = descriptor.program.clone();
    tokio::spawn(async move {
        loop {
            match fetch(&mut tokio::process::Command::new(&program), FETCH_TIMEOUT).await {
                Ok(catalogue) => install(catalogue),
                Err(err) => tracing::warn!("Codex model catalog refresh failed: {err:#}"),
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_choices_and_settings_follow_model_capabilities() {
        with_test_catalogue(
            &[
                json!({"model": "future-reasoner", "supportedReasoningEfforts": [
                {"reasoningEffort": "ultra"}, {"reasoningEffort": "future-effort"}]}),
                json!({"model": "future-direct", "supportedReasoningEfforts": []}),
            ],
            || {
                let mut descriptor = nebula_core::harness::builtin("codex").unwrap();
                assert_eq!(
                    crate::config::model_choices_in(&descriptor),
                    ["default", "future-reasoner", "future-direct"]
                );
                assert_eq!(
                    crate::config::effort_choices_in(&descriptor, Some("future-reasoner")),
                    ["default", "ultra", "future-effort"]
                );
                assert!(
                    crate::config::effort_choices_in(&descriptor, Some("future-direct")).is_empty()
                );
                assert_eq!(
                    crate::config::fit_effort_in(
                        &descriptor,
                        Some("future-reasoner"),
                        Some("ultra".into())
                    ),
                    Some("ultra".into())
                );
                assert_eq!(
                    crate::config::fit_effort_in(
                        &descriptor,
                        Some("future-reasoner"),
                        Some("minimal".into())
                    ),
                    None
                );
                let mut cfg = crate::config::Config {
                    codex_model: "future-reasoner".into(),
                    codex_effort: "ultra".into(),
                    ..Default::default()
                };
                cfg.cycle_agent_row("codex", crate::config::HarnessField::Model, 1);
                assert_eq!(cfg.codex_model, "future-direct");
                assert_eq!(cfg.codex_effort, DEFAULT_CHOICE);
                assert_eq!(cfg.default_effort(nebula_core::AgentKind::Codex), None);
                // Users can explicitly opt out of discovery for a custom provider.
                descriptor.model.catalog = None;
                descriptor.model.models = vec![nebula_core::harness::ModelEntry {
                    id: "custom/model".into(),
                    hint: None,
                }];
                assert_eq!(
                    crate::config::model_choices_in(&descriptor),
                    ["default", "custom/model"]
                );
            },
        );
    }

    #[test]
    fn discovers_future_models_and_efforts_without_name_rules() {
        let catalogue = Catalogue::parse(
            &[
                json!({"model": "future-model", "isDefault": true,
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low"}, {"reasoningEffort": "ultra"},
                    {"reasoningEffort": "future-effort"}, {"reasoningEffort": "low"}]}),
                json!({"model": "hidden-model", "hidden": true}),
                json!({"model": "no-reasoning", "supportedReasoningEfforts": []}),
                json!({"model": "FUTURE-MODEL"}),
                json!({"model": " "}),
                json!({"id": "missing-model-field"}),
            ],
            false,
        );
        assert_eq!(
            catalogue.models(),
            ["default", "future-model", "no-reasoning"]
        );
        assert_eq!(
            catalogue.efforts(Some("FUTURE-model")).unwrap(),
            ["low", "ultra", "future-effort"]
        );
        assert_eq!(catalogue.efforts(None), catalogue.efforts(Some("default")));
        assert_eq!(catalogue.efforts(None), None);
        assert_eq!(catalogue.efforts(Some("no-reasoning")), Some(vec![]));
        assert_eq!(catalogue.efforts(Some("custom-provider/model")), None);
        assert_eq!(Catalogue::default().models(), ["default"]);
    }

    #[test]
    fn cache_uses_codex_visibility_and_capabilities() {
        let home = tempfile::tempdir().unwrap();
        assert!(read_cache(home.path()).is_none());
        let path = home.path().join("models_cache.json");
        std::fs::write(&path, "invalid").unwrap();
        assert!(read_cache(home.path()).is_none());
        std::fs::write(
            &path,
            json!({"models": [
                {"slug": "next-generation", "visibility": "list",
                    "supported_reasoning_levels": [{"effort": "max"}, {"effort": "ultra"}]},
                {"slug": "internal", "visibility": "hide"}
            ]})
            .to_string(),
        )
        .unwrap();
        let catalogue = read_cache(home.path()).unwrap();
        assert_eq!(catalogue.models(), ["default", "next-generation"]);
        assert_eq!(
            catalogue.efforts(Some("next-generation")).unwrap(),
            ["max", "ultra"]
        );
        assert_eq!(
            catalogue.efforts(None),
            None,
            "cache does not guess the configured default"
        );
    }

    // Fake app-server runs only a shell, never the installed Codex or its
    // credentials. Validate the actual handshake, pagination and cleanup.
    #[cfg(unix)]
    fn server(script: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", script, "fake-codex"]);
        command
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetch_obeys_handshake_and_paginates() {
        let mut command = server(
            r#"
            read -r init
            case "$init" in *'"initialize"'*) ;; *) exit 1;; esac
            printf '%s\n' '{"id":0,"result":{}}'
            read -r ready
            case "$ready" in *'"initialized"'*) ;; *) exit 2;; esac
            read -r page
            case "$page" in *'"model/list"'*) ;; *) exit 3;; esac
            printf '%s\n' '{"method":"notification"}' '{"id":1,"result":{"data":[{"model":"future-a","supportedReasoningEfforts":[{"reasoningEffort":"ultra"}]}],"nextCursor":"page2"}}'
            read -r page
            case "$page" in *'"cursor":"page2"'*) ;; *) exit 4;; esac
            printf '%s\n' '{"id":2,"result":{"data":[{"model":"future-b"}],"nextCursor":null}}'
            exec sleep 30
        "#,
        );
        let catalogue = fetch(&mut command, Duration::from_secs(3)).await.unwrap();
        assert_eq!(catalogue.models(), ["default", "future-a", "future-b"]);
        assert_eq!(catalogue.efforts(Some("future-a")).unwrap(), ["ultra"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetch_handles_errors_disconnects_and_timeout() {
        for script in [
            "printf '%s\\n' '{\"id\":0,\"error\":{\"code\":-1}}'",
            "printf '%s\\n' 'malformed'",
            "exit 0",
        ] {
            assert!(fetch(&mut server(script), Duration::from_secs(2))
                .await
                .is_err());
        }
        let err = fetch(&mut server("exec sleep 30"), Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    #[ignore = "requires installed, authenticated Codex; lists models without inference"]
    async fn live_codex_catalogue() {
        let catalogue = fetch(&mut tokio::process::Command::new("codex"), FETCH_TIMEOUT)
            .await
            .unwrap();
        assert!(catalogue.models().len() > 1);
        eprintln!("Codex model catalog: {catalogue:?}");
        let expected_models = catalogue.models();
        let first = catalogue.0[0].clone();
        install(catalogue);
        let descriptor = nebula_core::harness::builtin("codex").unwrap();
        assert_eq!(
            crate::config::model_choices_in(&descriptor),
            expected_models
        );
        let choices = crate::config::effort_choices_in(&descriptor, Some(&first.id));
        if first.efforts.is_empty() {
            assert!(choices.is_empty());
        } else {
            assert_eq!(choices[0], DEFAULT_CHOICE);
            assert_eq!(choices[1..], first.efforts);
        }
        assert_eq!(
            crate::config::fit_effort_in(&descriptor, Some(&first.id), Some("unsupported".into())),
            None
        );
        // The Agents tab clears a saved effort when cycling to a model
        // that doesn't support it, just as the preset editor does.
        let mut cfg = crate::config::Config {
            codex_effort: "unsupported".into(),
            ..Default::default()
        };
        cfg.cycle_agent_row("codex", crate::config::HarnessField::Model, 1);
        assert_eq!(cfg.codex_model, first.id);
        assert_eq!(cfg.codex_effort, DEFAULT_CHOICE);
    }
}
