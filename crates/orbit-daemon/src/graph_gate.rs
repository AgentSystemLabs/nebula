//! GRAPH GATE: makes a Claude session in a checkout with a graphify graph
//! actually use it.
//!
//! The appended system prompt ([`crate::graphify::claude_guidance`]) only
//! *tells* Claude the graph exists, and in practice Claude goes straight
//! to `find`, `grep`, Grep and Glob anyway — searches that pour whole file
//! listings and match dumps into its context. The gate is a `PreToolUse`
//! hook on `Grep|Glob|Bash` (installed by `hooks::installer`) that the
//! daemon answers here:
//!
//! - a `graphify query|explain|path|affected|god-nodes` call marks the
//!   graph used for the current prompt, and passes;
//! - a broad search (Grep, Glob, or a Bash command that starts with
//!   `grep`/`rg`/`find`/`fd`/`ag`/`ack`/`git grep`) made before any graph
//!   query this prompt is DENIED, with a reason that hands Claude the exact
//!   `graphify query` to run — at most [`MAX_NUDGES`] times a prompt, so a
//!   session that will not or cannot query the graph is never stuck;
//!
//! Measured against Claude Code 2.1.296: a deny that offered "retry the
//! same search and it goes through" was simply retried, graph untouched;
//! a deny that requires the query first was answered with
//! `graphify query` after at most two attempts.
//! - everything else passes untouched.
//!
//! "Once per prompt": `UserPromptSubmit` starts a fresh turn. The gate
//! fails open everywhere — no graph, graphify switched off, an unknown
//! payload, an unreachable daemon (the hook's curl prints nothing) all let
//! the tool run.
//!
//! Every graph query, search and nudge is appended to
//! `<checkout>/.orbit/local/graph/orbit-usage.jsonl` (git-excluded with the
//! rest of `.orbit/local/`), so how much the graph is really used can be read
//! back with `orbit graph-usage`.

use orbit_core::AgentId;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The per-checkout usage log, inside graphify's own output folder.
pub const USAGE_LOG: &str = "orbit-usage.jsonl";

/// graphify's read-only query subcommands — the ones that count as using
/// the graph (and that the permission rules pre-approve).
const GRAPH_SUBCOMMANDS: &[&str] = &["query", "explain", "path", "affected", "god-nodes"];

/// Shell programs whose run, at the head of a command, is a broad search
/// of the tree.
const SEARCH_PROGRAMS: &[&str] = &["grep", "egrep", "rg", "find", "fd", "ag", "ack"];

/// What one tool call is, as far as the gate cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolKind {
    /// A graphify query.
    Graph,
    /// A broad file-by-file search.
    Search,
    /// Anything else.
    Other,
}

/// Classify a `PreToolUse` call by its tool name and input.
pub fn classify(tool_name: &str, tool_input: Option<&Value>) -> ToolKind {
    match tool_name {
        "Grep" | "Glob" => ToolKind::Search,
        "Bash" => {
            let command = tool_input
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("");
            classify_command(command)
        }
        _ => ToolKind::Other,
    }
}

/// Classify a shell command line. Each `&&` / `||` / `;` / newline
/// segment is looked at by its first pipe stage only — `ps aux | grep x`
/// filters output, it does not search the tree. A graph query anywhere
/// wins over a search beside it.
pub fn classify_command(command: &str) -> ToolKind {
    let mut kind = ToolKind::Other;
    for segment in command
        .split(['\n', ';'])
        .flat_map(|s| s.split("&&"))
        .flat_map(|s| s.split("||"))
    {
        let head = segment.split('|').next().unwrap_or("");
        let words: Vec<&str> = head
            .split_whitespace()
            // `FOO=1 grep …`, `sudo`, `command`, `time` prefixes.
            .skip_while(|w| w.contains('=') || matches!(*w, "sudo" | "command" | "time" | "exec"))
            .collect();
        let Some(program) = words.first() else {
            continue;
        };
        let name = program.rsplit('/').next().unwrap_or(program);
        if name == "graphify" {
            if words
                .get(1)
                .is_some_and(|sub| GRAPH_SUBCOMMANDS.contains(sub))
            {
                return ToolKind::Graph;
            }
            continue;
        }
        let git_grep = name == "git" && words.get(1) == Some(&"grep");
        if git_grep || SEARCH_PROGRAMS.contains(&name) {
            kind = ToolKind::Search;
        }
    }
    kind
}

