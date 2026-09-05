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

/// `gh`'s state string for a pull request that still accepts work — the
/// one state a branch's PR ROW is kept for, and the one the preview and the
/// detail-driven retirement key on.
pub const STATE_OPEN: &str = "OPEN";

/// Whether a `gh` state string is [`STATE_OPEN`]; drafts are open too, so
/// this alone never says anything about `isDraft`.
fn state_is_open(state: &str) -> bool {
    state == STATE_OPEN
}

/// Run `gh` with `args` (in `dir` when given) under `timeout`, yielding
/// stdout on success. Every failure — no `gh`, bad exit, timeout — is
/// `None`, since each is an ordinary "couldn't ask" to every caller.
async fn gh(dir: Option<&Path>, args: &[&str], timeout: std::time::Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args).stdin(std::process::Stdio::null());
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `v[key]` as a string, `""` when absent or not a string.
fn str_at(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `v[key]` as a number, 0 when absent or not one.
fn u64_at(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// `v[key]` as a flag, false when absent or not one.
fn bool_at(v: &serde_json::Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
}

/// `v[key]` as an array, empty when absent or not one.
fn arr_at<'a>(v: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    v.get(key)
        .and_then(|c| c.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// `v["state"]`, assumed open when `gh` left it out — a PR it lists is
/// open until it says otherwise.
fn state_at(v: &serde_json::Value) -> String {
    v.get("state")
        .and_then(|s| s.as_str())
        .unwrap_or(STATE_OPEN)
        .to_string()
}

/// `v["url"]`, but only when it is something a browser can open. Only
/// http(s) reaches `open(1)`; gh has no business returning anything else,
/// but the row leads straight to a browser so it's checked anyway.
fn web_url(v: &serde_json::Value) -> Option<String> {
    let url = v.get("url")?.as_str()?.to_string();
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

/// The pull request still open on a checkout's branch.
///
/// Always open: `gh pr view` on a branch answers with that branch's most
/// recent pull request whatever its state — a merged one keeps coming back
/// for as long as the branch exists — and this row lives under an OPEN PRS
/// header, so [`parse`] turns a MERGED or CLOSED answer into "no PR" and the
/// row simply goes. A draft is open, just not finished, and stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub is_draft: bool,
    /// When somebody *other than you* commented or submitted a review, as
    /// GitHub's RFC 3339 stamps, oldest first. Those sort lexicographically,
    /// so "posted since the mark we stored" is a string compare — nebula
    /// never has to parse a date or trust a clock.
    pub activity: Vec<String>,
}

impl PullRequest {
    /// Short word for the row's trailing badge — the same slot the agent
    /// rows use for their CLI kind. Every row here is open by construction,
    /// so the only thing left to say is whether it's still a draft.
    pub fn badge(&self) -> &'static str {
        if self.is_draft {
            "draft"
        } else {
            "pr"
        }
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
        marker.map_or(self.activity.len(), |mark| {
            self.activity.iter().filter(|at| at.as_str() > mark).count()
        })
    }
}

/// Ask `gh` for the pull request on `dir`'s current branch. `None` covers
/// every ordinary miss: no PR, no `gh`, no remote, not logged in.
pub async fn lookup(dir: &Path) -> Option<PullRequest> {
    let out = gh(
        Some(dir),
        &[
            "pr",
            "view",
            "--json",
            "number,url,title,state,isDraft,comments,reviews",
        ],
        TIMEOUT,
    )
    .await?;
    // Only asked once `gh` has proved it works, so a machine without it
    // never pays for the extra process.
    parse(&out, viewer_login().await)
}

/// Your own GitHub login, resolved once per process. Needed only to keep
/// your own review submissions out of the unread count: `gh` flags comments
/// with `viewerDidAuthor`, but reviews carry nothing but an author — and
/// replying to an inline thread on your own PR files a review.
static VIEWER: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

async fn viewer_login() -> Option<&'static str> {
    VIEWER
        .get_or_init(|| async {
            let out = gh(None, &["api", "user", "--jq", ".login"], TIMEOUT).await?;
            let login = out.trim().to_string();
            (!login.is_empty()).then_some(login)
        })
        .await
        .as_deref()
}

/// Parse `gh pr view --json …` output. Kept separate from the process call
/// so the shape it expects is testable without a GitHub account. `viewer`
/// is your login when it's known; without it your own reviews count as
/// activity, which is a wrong badge rather than a broken one.
///
/// A pull request that is no longer open parses to `None`. `gh` prefers an
/// open PR when the branch has several, so this only bites once nothing on
/// the branch accepts work any more — exactly when the row should be gone.
fn parse(json: &str, viewer: Option<&str>) -> Option<PullRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let url = web_url(&v)?;
    if !state_is_open(&state_at(&v)) {
        return None;
    }
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        url,
        title: str_at(&v, "title"),
        is_draft: bool_at(&v, "isDraft"),
        activity: activity(&v, viewer),
    })
}

