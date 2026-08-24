//! The pull request open on a worktree's branch, discovered with the
//! GitHub CLI (`gh pr view`). The PR itself is never persisted — the row it
//! feeds sits above the worktree's saved links and refreshes on its own, so
//! a PR opened outside nebula shows up without anyone typing its URL. The
//! one thing that outlives the process is how far the user has read into
//! the conversation, which the daemon keeps (`pr_seen`) so the row can say
//! how many comments landed while they were away.
//!
//! The same `gh` also answers the wider question this module's other half
//! asks — every pull request still open on the *project's* repo, for the
//! group at the bottom of the worktrees panel (see [`list`]).
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

/// Every open pull request on a project's repo, and what it costs to ask.
///
/// A worktree's own PR ([`lookup`]) is one `gh pr view` per checkout; this
/// is one `gh pr list` per *project*, answering "what's still open here?"
/// for the group at the bottom of the worktrees panel. It deliberately
/// carries no conversation: reading comment counts for a hundred rows would
/// be a request each, so the unread badge stays a per-worktree affair.
/// One page, one call, however many PRs the repo has.
///
/// `gh` pages past its own 30-row default, so the cap is ours to set: a
/// repo with hundreds of open pull requests would spend several API calls
/// per refresh filling rows nobody scrolls to.
pub const LIST_LIMIT: usize = 100;

/// One row of a project's open-pull-request list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub is_draft: bool,
}

impl OpenPr {
    /// Row text: `#42 title`, the same shape the worktree link rows use.
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            format!("#{}", self.number)
        } else {
            format!("#{} {}", self.number, self.title)
        }
    }

    /// Trailing badge — every row here is open by construction, so the only
    /// thing left to say is whether it's still a draft.
    pub fn badge(&self) -> &'static str {
        if self.is_draft {
            "draft"
        } else {
            "pr"
        }
    }
}

/// Ask `gh` for every open pull request on `dir`'s repo, newest first.
/// `None` is "couldn't ask" — no `gh`, no remote, not logged in, timed out —
/// and is deliberately distinct from `Some(vec![])`, which is the real
/// answer "nothing is open": the caller keeps the last good list rather than
/// blanking the panel over one failed call.
pub async fn list(dir: &Path) -> Option<Vec<OpenPr>> {
    let run = tokio::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            &LIST_LIMIT.to_string(),
            "--json",
            "number,url,title,isDraft",
        ])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_list(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `gh pr list --json …` output — a bare array. Kept separate from
/// the process call so the shape it expects is testable without a GitHub
/// account. A row whose url could never be opened is dropped rather than
/// failing the whole list; a payload that isn't an array at all is a miss.
fn parse_list(json: &str) -> Option<Vec<OpenPr>> {
    let rows = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let rows = rows.as_array()?;
    Some(
        rows.iter()
            .filter_map(|v| {
                let url = v.get("url")?.as_str()?.to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return None;
                }
                Some(OpenPr {
                    number: v.get("number")?.as_u64()?,
                    title: v
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url,
                    is_draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
                })
            })
            .collect(),
    )
}

/// How long a `gh pr diff` may run. Diffs are bigger than metadata and
/// GitHub can be slow to assemble one for a large pull request, so this is
/// looser than [`TIMEOUT`] — but still bounded, because the user is sitting
/// in front of a "loading" flash while it runs.
const DIFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// The readable contents of one pull request: what it says it does, and
/// what people said back. Fetched on demand — only for the row the cursor
/// actually rests on — and cached for the session, because this is a second
/// API call on top of the list and the body of a merged-or-not pull request
/// does not change while you read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetail {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: String,
    pub is_draft: bool,
    pub author: String,
    /// Branch this merges into, and the branch it comes from.
    pub base: String,
    pub head: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// The description, verbatim markdown. Rendered as plain wrapped text —
    /// nebula is not a markdown viewer, and mangling someone's fenced code
    /// block would be worse than showing it as written.
    pub body: String,
    /// Issue comments and review submissions in one list, oldest first —
    /// the order they were said in, which is the order they read in.
    pub comments: Vec<PrComment>,
}

/// One thing somebody said on a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    pub author: String,
    /// RFC 3339, as GitHub gives it.
    pub at: String,
    /// Empty for a plain comment; the review state (`APPROVED`,
    /// `CHANGES_REQUESTED`, `COMMENTED`) when it came in as a review.
    pub review_state: String,
    pub body: String,
}

impl PrComment {
    /// Short word for the row's badge, or None for a plain comment.
    pub fn verdict(&self) -> Option<&'static str> {
        match self.review_state.as_str() {
            "APPROVED" => Some("approved"),
            "CHANGES_REQUESTED" => Some("changes requested"),
            "DISMISSED" => Some("dismissed"),
            _ => None,
        }
    }
}

/// Ask `gh` for one pull request's description and conversation. `number`
/// picks the PR, so this works from any checkout of the repo — the row the
/// cursor is on need not be checked out anywhere.
pub async fn detail(dir: &Path, number: u64) -> Option<PrDetail> {
    let run = tokio::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "number,url,title,state,isDraft,author,baseRefName,headRefName,\
             additions,deletions,changedFiles,body,comments,reviews",
        ])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_detail(&String::from_utf8_lossy(&out.stdout))
}