/// The checkout a session at `cwd` is in, when it has a graph: `cwd` or
/// the nearest ancestor holding `.orbit/local/graph/graph.json` (a Bash `cd`
/// into a subfolder moves the reported cwd down).
pub fn graph_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| crate::graphify::has_graph(dir))
        .map(Path::to_path_buf)
}

/// How many searches a prompt may have denied before the gate lets them
/// through regardless.
pub const MAX_NUDGES: u8 = 2;

/// One prompt's worth of gate state for a session.
#[derive(Debug, Default, Clone, Copy)]
struct Turn {
    /// A graph query ran this prompt.
    graph_used: bool,
    /// Searches denied this prompt so far.
    nudges: u8,
}

/// What the gate decided for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Deny, with the reason Claude reads.
    Deny(String),
}

/// The gate's memory: per session, the current prompt's turn.
#[derive(Default)]
pub struct GraphGate {
    turns: Mutex<HashMap<AgentId, Turn>>,
}

impl GraphGate {
    /// A new prompt arrived for `agent`: the next search may be nudged again.
    pub fn new_turn(&self, agent: &AgentId) {
        self.turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(agent);
    }

    /// Decide one call of `kind` by `agent`, in a checkout that has a
    /// graph and wants the gate (the caller checks both).
    pub fn decide(&self, agent: &AgentId, kind: &ToolKind) -> Verdict {
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let turn = turns.entry(agent.clone()).or_default();
        match kind {
            ToolKind::Graph => {
                turn.graph_used = true;
                Verdict::Allow
            }
            ToolKind::Search if !turn.graph_used && turn.nudges < MAX_NUDGES => {
                turn.nudges += 1;
                Verdict::Deny(nudge(&crate::graphify::shell_command()))
            }
            ToolKind::Search | ToolKind::Other => Verdict::Allow,
        }
    }
}

/// The reason a denied search hands back to Claude — short, since it lands
/// in context, and concrete: the command to run instead.
pub fn nudge(graphify: &str) -> String {
    format!(
        "[orbit] Search blocked: this checkout has a code knowledge graph, and orbit requires one \
graph query before file-by-file search in each prompt. Run:\n\n  {graphify} query \"<what you are \
looking for>\"\n\n(or {graphify} explain \"<symbol or file>\", affected \"<symbol>\", path \"<A>\" \
\"<B>\"). It returns the relevant files, symbols and line numbers in one call. After that, Grep, \
Glob, find and grep work normally for the rest of this prompt."
    )
}

/// The `PreToolUse` hook reply for `verdict`: the deny envelope, or an
/// empty body (the hook prints nothing and the tool runs).
pub fn reply(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Allow => String::new(),
        Verdict::Deny(reason) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    }
}

/// Append one usage record to `root`'s log. Best effort: a failed write
/// only loses the record.
pub fn record(
    root: &Path,
    agent: &AgentId,
    kind: &ToolKind,
    verdict: &Verdict,
    tool: &str,
    input: Option<&Value>,
) {
    let event = match (kind, verdict) {
        (ToolKind::Graph, _) => "graph",
        (ToolKind::Search, Verdict::Deny(_)) => "nudge",
        (ToolKind::Search, Verdict::Allow) => "search",
        (ToolKind::Other, _) => return,
    };
    let detail = input
        .and_then(|i| i.get("command").or_else(|| i.get("pattern")))
        .and_then(Value::as_str)
        .unwrap_or("");
    let detail: String = detail.chars().take(200).collect();
    let line = serde_json::json!({
        "ts": orbit_core::clock::now_secs(),
        "agent": agent.0,
        "event": event,
        "tool": tool,
        "detail": detail,
    });
    let path = root.join(crate::graphify::OUT_DIR).join(USAGE_LOG);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// What a usage log adds up to (`orbit graph-usage`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub graph: usize,
    pub searches: usize,
    pub nudges: usize,
    /// Runs of nudges — roughly, prompts where the session went for a
    /// search before the graph.
    pub nudged_prompts: usize,
    /// Of those, the ones whose next gated call was a graph query.
    pub nudges_heeded: usize,
    pub sessions: usize,
    pub sessions_using_graph: usize,
    /// The most recent graph queries, newest last.
    pub recent_graph: Vec<String>,
}

