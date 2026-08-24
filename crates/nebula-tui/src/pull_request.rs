//! The pull request open on a worktree's branch, discovered with the
//! GitHub CLI (`gh pr view`). The PR itself is never persisted — the row it
//! feeds sits above the worktree's saved links and refreshes on its own, so
//! a PR opened outside nebula shows up without anyone typing its URL. The
//! one thing that outlives the process is how far the user has read into
//! the conversation, which the daemon keeps (`pr_seen`) so the row can say
//! how many comments landed while they were away.
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
    /// When somebody *other than you* commented or submitted a review, as
    /// GitHub's RFC 3339 stamps, oldest first. Those sort lexicographically,
    /// so "posted since the mark we stored" is a string compare — nebula
    /// never has to parse a date or trust a clock.
    pub activity: Vec<String>,
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

    /// The mark to store when the user opens this PR: everything nebula
    /// currently knows about has been read. Empty when nobody has posted —
    /// which still beats no mark at all, since every real stamp sorts above
    /// it, so the next comment to land counts as new.
    pub fn seen_marker(&self) -> &str {
        self.activity.last().map(String::as_str).unwrap_or("")
    }

    /// How many comments and reviews arrived after `marker`. `None` — a PR
    /// never opened from nebula — leaves the whole conversation unread,
    /// which is the honest answer: the user hasn't looked at any of it.
    pub fn unseen(&self, marker: Option<&str>) -> usize {
        match marker {
            Some(mark) => self.activity.iter().filter(|at| at.as_str() > mark).count(),
            None => self.activity.len(),
        }
    }
}

/// Ask `gh` for the pull request on `dir`'s current branch. `None` covers
/// every ordinary miss: no PR, no `gh`, no remote, not logged in.
pub async fn lookup(dir: &Path) -> Option<PullRequest> {
    let run = tokio::process::Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "number,url,title,state,isDraft,comments,reviews",
        ])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    // Only asked once `gh` has proved it works, so a machine without it
    // never pays for the extra process.
    parse(&String::from_utf8_lossy(&out.stdout), viewer_login().await)
}

/// Your own GitHub login, resolved once per process. Needed only to keep
/// your own review submissions out of the unread count: `gh` flags comments
/// with `viewerDidAuthor`, but reviews carry nothing but an author — and
/// replying to an inline thread on your own PR files a review.
static VIEWER: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

async fn viewer_login() -> Option<&'static str> {
    VIEWER
        .get_or_init(|| async {
            let run = tokio::process::Command::new("gh")
                .args(["api", "user", "--jq", ".login"])
                .stdin(std::process::Stdio::null())
                .output();
            let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
            if !out.status.success() {
                return None;
            }
            let login = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!login.is_empty()).then_some(login)
        })
        .await
        .as_deref()
}

/// Parse `gh pr view --json …` output. Kept separate from the process call
/// so the shape it expects is testable without a GitHub account. `viewer`
/// is your login when it's known; without it your own reviews count as
/// activity, which is a wrong badge rather than a broken one.
fn parse(json: &str, viewer: Option<&str>) -> Option<PullRequest> {
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
        activity: activity(&v, viewer),
    })
}