/// Timestamps of everything other people posted on the PR — issue comments
/// and review submissions alike, since either is a reason to go look —
/// sorted oldest first so the last one is the high-water mark.
fn activity(v: &serde_json::Value, viewer: Option<&str>) -> Vec<String> {
    let mut stamps: Vec<String> = Vec::new();
    for c in arr_at(v, "comments") {
        if c.get("viewerDidAuthor").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        if let Some(at) = c.get("createdAt").and_then(|t| t.as_str()) {
            stamps.push(at.to_string());
        }
    }
    for r in arr_at(v, "reviews") {
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
    /// The branch the pull request comes from (`gh`'s `headRefName`) — the
    /// one a PR SESSION's worktree is checked out on, so the row carries it
    /// even though nothing on screen shows it.
    pub head: String,
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

/// What a PR SESSION launch carries from a PROJECT OPEN PRS GROUP row all
/// the way to `ClientRequest::CreatePrAgent`: which pull request the work is
/// scoped to, and the head branch the DAEMON checks its worktree out on.
/// The two travel together — a URL without its branch cannot be launched —
/// so they ride the pickers, the MODEL / EFFORT submenus and the name prompt
/// as one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLaunch {
    pub url: String,
    pub head: String,
}

impl PrLaunch {
    pub fn of(pr: &OpenPr) -> Self {
        Self {
            url: pr.url.clone(),
            head: pr.head.clone(),
        }
    }
}

/// Ask `gh` for every open pull request on `dir`'s repo, newest first.
/// `None` is "couldn't ask" — no `gh`, no remote, not logged in, timed out —
/// and is deliberately distinct from `Some(vec![])`, which is the real
/// answer "nothing is open": the caller keeps the last good list rather than
/// blanking the panel over one failed call.
///
/// Two properties of `--state open` the group depends on:
///
/// * **Drafts are in it.** A draft *is* an open pull request, and it is the
///   one most likely to have a nebula worktree still attached to it — the
///   list would be worth least if it hid exactly the work in progress. They
///   arrive with `isDraft` set and wear a `draft` badge; nothing here or
///   downstream filters them out. Where they land does change: the group
///   sinks them below every finished pull request ([`drafts_last`]), so
///   the rows asking for a reviewer come first.
/// * **Closed ones fall out of it.** This is the whole mechanism for
///   pruning: a pull request that was merged or closed since the last call
///   simply stops coming back, so re-asking on a beat *is* the periodic
///   "should this row still be here?" check. Nothing has to track closures
///   separately.
pub async fn list(dir: &Path) -> Option<Vec<OpenPr>> {
    let limit = LIST_LIMIT.to_string();
    let out = gh(
        Some(dir),
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            &limit,
            "--json",
            "number,url,title,isDraft,headRefName",
        ],
        TIMEOUT,
    )
    .await?;
    parse_list(&out)
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
                let url = web_url(v)?;
                Some(OpenPr {
                    number: v.get("number")?.as_u64()?,
                    title: str_at(v, "title"),
                    url,
                    is_draft: bool_at(v, "isDraft"),
                    head: str_at(v, "headRefName"),
                })
            })
            .collect(),
    )
}