/// Add up a usage log's lines. Lines that do not parse are skipped.
pub fn summarize(log: &str, recent: usize) -> Summary {
    let mut out = Summary::default();
    let mut in_streak: HashMap<String, bool> = HashMap::new();
    let mut used: HashMap<String, bool> = HashMap::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let agent = v["agent"].as_str().unwrap_or("").to_string();
        let event = v["event"].as_str().unwrap_or("");
        let streak = in_streak
            .insert(agent.clone(), event == "nudge")
            .unwrap_or(false);
        let seen = used.entry(agent.clone()).or_insert(false);
        match event {
            "graph" => {
                out.graph += 1;
                *seen = true;
                out.nudges_heeded += usize::from(streak);
                out.recent_graph
                    .push(v["detail"].as_str().unwrap_or("").to_string());
            }
            "search" => out.searches += 1,
            "nudge" => {
                out.nudges += 1;
                out.nudged_prompts += usize::from(!streak);
            }
            _ => {}
        }
    }
    out.sessions = used.len();
    out.sessions_using_graph = used.values().filter(|u| **u).count();
    let skip = out.recent_graph.len().saturating_sub(recent);
    out.recent_graph.drain(..skip);
    out
}

/// A percentage, 0 when there is nothing to divide by.
fn pct(part: usize, whole: usize) -> usize {
    (part * 100).checked_div(whole).unwrap_or(0)
}

