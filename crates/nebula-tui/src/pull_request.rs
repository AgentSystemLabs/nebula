//! The pull request open on a worktree's branch, discovered with the
//! GitHub CLI (`gh pr view`). Nothing here is persisted — the row it feeds
//! sits above the worktree's saved links and refreshes on its own, so a PR
//! opened outside nebula shows up without anyone typing its URL.
//!
//! `gh` may be missing, unauthenticated, or pointed at a repo with no
//! remote; every one of those is an ordinary "no PR" answer, not an error
//! worth a flash. Lookups are async because they hit the network.

use std::path::Path;

/// How long a lookup may run before we give up on it. `gh` retries and can
/// hang on a stalled network; the row is a convenience, not worth a task
/// that never ends.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The pull request `gh` reports for a checkout's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub title: String,
    /// `gh`'s state string: OPEN, MERGED or CLOSED.
    pub state: String,
    pub is_draft: bool,
}

impl PullRequest {
    /// Short state word for the row's trailing badge — the same slot the
    /// agent rows use for their CLI kind.
    pub fn badge(&self) -> &'static str {
        match self.state.as_str() {
            "OPEN" if self.is_draft => "draft",
            "OPEN" => "pr",
            "MERGED" => "merged",
            "CLOSED" => "closed",
            _ => "pr",
        }
    }

    /// Whether the PR is still open (draft included) — the badge is quiet
    /// for these and loud for the ones that no longer accept work.
    pub fn is_open(&self) -> bool {
        self.state == "OPEN"
    }
}

/// Ask `gh` for the pull request on `dir`'s current branch. `None` covers
/// every ordinary miss: no PR, no `gh`, no remote, not logged in.
pub async fn lookup(dir: &Path) -> Option<PullRequest> {
    let run = tokio::process::Command::new("gh")
        .args(["pr", "view", "--json", "number,url,title,state,isDraft"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `gh pr view --json …` output. Kept separate from the process call
/// so the shape it expects is testable without a GitHub account.
fn parse(json: &str) -> Option<PullRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let url = v.get("url")?.as_str()?.to_string();
    // Only http(s) reaches `open(1)`; gh has no business returning anything
    // else, but the row leads straight to a browser so it's checked anyway.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        url,
        title: v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        state: v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("OPEN")
            .to_string(),
        is_draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_gh_pr_view_payload() {
        let pr = parse(
            r#"{"isDraft":false,"number":42,"state":"OPEN","title":"Attach links to worktrees","url":"https://github.com/o/r/pull/42"}"#,
        )
        .expect("parsed");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.url, "https://github.com/o/r/pull/42");
        assert_eq!(pr.title, "Attach links to worktrees");
        assert_eq!(pr.badge(), "pr");
        assert!(pr.is_open());
    }

    #[test]
    fn badges_name_the_state() {
        let base = PullRequest {
            number: 1,
            url: "https://x.dev/pull/1".into(),
            title: "t".into(),
            state: "OPEN".into(),
            is_draft: true,
        };
        assert_eq!(base.badge(), "draft");
        assert!(base.is_open(), "a draft is still open");
        let merged = PullRequest {
            state: "MERGED".into(),
            is_draft: false,
            ..base.clone()
        };
        assert_eq!(merged.badge(), "merged");
        assert!(!merged.is_open());
        let closed = PullRequest {
            state: "CLOSED".into(),
            ..merged
        };
        assert_eq!(closed.badge(), "closed");
    }

    #[test]
    fn refuses_payloads_that_are_not_http_links() {
        // No PR at all, and a payload whose url could never be opened.
        assert!(parse("").is_none());
        assert!(parse("{}").is_none());
        assert!(parse(r#"{"number":1,"url":"file:///etc/passwd"}"#).is_none());
    }
}