/// Sink the drafts below everything else, keeping `gh`'s newest-first
/// order within each half. A draft is open, but it is not asking anyone for
/// anything yet; the rows that want a reviewer come first, and a draft is
/// told apart by where it sits as much as by its badge. Stable, so the
/// cursor's PR — followed by URL across every refresh — never swaps places
/// with a neighbour it did not change relative to.
pub fn drafts_last(list: &mut [OpenPr]) {
    list.sort_by_key(|pr| pr.is_draft);
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

impl PrDetail {
    /// Whether this pull request still accepts work. A draft counts: it is
    /// open, just not finished. This is the per-row second opinion on the
    /// question [`list`] answers in bulk — when the cursor rests on a row
    /// long enough to fetch its detail, GitHub gets asked about that one
    /// pull request directly, and a `MERGED` or `CLOSED` answer retires the
    /// row without waiting for the next list.
    pub fn is_open(&self) -> bool {
        state_is_open(&self.state)
    }
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
    let number = number.to_string();
    let out = gh(
        Some(dir),
        &[
            "pr",
            "view",
            &number,
            "--json",
            "number,url,title,state,isDraft,author,baseRefName,headRefName,\
             additions,deletions,changedFiles,body,comments,reviews",
        ],
        TIMEOUT,
    )
    .await?;
    parse_detail(&out)
}

fn parse_detail(json: &str) -> Option<PrDetail> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(PrDetail {
        number: v.get("number")?.as_u64()?,
        url: v.get("url")?.as_str()?.to_string(),
        title: str_at(&v, "title"),
        state: state_at(&v),
        is_draft: bool_at(&v, "isDraft"),
        author: login(v.get("author")),
        base: str_at(&v, "baseRefName"),
        head: str_at(&v, "headRefName"),
        additions: u64_at(&v, "additions"),
        deletions: u64_at(&v, "deletions"),
        changed_files: u64_at(&v, "changedFiles"),
        body: str_at(&v, "body"),
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
    let mut out: Vec<PrComment> = Vec::new();
    for c in arr_at(v, "comments") {
        out.push(PrComment {
            author: login(c.get("author")),
            at: str_at(c, "createdAt"),
            review_state: String::new(),
            body: str_at(c, "body"),
        });
    }
    for r in arr_at(v, "reviews") {
        let Some(at) = r.get("submittedAt").and_then(|t| t.as_str()) else {
            continue;
        };
        out.push(PrComment {
            author: login(r.get("author")),
            at: at.to_string(),
            review_state: str_at(r, "state"),
            body: str_at(r, "body"),
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
    gh(
        Some(dir),
        &["pr", "diff", &number.to_string()],
        DIFF_TIMEOUT,
    )
    .await
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
    }

    /// `gh pr view` keeps answering with a branch's pull request after it
    /// is merged or closed. The row it feeds sits under an OPEN PRS header,
    /// so those answers are "no PR"; a draft is open and stays, badged.
    #[test]
    fn a_merged_or_closed_branch_pull_request_is_no_row() {
        let payload = |state: &str, draft: bool| {
            format!(
                r#"{{"number":1,"url":"https://x.dev/pull/1","title":"t","state":"{state}","isDraft":{draft}}}"#
            )
        };
        assert!(parse(&payload("MERGED", false), None).is_none());
        assert!(parse(&payload("CLOSED", false), None).is_none());
        let draft = parse(&payload("OPEN", true), None).expect("a draft is still open");
        assert_eq!(draft.badge(), "draft");
        let open = parse(&payload("OPEN", false), None).expect("open");
        assert_eq!(open.badge(), "pr");
        // An older `gh` that leaves `state` out is trusted to have listed
        // something open.
        let bare = parse(r#"{"number":1,"url":"https://x.dev/pull/1"}"#, None);
        assert!(bare.is_some(), "no state field means open");
    }

    /// Drafts sink below the finished pull requests and keep `gh`'s
    /// newest-first order on both sides of that line.
    #[test]
    fn drafts_sink_below_the_open_rows_in_their_own_order() {
        let row = |number: u64, is_draft: bool| OpenPr {
            number,
            title: String::new(),
            url: format!("https://github.com/o/r/pull/{number}"),
            is_draft,
            head: String::new(),
        };
        let mut list = vec![row(42, true), row(40, false), row(31, true), row(30, false)];
        drafts_last(&mut list);
        let numbers: Vec<u64> = list.iter().map(|p| p.number).collect();
        assert_eq!(numbers, [40, 30, 42, 31]);
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
              {"number":42,"title":"Attach links","url":"https://github.com/o/r/pull/42","isDraft":false,"headRefName":"attach-links"},
              {"number":7,"title":"WIP","url":"https://github.com/o/r/pull/7","isDraft":true}
            ]"#,
        )
        .expect("parsed");
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].label(), "#42 Attach links");
        assert_eq!(prs[0].badge(), "pr");
        assert_eq!(prs[1].badge(), "draft");
        assert_eq!(
            prs[0].head, "attach-links",
            "the branch a PR SESSION runs on"
        );
        assert!(
            prs[1].head.is_empty(),
            "a row `gh` gave no branch for still lists; only its PR SESSION is refused"
        );
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