/// The report `orbit graph-usage` prints for the log at `log_path`.
pub fn render(summary: &Summary, log_path: &Path) -> String {
    let s = summary;
    let mut out = format!("graph usage · {}\n\n", log_path.display());
    out += &format!("  graph queries  {:>5}\n", s.graph);
    out += &format!(
        "  searches       {:>5}   grep / find / Grep / Glob that ran\n",
        s.searches
    );
    out += &format!(
        "  nudges         {:>5}   in {} prompts; {} switched to the graph ({}%)\n",
        s.nudges,
        s.nudged_prompts,
        s.nudges_heeded,
        pct(s.nudges_heeded, s.nudged_prompts)
    );
    out += &format!(
        "  graph share    {:>4}%   graph queries / (graph queries + searches)\n",
        pct(s.graph, s.graph + s.searches)
    );
    out += &format!(
        "  sessions       {:>5}   {} used the graph\n",
        s.sessions, s.sessions_using_graph
    );
    if !s.recent_graph.is_empty() {
        out += "\nrecent graph queries:\n";
        for q in &s.recent_graph {
            out += &format!("  {q}\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grep_and_glob_tools_are_searches() {
        assert_eq!(classify("Grep", None), ToolKind::Search);
        assert_eq!(classify("Glob", None), ToolKind::Search);
        assert_eq!(classify("Read", None), ToolKind::Other);
    }

    #[test]
    fn bash_search_heads_are_searches() {
        for cmd in [
            "find . -name '*compose*'",
            "cd app && rg -n Foo src",
            "grep -rn foo .",
            "git grep -n foo",
            "LC_ALL=C grep foo x",
            "/usr/bin/find . -type f | head",
            "ls; fd compose",
        ] {
            assert_eq!(classify_command(cmd), ToolKind::Search, "{cmd}");
        }
    }

    #[test]
    fn filters_and_other_commands_are_not_searches() {
        for cmd in [
            "ps aux | grep claude",
            "ls -la",
            "cat a.txt | rg x",
            "cargo test",
            "",
        ] {
            assert_eq!(classify_command(cmd), ToolKind::Other, "{cmd}");
        }
    }

    #[test]
    fn graphify_queries_are_graph_use() {
        for cmd in [
            "graphify query \"where is auth\"",
            "/Users/x/.local/bin/graphify explain Foo",
            "cd sub && graphify affected Bar",
            "graphify god-nodes | head",
        ] {
            assert_eq!(classify_command(cmd), ToolKind::Graph, "{cmd}");
        }
        assert_eq!(classify_command("graphify update ."), ToolKind::Other);
        let input = json!({ "command": "graphify path A B" });
        assert_eq!(classify("Bash", Some(&input)), ToolKind::Graph);
    }

    #[test]
    fn searches_are_denied_until_a_graph_query_at_most_twice_a_turn() {
        let gate = GraphGate::default();
        let a = AgentId("a".into());
        assert!(matches!(
            gate.decide(&a, &ToolKind::Search),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            gate.decide(&a, &ToolKind::Search),
            Verdict::Deny(_)
        ));
        // Never stuck: the third goes through.
        assert_eq!(gate.decide(&a, &ToolKind::Search), Verdict::Allow);
        // A new prompt nudges again.
        gate.new_turn(&a);
        assert!(matches!(
            gate.decide(&a, &ToolKind::Search),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn a_graph_query_after_a_nudge_unlocks_search() {
        let gate = GraphGate::default();
        let a = AgentId("a".into());
        assert!(matches!(
            gate.decide(&a, &ToolKind::Search),
            Verdict::Deny(_)
        ));
        assert_eq!(gate.decide(&a, &ToolKind::Graph), Verdict::Allow);
        assert_eq!(gate.decide(&a, &ToolKind::Search), Verdict::Allow);
    }

    #[test]
    fn a_graph_query_first_means_no_nudge() {
        let gate = GraphGate::default();
        let a = AgentId("a".into());
        assert_eq!(gate.decide(&a, &ToolKind::Graph), Verdict::Allow);
        assert_eq!(gate.decide(&a, &ToolKind::Search), Verdict::Allow);
        assert_eq!(gate.decide(&a, &ToolKind::Other), Verdict::Allow);
    }

    #[test]
    fn sessions_are_gated_separately() {
        let gate = GraphGate::default();
        let (a, b) = (AgentId("a".into()), AgentId("b".into()));
        assert!(matches!(
            gate.decide(&a, &ToolKind::Search),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            gate.decide(&b, &ToolKind::Search),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn reply_is_empty_on_allow_and_a_deny_envelope_otherwise() {
        assert_eq!(reply(&Verdict::Allow), "");
        let body: Value = serde_json::from_str(&reply(&Verdict::Deny("why".into()))).unwrap();
        assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            body["hookSpecificOutput"]["permissionDecisionReason"],
            "why"
        );
        assert_eq!(body["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    }

    #[test]
    fn graph_root_walks_up_to_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::graphify::out_dir(tmp.path())).unwrap();
        std::fs::write(crate::graphify::graph_json(tmp.path()), "{}").unwrap();
        let sub = tmp.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(graph_root(&sub).as_deref(), Some(tmp.path()));
        let other = tempfile::tempdir().unwrap();
        assert_eq!(graph_root(other.path()), None);
    }

    #[test]
    fn record_appends_graph_search_and_nudge_lines() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::graphify::out_dir(tmp.path())).unwrap();
        let a = AgentId("a".into());
        let cmd = json!({ "command": "graphify query x" });
        record(
            tmp.path(),
            &a,
            &ToolKind::Graph,
            &Verdict::Allow,
            "Bash",
            Some(&cmd),
        );
        record(
            tmp.path(),
            &a,
            &ToolKind::Search,
            &Verdict::Deny("r".into()),
            "Grep",
            Some(&json!({"pattern": "foo"})),
        );
        record(
            tmp.path(),
            &a,
            &ToolKind::Search,
            &Verdict::Allow,
            "Grep",
            None,
        );
        record(
            tmp.path(),
            &a,
            &ToolKind::Other,
            &Verdict::Allow,
            "Bash",
            None,
        );
        let log =
            std::fs::read_to_string(crate::graphify::out_dir(tmp.path()).join(USAGE_LOG)).unwrap();
        let events: Vec<String> = log
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(events, ["graph", "nudge", "search"]);
    }

    #[test]
    fn summarize_counts_events_heeded_nudges_and_sessions() {
        let log = [
            r#"{"agent":"a","event":"nudge","detail":"foo"}"#,
            r#"{"agent":"a","event":"nudge","detail":"foo2"}"#,
            r#"{"agent":"a","event":"graph","detail":"graphify query foo"}"#,
            r#"{"agent":"a","event":"search","detail":"bar"}"#,
            r#"{"agent":"b","event":"nudge","detail":"x"}"#,
            r#"{"agent":"b","event":"search","detail":"x"}"#,
            "garbage",
        ]
        .join("\n");
        let s = summarize(&log, 10);
        assert_eq!((s.graph, s.searches, s.nudges), (1, 2, 3));
        assert_eq!((s.nudged_prompts, s.nudges_heeded), (2, 1));
        assert_eq!((s.sessions, s.sessions_using_graph), (2, 1));
        assert_eq!(s.recent_graph, ["graphify query foo"]);
        let text = render(&s, Path::new("/x/orbit-usage.jsonl"));
        assert!(text.contains("graph share      33%"), "{text}");
    }
}