/// Timestamps of everything other people posted on the PR — issue comments
/// and review submissions alike, since either is a reason to go look —
/// sorted oldest first so the last one is the high-water mark.
fn activity(v: &serde_json::Value, viewer: Option<&str>) -> Vec<String> {
    let list = |key: &str| {
        v.get(key)
            .and_then(|c| c.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let mut stamps: Vec<String> = Vec::new();
    for c in list("comments") {
        if c.get("viewerDidAuthor").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        if let Some(at) = c.get("createdAt").and_then(|t| t.as_str()) {
            stamps.push(at.to_string());
        }
    }
    for r in list("reviews") {
        // No `submittedAt` means a pending review — your own draft, which
        // nobody else can see yet.
        let Some(at) = r.get("submittedAt").and_then(|t| t.as_str()) else {
            continue;
        };
        let author = r
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(|l| l.as_str());
        if viewer.is_some() && author == viewer {
            continue;
        }
        stamps.push(at.to_string());
    }
    stamps.sort();
    stamps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PR carrying `activity`, for the counting tests.
    fn with_activity(stamps: &[&str]) -> PullRequest {
        PullRequest {
            number: 1,
            url: "https://github.com/o/r/pull/1".into(),
            title: "t".into(),
            state: "OPEN".into(),
            is_draft: false,
            activity: stamps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parses_a_gh_pr_view_payload() {
        let pr = parse(
            r#"{"isDraft":false,"number":42,"state":"OPEN","title":"Attach links to worktrees","url":"https://github.com/o/r/pull/42"}"#,
            None,
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
            activity: vec![],
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
        assert!(parse("", None).is_none());
        assert!(parse("{}", None).is_none());
        assert!(parse(r#"{"number":1,"url":"file:///etc/passwd"}"#, None).is_none());
    }

    /// Comments and review submissions both count, both are sorted into one
    /// oldest-first list, and anything the viewer wrote is left out — the
    /// badge is about what *other* people said.
    #[test]
    fn activity_gathers_other_peoples_comments_and_reviews() {
        let pr = parse(
            r#"{
              "number": 42, "url": "https://github.com/o/r/pull/42",
              "comments": [
                {"createdAt": "2024-04-26T21:44:55Z", "viewerDidAuthor": false},
                {"createdAt": "2024-04-27T09:00:00Z", "viewerDidAuthor": true}
              ],
              "reviews": [
                {"submittedAt": "2024-04-25T19:55:42Z", "author": {"login": "steiza"}},
                {"submittedAt": "2024-04-28T08:00:00Z", "author": {"login": "me"}},
                {"author": {"login": "steiza"}}
              ]
            }"#,
            Some("me"),
        )
        .expect("parsed");
        assert_eq!(
            pr.activity,
            ["2024-04-25T19:55:42Z", "2024-04-26T21:44:55Z"],
            "own comment, own review and an unsubmitted review all drop out"
        );
    }

    /// A payload from an older `gh` (or a PR with an empty conversation)
    /// carries no comment arrays at all; that's zero activity, not a miss.
    #[test]
    fn a_payload_without_conversation_fields_still_parses() {
        let pr = parse(
            r#"{"number":1,"url":"https://github.com/o/r/pull/1"}"#,
            Some("me"),
        )
        .expect("parsed");
        assert!(pr.activity.is_empty());
        assert_eq!(pr.seen_marker(), "");
    }

    /// The unread count is a comparison against the mark stored on the last
    /// open — no mark means nothing has been read.
    #[test]
    fn unseen_counts_what_landed_after_the_mark() {
        let pr = with_activity(&[
            "2024-04-25T19:55:42Z",
            "2024-04-26T21:44:55Z",
            "2024-04-27T09:00:00Z",
        ]);
        assert_eq!(pr.unseen(None), 3, "never opened: all of it is unread");
        assert_eq!(pr.unseen(Some("2024-04-25T19:55:42Z")), 2);
        assert_eq!(pr.unseen(Some(pr.seen_marker())), 0, "opening clears it");
    }

    /// Opening a PR nobody has posted on stores an empty mark, and that
    /// mark still does its job: every real timestamp sorts above it, so the
    /// next comment to land reads as new.
    #[test]
    fn an_empty_mark_still_catches_the_next_comment() {
        let quiet = with_activity(&[]);
        assert_eq!(quiet.seen_marker(), "");
        assert_eq!(quiet.unseen(Some("")), 0);

        let later = with_activity(&["2024-04-26T21:44:55Z"]);
        assert_eq!(later.unseen(Some("")), 1);
    }

    /// Deleted comments shrink the list; the count must not go negative or
    /// wrap — it just reports nothing new.
    #[test]
    fn a_deleted_comment_does_not_invent_unread_ones() {
        let pr = with_activity(&["2024-04-25T19:55:42Z"]);
        assert_eq!(pr.unseen(Some("2024-04-27T09:00:00Z")), 0);
    }
}