fn parse_detail(json: &str) -> Option<PrDetail> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let str_at = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let num_at = |key: &str| v.get(key).and_then(|x| x.as_u64()).unwrap_or(0);
    Some(PrDetail {
        number: v.get("number")?.as_u64()?,
        url: v.get("url")?.as_str()?.to_string(),
        title: str_at("title"),
        state: v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("OPEN")
            .to_string(),
        is_draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
        author: login(v.get("author")),
        base: str_at("baseRefName"),
        head: str_at("headRefName"),
        additions: num_at("additions"),
        deletions: num_at("deletions"),
        changed_files: num_at("changedFiles"),
        body: str_at("body"),
        comments: conversation(&v),
    })
}

fn login(author: Option<&serde_json::Value>) -> String {
    author
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Comments and review submissions merged into one oldest-first thread.
/// A review with no body is a bare verdict (an approval with nothing typed);
/// it is kept, because "someone approved this" is worth reading, and its
/// empty body renders as the badge alone. Unsubmitted reviews — your own
/// pending draft — are left out; nobody else can see them.
fn conversation(v: &serde_json::Value) -> Vec<PrComment> {
    let list = |key: &str| {
        v.get(key)
            .and_then(|c| c.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let mut out: Vec<PrComment> = Vec::new();
    for c in list("comments") {
        out.push(PrComment {
            author: login(c.get("author")),
            at: c
                .get("createdAt")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            review_state: String::new(),
            body: c
                .get("body")
                .and_then(|b| b.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    for r in list("reviews") {
        let Some(at) = r.get("submittedAt").and_then(|t| t.as_str()) else {
            continue;
        };
        out.push(PrComment {
            author: login(r.get("author")),
            at: at.to_string(),
            review_state: r
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string(),
            body: r
                .get("body")
                .and_then(|b| b.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    // RFC 3339 UTC stamps sort lexicographically into chronological order —
    // the same trick the unread badge uses, so still no date parsing.
    out.sort_by(|a, b| a.at.cmp(&b.at));
    out
}

/// The whole unified diff of a pull request, in one call. `None` when `gh`
/// couldn't answer.
pub async fn diff(dir: &Path, number: u64) -> Option<String> {
    let run = tokio::process::Command::new("gh")
        .args(["pr", "diff", &number.to_string()])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(DIFF_TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Cut a unified diff into one chunk per file, in the order git emitted
/// them: `(path, that file's diff text)`.
///
/// The path comes from the `+++ b/…` line when there is one and falls back
/// to the `diff --git` header, so a deleted file (whose `+++` is
/// `/dev/null`) still reports the path it had. Anything before the first
/// `diff --git` — `gh` prints nothing there today, but a future banner
/// would land there — is dropped rather than shown as a nameless file.
pub fn split_unified_diff(text: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut path = String::new();
    let mut lines: Vec<&str> = Vec::new();
    let flush = |files: &mut Vec<(String, String)>, path: &mut String, lines: &mut Vec<&str>| {
        if !path.is_empty() {
            files.push((std::mem::take(path), lines.join("\n")));
        }
        lines.clear();
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&mut files, &mut path, &mut lines);
            path = header_path(rest);
        }
        if path.is_empty() {
            continue; // preamble before the first file
        }
        // `+++ b/x` is authoritative: it survives the quoting and the
        // spaces-in-names ambiguity that makes `diff --git` hard to split.
        if let Some(rest) = line.strip_prefix("+++ ") {
            if rest != "/dev/null" {
                path = rest.strip_prefix("b/").unwrap_or(rest).to_string();
            }
        }
        lines.push(line);
    }
    flush(&mut files, &mut path, &mut lines);
    files
}

/// Best-effort path out of a `diff --git a/x b/x` header. The two halves
/// are the same path for everything but a rename, so the second half is
/// taken and the `b/` prefix stripped; a name containing spaces makes this
/// ambiguous, which is why the `+++` line overrides it when one follows.
fn header_path(rest: &str) -> String {
    let rest = rest.trim();
    match rest.split_once(" b/") {
        Some((_, b)) => b.to_string(),
        None => rest
            .rsplit(' ')
            .next()
            .unwrap_or(rest)
            .strip_prefix("b/")
            .unwrap_or(rest)
            .to_string(),
    }
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

    #[test]
    fn parses_a_gh_pr_list_payload() {
        let prs = parse_list(
            r#"[
              {"number":42,"title":"Attach links","url":"https://github.com/o/r/pull/42","isDraft":false},
              {"number":7,"title":"WIP","url":"https://github.com/o/r/pull/7","isDraft":true}
            ]"#,
        )
        .expect("parsed");
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].label(), "#42 Attach links");
        assert_eq!(prs[0].badge(), "pr");
        assert_eq!(prs[1].badge(), "draft");
    }

    /// An empty repo answers with an empty array — a real answer, not a
    /// miss, so the panel shows "no open pull requests" rather than
    /// pretending it never asked.
    #[test]
    fn an_empty_list_is_an_answer_not_a_miss() {
        assert_eq!(parse_list("[]"), Some(vec![]));
    }

    /// One unusable row must not cost the whole list; a payload that isn't
    /// a list at all is a miss.
    #[test]
    fn list_rows_that_could_never_be_opened_drop_out() {
        let prs = parse_list(
            r#"[
              {"number":1,"url":"file:///etc/passwd"},
              {"url":"https://github.com/o/r/pull/2"},
              {"number":3,"url":"https://github.com/o/r/pull/3"}
            ]"#,
        )
        .expect("parsed");
        assert_eq!(
            prs.len(),
            1,
            "only the row with both a number and an http url"
        );
        assert_eq!(prs[0].label(), "#3", "a missing title still names the PR");
        assert!(parse_list("").is_none());
        assert!(parse_list("{}").is_none());
    }

    /// The preview payload: description, stats, and one merged oldest-first
    /// thread of comments and reviews.
    #[test]
    fn parses_a_gh_pr_view_detail_payload() {
        let d = parse_detail(
            r#"{
              "number": 42, "url": "https://github.com/o/r/pull/42",
              "title": "Attach links", "state": "OPEN", "isDraft": false,
              "author": {"login": "webdevcody"},
              "baseRefName": "main", "headRefName": "feat/links",
              "additions": 106, "deletions": 4, "changedFiles": 2,
              "body": "Closes #1\n\nMakes the row.",
              "comments": [
                {"author": {"login": "steiza"}, "createdAt": "2024-04-26T21:44:55Z", "body": "nice"}
              ],
              "reviews": [
                {"author": {"login": "kate"}, "submittedAt": "2024-04-25T19:55:42Z",
                 "state": "APPROVED", "body": "ship it"},
                {"author": {"login": "kate"}, "state": "PENDING", "body": "draft"}
              ]
            }"#,
        )
        .expect("parsed");
        assert_eq!(d.author, "webdevcody");
        assert_eq!((d.base.as_str(), d.head.as_str()), ("main", "feat/links"));
        assert_eq!((d.additions, d.deletions, d.changed_files), (106, 4, 2));
        assert!(d.body.starts_with("Closes #1"));
        // The review is older than the comment, so it leads — and the
        // unsubmitted one never shows up.
        assert_eq!(d.comments.len(), 2);
        assert_eq!(d.comments[0].author, "kate");
        assert_eq!(d.comments[0].verdict(), Some("approved"));
        assert_eq!(d.comments[1].author, "steiza");
        assert_eq!(d.comments[1].verdict(), None, "a plain comment has none");
    }

    /// Missing optional fields are zeros and empty strings, not a failed
    /// parse: only the number and url are load-bearing.
    #[test]
    fn a_sparse_detail_payload_still_parses() {
        let d = parse_detail(r#"{"number":1,"url":"https://x.dev/pull/1"}"#).expect("parsed");
        assert_eq!(d.title, "");
        assert_eq!(d.state, "OPEN");
        assert!(d.comments.is_empty());
        assert!(parse_detail("{}").is_none());
    }

    /// The diff is cut per file, in git's order, with the `+++ b/…` line
    /// naming each chunk.
    #[test]
    fn a_unified_diff_splits_per_file() {
        let text = "\
diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-x
+y
";
        let files = split_unified_diff(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "src/a.rs");
        assert!(files[0].1.starts_with("diff --git a/src/a.rs"));
        assert!(files[0].1.contains("+new"));
        assert!(
            !files[0].1.contains("src/b.rs"),
            "the chunk stops at the next file: {}",
            files[0].1
        );
        assert_eq!(files[1].0, "src/b.rs");
        // Nothing is lost and nothing is duplicated: every input line lands
        // in exactly one chunk. Checked against real `gh pr diff` output
        // too, which is what this invariant is really guarding.
        let total: usize = files.iter().map(|(_, d)| d.lines().count()).sum();
        assert_eq!(total, text.lines().count());
    }

    /// A deleted file's `+++` is `/dev/null`, so the name has to come from
    /// the `diff --git` header — and a rename reports the new path.
    #[test]
    fn deleted_and_renamed_files_still_get_a_name() {
        let files = split_unified_diff(
            "\
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-x
diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
--- a/old.rs
+++ b/new.rs
",
        );
        assert_eq!(files[0].0, "gone.rs");
        assert_eq!(files[1].0, "new.rs");
    }

    /// Nothing to split is an empty list, not a nameless file.
    #[test]
    fn an_empty_diff_yields_no_files() {
        assert!(split_unified_diff("").is_empty());
        assert!(split_unified_diff("some banner\nwith no diff\n").is_empty());
    }

    /// Deleted comments shrink the list; the count must not go negative or
    /// wrap — it just reports nothing new.
    #[test]
    fn a_deleted_comment_does_not_invent_unread_ones() {
        let pr = with_activity(&["2024-04-25T19:55:42Z"]);
        assert_eq!(pr.unseen(Some("2024-04-27T09:00:00Z")), 0);
    }
}
