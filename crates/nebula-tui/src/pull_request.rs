//! The pull request on a worktree's branch, discovered with the GitHub CLI
//! (`gh pr view`). What it feeds — the pull request on the worktree's
//! BAND — refreshes on its own, so a PR opened outside nebula shows up without
//! anyone typing its URL, and one that has since been merged or closed
//! stays on the row, badged, for as long as the checkout does: the worktree
//! outlives its pull request, and the PR is what you check before archiving
//! or deleting it. Nothing here is the source of truth — GitHub is — but
//! every answer is remembered on disk (`pr_cache`) so the next launch paints
//! the rows from what the last one knew while the lookups catch up. How far
//! the user has read into the conversation is the daemon's (`pr_seen`), so
//! the row can say how many comments landed while they were away.
//!
//! The same `gh` also answers the wider question this module's other half
//! asks — every pull request still open on the *project's* repo, for the
//! PULL REQUESTS MODAL and the header's counts (see [`list`]).
//!
//! `gh` may be missing, unauthenticated, or pointed at a repo with no
//! remote; every one of those is an ordinary "couldn't ask", not an error
//! worth a flash — and, since the last answer is cached, not a reason to
//! blank a row either (see [`Lookup`]). Lookups are async because they hit
//! the network.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// How long a lookup may run before we give up on it. `gh` retries and can
/// hang on a stalled network; the row is a convenience, not worth a task
/// that never ends.
pub(crate) const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// `gh`'s state strings. Only [`STATE_OPEN`] still accepts work: it is the
/// one state the PROJECT OPEN PRS GROUP lists, and the one the preview and
/// the detail-driven retirement key on. The other two are what a branch's
/// PR ROW wears once the work is done.
pub const STATE_OPEN: &str = "OPEN";
pub const STATE_MERGED: &str = "MERGED";
pub const STATE_CLOSED: &str = "CLOSED";

/// Whether a `gh` state string is [`STATE_OPEN`]; drafts are open too, so
/// this alone never says anything about `isDraft`.
fn state_is_open(state: &str) -> bool {
    state == STATE_OPEN
}

/// Where a pull request stands, as a row paints it: the four looks a PR ROW
/// can take, folded from `gh`'s state string and its draft flag. `Merged`
/// and `Closed` win over the flag — a pull request closed while still a
/// draft is closed, which is the more useful thing to say — and anything
/// `gh` might add to its vocabulary later reads as open, the same trust
/// [`state_at`] extends to a missing field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Open,
    Draft,
    Merged,
    Closed,
}

impl Standing {
    pub fn of(state: &str, is_draft: bool) -> Self {
        match state {
            STATE_MERGED => Standing::Merged,
            STATE_CLOSED => Standing::Closed,
            _ if is_draft => Standing::Draft,
            _ => Standing::Open,
        }
    }

    /// The state's name in full, for a surface with the width to spell it
    /// out: the `/` PALETTE's pull request rows. `ready for review` is the
    /// open pull request that is not a draft — GitHub's own phrase for the
    /// step out of draft — and says nothing about approvals, checks or
    /// mergeability; [`badge`](Self::badge) is the same word cut to fit a
    /// sidebar column, so the two surfaces never disagree about a state.
    pub fn label(self) -> &'static str {
        match self {
            Standing::Open => "ready for review",
            Standing::Draft => "draft",
            Standing::Merged => "merged",
            Standing::Closed => "closed",
        }
    }

    /// Short word for a row's trailing badge — the same slot the agent rows
    /// use for their CLI kind. `ready` is [`label`](Self::label)'s `ready
    /// for review` at sidebar width: the SESSIONS PANEL's PR ROW keeps a
    /// title beside it in a 32-column default, which the full phrase would
    /// leave ten cells for.
    pub fn badge(self) -> &'static str {
        match self {
            Standing::Open => "ready",
            Standing::Draft => "draft",
            Standing::Merged => "merged",
            Standing::Closed => "closed",
        }
    }
}

/// How a pull request's checks stand, folded from `gh`'s
/// `statusCheckRollup` the way `gh pr checks` folds it ([`checks`]): one
/// failure fails the lot, one still running leaves the lot pending, and a
/// repo that runs no checks at all has nothing to say. Only `Failing` is
/// trouble; the rest is what the PR PREVIEW spells out beside the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Checks {
    #[default]
    Absent,
    Pending,
    Passing,
    Failing,
}

/// What GitHub says stands between a pull request and its merge button:
/// whether the branch still merges cleanly, and how its checks stand.
/// Read off every payload — branch row, list row and detail alike — so
/// the three surfaces go red together, and remembered with the row
/// (`pr_cache`); a document written before it was asked reads as healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Health {
    /// `gh`'s `mergeable` said `CONFLICTING`: the branch no longer merges
    /// and a person has to resolve it. `MERGEABLE` reads as clean, and so
    /// does `UNKNOWN` — GitHub computes mergeability lazily, so the first
    /// ask after a push often says so; the next beat answers.
    pub conflicts: bool,
    pub checks: Checks,
}

impl Health {
    /// The one word the row goes red for, if any. Conflicts outrank
    /// failing checks when both hold: checks cannot be trusted on a branch
    /// that no longer merges, and the rebase that resolves the conflict
    /// re-runs them anyway.
    pub fn trouble(self) -> Option<Trouble> {
        if self.conflicts {
            Some(Trouble::Conflicts)
        } else if self.checks == Checks::Failing {
            Some(Trouble::FailingChecks)
        } else {
            None
        }
    }
}

/// Why a pull request row is red: something GitHub says blocks the merge
/// and needs a person — the same red the STATUS DOT wears on a session
/// that needs someone. An open pull request's alone: a merged or closed
/// one is past needing its branch resolved, whatever the answer says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    Conflicts,
    FailingChecks,
}

impl Trouble {
    /// Spelled out, for the `/` PALETTE and the PR PREVIEW, where there
    /// is room: `merge conflicts`, `checks failing`.
    pub fn label(self) -> &'static str {
        match self {
            Trouble::Conflicts => "merge conflicts",
            Trouble::FailingChecks => "checks failing",
        }
    }

    /// The sidebar's badge — [`label`](Self::label) cut to the column, the
    /// way [`Standing::badge`] cuts `ready for review` to `ready`, so a
    /// title still fits beside it at the SESSIONS PANEL's default width.
    pub fn badge(self) -> &'static str {
        match self {
            Trouble::Conflicts => "conflicts",
            Trouble::FailingChecks => "failing",
        }
    }
}

/// Run `gh` with `args` (in `dir` when given) under `timeout`, yielding
/// stdout on success. Every failure — no `gh`, bad exit, timeout — is
/// `None`, since each is an ordinary "couldn't ask" to every caller.
pub(crate) async fn gh(
    dir: Option<&Path>,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    run_gh(dir, args, timeout).await.ok()
}

/// [`gh`] with the failure kept: `Err` carries what `gh` printed to stderr
/// on a bad exit, and nothing at all when it could not be run or timed
/// out. Only [`lookup`] reads it — `gh pr view` says "no pull request" and
/// "no network" with the same exit code and only the message apart.
async fn run_gh(
    dir: Option<&Path>,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args).stdin(std::process::Stdio::null());
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(_)) | Err(_) => return Err(String::new()),
    };
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `v[key]` as a string, `""` when absent or not a string.
pub(crate) fn str_at(v: &serde_json::Value, key: &str) -> String {
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
pub(crate) fn web_url(v: &serde_json::Value) -> Option<String> {
    let url = v.get("url")?.as_str()?.to_string();
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

/// The pull request on a checkout's branch, whatever state it is in.
///
/// `gh pr view` on a branch answers with that branch's most recent pull
/// request — preferring an open one, and a merged or closed one for as long
/// as the branch exists once nothing on it accepts work any more — and the
/// row keeps every answer. A checkout whose PR has shipped is exactly the
/// one about to be archived or deleted, and the PR is what gets checked
/// first, so the row stays put and its badge says `merged` or `closed`
/// instead. The PROJECT OPEN PRS GROUP is where closed pull requests fall
/// out; this row is per checkout, and the checkout is still here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub title: String,
    /// `gh`'s state string — [`STATE_OPEN`], [`STATE_MERGED`] or
    /// [`STATE_CLOSED`].
    pub state: String,
    pub is_draft: bool,
    /// Whether the branch still merges and its checks pass — what turns
    /// the row red ([`trouble`](Self::trouble)).
    #[serde(default)]
    pub health: Health,
    /// When somebody *other than you* commented or submitted a review, as
    /// GitHub's RFC 3339 stamps, oldest first. Those sort lexicographically,
    /// so "posted since the mark we stored" is a string compare — nebula
    /// never has to parse a date or trust a clock.
    pub activity: Vec<String>,
}

impl PullRequest {
    /// Whether this pull request still accepts work. A draft counts.
    pub fn is_open(&self) -> bool {
        state_is_open(&self.state)
    }

    /// The look the row takes: open, draft, merged or closed.
    pub fn standing(&self) -> Standing {
        Standing::of(&self.state, self.is_draft)
    }

    /// What the row goes red for, while the pull request is still open:
    /// a merged or closed one is past needing its branch resolved.
    pub fn trouble(&self) -> Option<Trouble> {
        self.is_open().then(|| self.health.trouble()).flatten()
    }

    /// The mark to store when the user opens this PR: everything nebula
    /// currently knows about has been read. Empty when nobody has posted —
    /// which still beats no mark at all, since every real stamp sorts above
    /// it, so the next comment to land counts as new.
    pub fn seen_marker(&self) -> &str {
        self.activity.last().map(String::as_str).unwrap_or("")
    }
}

/// What a branch lookup came back with. The two misses are kept apart
/// because the row they feed is remembered across launches (`pr_cache`):
/// a branch GitHub says has no pull request clears its row, while a call
/// that never reached GitHub leaves whatever the row last showed — the
/// last known state of a pull request is worth more than a blank, and an
/// offline launch must not wipe out every badge within a sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(PullRequest),
    /// `gh` answered: the branch has no pull request at all.
    Absent,
    /// `gh` couldn't answer — missing, not logged in, no remote, no
    /// network, a detached checkout, a timeout.
    Unavailable,
}

/// The one message `gh pr view` prints for a branch with no pull request
/// (`no pull requests found for branch "x"`). Everything else it can fail
/// with is a reason it couldn't ask, not an answer.
const NO_PR_MARKER: &str = "no pull requests found";

/// Sort a failed `gh pr view` into [`Lookup::Absent`] or
/// [`Lookup::Unavailable`] by what it printed.
pub(crate) fn classify_miss(stderr: &str) -> Lookup {
    if stderr.contains(NO_PR_MARKER) {
        Lookup::Absent
    } else {
        Lookup::Unavailable
    }
}

/// Ask `gh` for the pull request on `dir`'s current branch.
pub async fn lookup(dir: &Path) -> Lookup {
    let out = run_gh(
        Some(dir),
        &[
            "pr",
            "view",
            "--json",
            "number,url,title,state,isDraft,mergeable,statusCheckRollup,comments,reviews",
        ],
        TIMEOUT,
    )
    .await;
    match out {
        // Only asked once `gh` has proved it works, so a machine without
        // it never pays for the extra process.
        Ok(out) => match parse(&out, viewer_login().await) {
            Some(pr) => Lookup::Found(pr),
            None => Lookup::Absent,
        },
        Err(stderr) => classify_miss(&stderr),
    }
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
/// A merged or closed pull request parses like an open one, state and all.
/// `gh` prefers an open PR when the branch has several, so a closed answer
/// only arrives once nothing on the branch accepts work any more — and
/// that is the pull request the checkout's row should keep showing, not
/// hide, until the checkout itself goes.
fn parse(json: &str, viewer: Option<&str>) -> Option<PullRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let url = web_url(&v)?;
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        url,
        title: str_at(&v, "title"),
        state: state_at(&v),
        is_draft: bool_at(&v, "isDraft"),
        health: health(&v),
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

/// The pull request's [`Health`] as a `gh pr view` / `gh pr list` payload
/// carries it: `mergeable` and `statusCheckRollup`, either missing when the
/// caller did not ask for it, which reads as healthy.
fn health(v: &serde_json::Value) -> Health {
    Health {
        conflicts: str_at(v, "mergeable") == "CONFLICTING",
        checks: checks(arr_at(v, "statusCheckRollup")),
    }
}

/// Fold a `statusCheckRollup` — one entry per check run or commit status
/// on the head commit — the way `gh pr checks` does. A check run's word is
/// its `conclusion` once `COMPLETED` and its `status` (queued, in
/// progress) until then; a plain commit status carries a `state` instead.
/// Any failure fails the lot; else anything still running leaves the lot
/// pending; else it passes — a skipped or neutral job is not a failure.
fn checks(rollup: &[serde_json::Value]) -> Checks {
    let mut out = Checks::Absent;
    for entry in rollup {
        let word = if entry.get("state").is_some() {
            str_at(entry, "state")
        } else if str_at(entry, "status") == "COMPLETED" {
            str_at(entry, "conclusion")
        } else {
            str_at(entry, "status")
        };
        let one = match word.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => Checks::Passing,
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => return Checks::Failing,
            _ => Checks::Pending,
        };
        out = match (out, one) {
            (Checks::Pending, _) | (_, Checks::Pending) => Checks::Pending,
            _ => Checks::Passing,
        };
    }
    out
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub is_draft: bool,
    /// Whether the branch still merges and its checks pass — what turns
    /// the row red ([`trouble`](Self::trouble)).
    #[serde(default)]
    pub health: Health,
    /// The local branch the pull request's checkout is on
    /// ([`checkout_branch`]) — the one a PR SESSION's worktree is cut on,
    /// the one a checkout has to be on to list under this row, and what
    /// `CreatePrAgent` carries as `head`. A same-repo pull request's is
    /// its head branch (`gh`'s `headRefName`); a fork's is
    /// `<owner>/<headRefName>`, because a fork's branch shares nothing but
    /// a name with ours — `main` above all, which every fork has and the
    /// ROOT WORKTREE is on.
    pub head: String,
}

/// `#42 title`, or `#42` alone for an untitled one — how a pull request
/// or an issue is named on a row.
pub fn numbered_label(number: u64, title: &str) -> String {
    if title.is_empty() {
        format!("#{number}")
    } else {
        format!("#{number} {title}")
    }
}

impl OpenPr {
    /// Row text: `#42 title`.
    pub fn label(&self) -> String {
        numbered_label(self.number, &self.title)
    }

    /// Open or draft — every row here is open by construction (`list` asks
    /// for nothing else), so the only thing left to say is whether it's
    /// still a draft.
    pub fn standing(&self) -> Standing {
        Standing::of(STATE_OPEN, self.is_draft)
    }

    /// Trailing badge: `ready` or `draft`.
    pub fn badge(&self) -> &'static str {
        self.standing().badge()
    }

    /// What the row goes red for — every row here is open, so the
    /// health's word is the row's.
    pub fn trouble(&self) -> Option<Trouble> {
        self.health.trouble()
    }
}

/// What a PR SESSION launch carries from a PROJECT OPEN PRS GROUP row all
/// the way to `ClientRequest::CreatePrAgent`: which pull request the work is
/// scoped to, and the branch the DAEMON checks its worktree out on
/// (`OpenPr::head` — the head branch, under its owner's name for a fork).
/// The two travel together — a URL without its branch cannot be launched —
/// so they ride the pickers, the MODEL / EFFORT submenus and the name prompt
/// as one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLaunch {
    pub url: String,
    pub head: String,
    /// The pull request's number — what the QUICK PROMPT's title and
    /// target row call it (`PR #42`).
    pub number: u64,
}

impl PrLaunch {
    pub fn of(pr: &OpenPr) -> Self {
        Self {
            url: pr.url.clone(),
            head: pr.head.clone(),
            number: pr.number,
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
            "number,url,title,isDraft,headRefName,isCrossRepository,headRepositoryOwner,\
             mergeable,statusCheckRollup",
        ],
        TIMEOUT,
    )
    .await?;
    parse_list(&out)
}

/// The local branch a pull request's checkout is on — [`OpenPr::head`].
///
/// A same-repo pull request's is its head branch: the DAEMON fetches it
/// from `origin` and the checkout tracks it. A fork's head branch is not
/// ours, whatever it is called, and the name alone cannot tell the two
/// apart: a contributor's pull request from their fork's `main` matched
/// the ROOT WORKTREE (on our `main`), so its PR SESSION ran in the main
/// checkout, on our code, and nothing was cut or nested. So a fork's
/// checkout is on `<owner>/<headRefName>` — `givemeurhats/main`, the name
/// `gh pr checkout` gives that same collision — which no branch of ours
/// has, which `origin` has no branch for (the DAEMON seeds it from
/// `refs/pull/N/head`), and which says on the row whose code the checkout
/// holds. A fork that has since been deleted leaves no owner to name:
/// `pr-<number>/<headRefName>`.
fn checkout_branch(v: &serde_json::Value) -> String {
    let head = str_at(v, "headRefName");
    if head.is_empty() || !bool_at(v, "isCrossRepository") {
        return head;
    }
    let owner = v
        .get("headRepositoryOwner")
        .and_then(|owner| owner.get("login"))
        .and_then(|login| login.as_str())
        .filter(|login| !login.is_empty());
    match owner {
        Some(owner) => format!("{owner}/{head}"),
        None => format!("pr-{}/{head}", u64_at(v, "number")),
    }
}

/// Parse `gh pr list --json …` output — a bare array. Kept separate from
/// the process call so the shape it expects is testable without a GitHub
/// account. A row whose url could never be opened is dropped rather than
/// failing the whole list; a payload that isn't an array at all is a miss.
pub(crate) fn parse_list(json: &str) -> Option<Vec<OpenPr>> {
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
                    health: health(v),
                    head: checkout_branch(v),
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
/// actually rests on — and cached for the session and across launches,
/// because this is a second API call on top of the list and the body of a
/// merged-or-not pull request does not change while you read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDetail {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: String,
    pub is_draft: bool,
    /// Whether the branch still merges and its checks pass — spelled out
    /// beside the state, and what the row it was fetched for goes red for.
    #[serde(default)]
    pub health: Health,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            "number,url,title,state,isDraft,mergeable,statusCheckRollup,author,\
             baseRefName,headRefName,additions,deletions,changedFiles,body,\
             comments,reviews",
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
        health: health(&v),
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

/// `author.login`, `""` when absent.
pub(crate) fn login(author: Option<&serde_json::Value>) -> String {
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

/// How long a `gh pr comment` may run. The body is small and the call
/// is one request, so the metadata budget serves — but the person who
/// pressed Enter is watching the footer, so it is bounded all the same.
const COMMENT_TIMEOUT: std::time::Duration = TIMEOUT;

/// What a post that never reached `gh`'s own words flashes: the binary
/// could not be run, or it ran and stalled past [`COMMENT_TIMEOUT`].
pub const GH_NOT_RUN: &str = "gh could not be run";
pub const GH_TIMED_OUT: &str = "gh timed out";

/// Post `body` as an issue comment on pull request `number`, from a
/// checkout of its repo, as whoever `gh` is logged in as. `Ok` carries the
/// URL `gh` prints for the new comment; `Err` carries the reason it did
/// not post — `gh`'s own first line when it ran and refused, or
/// [`GH_NOT_RUN`] / [`GH_TIMED_OUT`] when it never answered — because "not
/// logged in" and "no network" are different things to tell the person
/// still holding the text.
///
/// The body crosses on stdin (`--body-file -`) rather than in argv: a
/// comment is markdown written by a person and may be long, start with a
/// dash, or hold anything else an argument parser would misread.
pub async fn comment(dir: &Path, number: u64, body: &str) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let number = number.to_string();
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["pr", "comment", &number, "--body-file", "-"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let run = async {
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        // A `gh` that exits before reading the pipe (bad auth, a usage
        // error) closes its end and the write fails; its stderr says why,
        // and `wait_with_output` is what reads that.
        let _ = stdin.write_all(body.as_bytes()).await;
        drop(stdin);
        child.wait_with_output().await.ok()
    };
    let out = match tokio::time::timeout(COMMENT_TIMEOUT, run).await {
        Ok(Some(out)) => out,
        Ok(None) => return Err(GH_NOT_RUN.into()),
        Err(_) => return Err(GH_TIMED_OUT.into()),
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(comment_error(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// What `gh pr comment` printed when it refused, cut to the one line worth
/// flashing: its first non-empty line, or [`GH_NOT_RUN`] when it printed
/// nothing at all.
pub fn comment_error(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| GH_NOT_RUN.to_string())
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

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl PullRequest {
    /// Short word for the row's trailing badge — the same slot the agent
    /// rows use for their CLI kind: `ready`, `draft`, `merged` or `closed`.
    pub fn badge(&self) -> &'static str {
        self.standing().badge()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a refused `gh pr comment` flashes is its first line — the one
    /// that says "not logged in" or "could not resolve" — and a `gh` that
    /// printed nothing gets the fixed "could not be run" line.
    #[test]
    fn a_comment_refusal_flashes_ghs_first_line() {
        assert_eq!(
            comment_error("\nerror: not logged in to github.com\nTo log in, run: gh auth login\n"),
            "error: not logged in to github.com"
        );
        assert_eq!(comment_error("   \n\n"), GH_NOT_RUN);
        assert_eq!(comment_error(""), GH_NOT_RUN);
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
        assert_eq!(pr.badge(), "ready");
        assert!(pr.is_open());
    }

    /// GitHub's word on whether the branch still merges (`mergeable`) and
    /// how its checks stand (`statusCheckRollup`) rides every payload —
    /// branch row, list row and detail alike — into the same `Health`, so
    /// the three surfaces go red together. `UNKNOWN` mergeability, what
    /// GitHub says while it is still computing, is not a conflict.
    #[test]
    fn health_reads_conflicts_and_checks_off_every_payload() {
        let view = r#"{"number":7,"url":"https://github.com/o/r/pull/7","title":"t","state":"OPEN","isDraft":false,"mergeable":"CONFLICTING","statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}]}"#;
        let pr = parse(view, None).expect("parsed");
        assert!(pr.health.conflicts);
        assert_eq!(pr.health.checks, Checks::Passing);
        assert_eq!(pr.trouble(), Some(Trouble::Conflicts));

        let list = r#"[{"number":8,"url":"https://github.com/o/r/pull/8","title":"t","isDraft":false,"headRefName":"h","mergeable":"UNKNOWN","statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}]}]"#;
        let rows = parse_list(list).expect("parsed");
        assert!(!rows[0].health.conflicts, "UNKNOWN is not a conflict");
        assert_eq!(rows[0].health.checks, Checks::Failing);
        assert_eq!(rows[0].trouble(), Some(Trouble::FailingChecks));

        let detail = r#"{"number":9,"url":"https://github.com/o/r/pull/9","state":"OPEN","mergeable":"MERGEABLE","statusCheckRollup":[]}"#;
        let d = parse_detail(detail).expect("parsed");
        assert_eq!(d.health, Health::default());
        assert_eq!(d.health.trouble(), None);

        // Not asked for — an older payload shape — reads as healthy.
        let bare = parse(
            r#"{"number":1,"url":"https://x.dev/pull/1","state":"OPEN"}"#,
            None,
        )
        .expect("parsed");
        assert_eq!(bare.health, Health::default());
    }

    /// The rollup folds the way `gh pr checks` folds it: any failure fails
    /// the lot, else anything still running leaves it pending, else it
    /// passes — a skipped or neutral job counts as a pass — and a plain
    /// commit status (`StatusContext`, a `state` rather than a conclusion)
    /// is read by the same words. No checks at all is nothing to say.
    #[test]
    fn checks_fold_like_gh_pr_checks() {
        let run = |status: &str, conclusion: &str| serde_json::json!({"__typename":"CheckRun","status":status,"conclusion":conclusion});
        let ctx = |state: &str| serde_json::json!({"__typename":"StatusContext","state":state});
        assert_eq!(checks(&[]), Checks::Absent);
        assert_eq!(
            checks(&[
                run("COMPLETED", "SUCCESS"),
                run("COMPLETED", "SKIPPED"),
                run("COMPLETED", "NEUTRAL"),
            ]),
            Checks::Passing
        );
        assert_eq!(
            checks(&[run("COMPLETED", "SUCCESS"), run("IN_PROGRESS", "")]),
            Checks::Pending
        );
        assert_eq!(
            checks(&[run("QUEUED", ""), run("COMPLETED", "FAILURE")]),
            Checks::Failing,
            "one failure fails the lot, whatever is still running"
        );
        for word in [
            "FAILURE",
            "ERROR",
            "CANCELLED",
            "TIMED_OUT",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
        ] {
            assert_eq!(checks(&[run("COMPLETED", word)]), Checks::Failing, "{word}");
        }
        assert_eq!(checks(&[ctx("SUCCESS")]), Checks::Passing);
        assert_eq!(checks(&[ctx("PENDING")]), Checks::Pending);
        assert_eq!(checks(&[ctx("ERROR")]), Checks::Failing);
    }

    /// Trouble is an open pull request's: a merged or closed one is past
    /// needing its branch resolved, whatever the cached answer says; a
    /// draft's conflict still needs a person. Conflicts outrank failing
    /// checks when both hold.
    #[test]
    fn trouble_is_an_open_pull_requests_and_conflicts_come_first() {
        let both = Health {
            conflicts: true,
            checks: Checks::Failing,
        };
        assert_eq!(both.trouble(), Some(Trouble::Conflicts));
        let pending = Health {
            conflicts: false,
            checks: Checks::Pending,
        };
        assert_eq!(pending.trouble(), None, "still running is not failing");

        let mut pr = parse(
            r#"{"number":7,"url":"https://github.com/o/r/pull/7","state":"MERGED","mergeable":"CONFLICTING"}"#,
            None,
        )
        .expect("parsed");
        assert!(pr.health.conflicts, "the answer is kept as given");
        assert_eq!(
            pr.trouble(),
            None,
            "a merged pull request is not in trouble"
        );
        pr.state = STATE_OPEN.into();
        assert_eq!(pr.trouble(), Some(Trouble::Conflicts));
        pr.is_draft = true;
        assert_eq!(
            pr.trouble(),
            Some(Trouble::Conflicts),
            "a draft's conflict still needs resolving"
        );
    }

    /// The palette and the preview spell the trouble out; the sidebar's
    /// badge is the same word cut to fit its column.
    #[test]
    fn the_palette_label_and_the_sidebar_badge_name_the_same_trouble() {
        assert_eq!(Trouble::Conflicts.label(), "merge conflicts");
        assert_eq!(Trouble::Conflicts.badge(), "conflicts");
        assert_eq!(Trouble::FailingChecks.label(), "checks failing");
        assert_eq!(Trouble::FailingChecks.badge(), "failing");
    }

    /// `gh pr view` keeps answering with a branch's pull request after it
    /// is merged or closed, and the row keeps it: the checkout is still
    /// here, and its pull request is what gets checked before the checkout
    /// is archived or deleted. The badge says what became of it; a draft is
    /// open and reads as one, unless it was closed as a draft, in which case
    /// closed is the more useful word.
    #[test]
    fn a_merged_or_closed_branch_pull_request_keeps_its_row_badged() {
        let payload = |state: &str, draft: bool| {
            format!(
                r#"{{"number":1,"url":"https://x.dev/pull/1","title":"t","state":"{state}","isDraft":{draft}}}"#
            )
        };
        let merged = parse(&payload("MERGED", false), None).expect("a merged PR is still a row");
        assert_eq!(merged.badge(), "merged");
        assert_eq!(merged.standing(), Standing::Merged);
        assert!(!merged.is_open());
        let closed = parse(&payload("CLOSED", false), None).expect("a closed PR is still a row");
        assert_eq!(closed.badge(), "closed");
        assert!(!closed.is_open());
        let closed_draft = parse(&payload("CLOSED", true), None).expect("closed");
        assert_eq!(
            closed_draft.standing(),
            Standing::Closed,
            "closed beats draft"
        );
        let draft = parse(&payload("OPEN", true), None).expect("a draft is still open");
        assert_eq!(draft.badge(), "draft");
        assert!(draft.is_open());
        let open = parse(&payload("OPEN", false), None).expect("open");
        assert_eq!(open.badge(), "ready");
        // An older `gh` that leaves `state` out is trusted to have listed
        // something open.
        let bare = parse(r#"{"number":1,"url":"https://x.dev/pull/1"}"#, None)
            .expect("no state field means open");
        assert_eq!(bare.standing(), Standing::Open);
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
            health: Default::default(),
            head: String::new(),
        };
        let mut list = vec![row(42, true), row(40, false), row(31, true), row(30, false)];
        drafts_last(&mut list);
        let numbers: Vec<u64> = list.iter().map(|p| p.number).collect();
        assert_eq!(numbers, [40, 30, 42, 31]);
    }

    /// One vocabulary for a pull request's state, in two lengths: the
    /// PALETTE spells `ready for review` out, the sidebar badge is the same
    /// word cut to `ready`, and every other state is the same word at both
    /// lengths — so a draft reads `draft` wherever it is, and nothing
    /// anywhere says `open` or `pr` for a state another surface names
    /// differently.
    #[test]
    fn the_palette_label_and_the_sidebar_badge_name_the_same_state() {
        assert_eq!(Standing::Open.label(), "ready for review");
        assert_eq!(Standing::Open.badge(), "ready");
        assert!(
            Standing::Open.label().starts_with(Standing::Open.badge()),
            "the badge is the label's first word"
        );
        for standing in [Standing::Draft, Standing::Merged, Standing::Closed] {
            assert_eq!(standing.label(), standing.badge(), "{standing:?}");
        }
        assert_eq!(Standing::Draft.label(), "draft");
        assert_ne!(
            Standing::Open.label(),
            Standing::Draft.label(),
            "a draft and a ready pull request are told apart by the word"
        );
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
        assert_eq!(prs[0].badge(), "ready");
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

    /// A fork's head branch is not ours, whatever it is called — `main`
    /// above all, which the ROOT WORKTREE is on: its checkout is on
    /// `<owner>/<branch>`, so it matches no branch of ours. A same-repo
    /// pull request keeps its branch's own name, slashes and all, and a
    /// fork since deleted is named for the pull request.
    #[test]
    fn a_forks_checkout_branch_carries_its_owner() {
        let prs = parse_list(
            r#"[
              {"number":129,"title":"Prefer PowerShell 7","url":"https://github.com/o/r/pull/129","isDraft":false,
               "headRefName":"main","isCrossRepository":true,"headRepositoryOwner":{"login":"givemeurhats"}},
              {"number":131,"title":"Settings hotkey","url":"https://github.com/o/r/pull/131","isDraft":false,
               "headRefName":"feat/settings-open-hotkey","isCrossRepository":true,"headRepositoryOwner":{"login":"wende"}},
              {"number":139,"title":"Cyrillic","url":"https://github.com/o/r/pull/139","isDraft":false,
               "headRefName":"dependabot/cargo/serde-2","isCrossRepository":false,"headRepositoryOwner":{"login":"o"}},
              {"number":140,"title":"Orphan","url":"https://github.com/o/r/pull/140","isDraft":false,
               "headRefName":"main","isCrossRepository":true,"headRepositoryOwner":null}
            ]"#,
        )
        .expect("parsed");
        let heads: Vec<&str> = prs.iter().map(|pr| pr.head.as_str()).collect();
        assert_eq!(
            heads,
            [
                "givemeurhats/main",
                "wende/feat/settings-open-hotkey",
                "dependabot/cargo/serde-2",
                "pr-140/main",
            ]
        );
        assert_eq!(PrLaunch::of(&prs[0]).head, "givemeurhats/main");
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

    /// `gh pr view` exits 1 both for a branch with no pull request and for
    /// a network it couldn't reach; only the message tells them apart. The
    /// first clears the row, the second keeps whatever it last showed —
    /// including the row a previous launch cached.
    #[test]
    fn a_failed_view_is_absent_only_when_gh_says_no_pull_request() {
        assert_eq!(
            classify_miss("no pull requests found for branch \"main\"\n"),
            Lookup::Absent
        );
        assert_eq!(
            classify_miss(
                "Post \"https://api.github.com/graphql\": dial tcp: connect: connection refused\n"
            ),
            Lookup::Unavailable
        );
        assert_eq!(
            classify_miss("could not determine current branch: not on any branch\n"),
            Lookup::Unavailable,
            "a detached checkout mid-rebase keeps its row"
        );
        assert_eq!(
            classify_miss(""),
            Lookup::Unavailable,
            "no gh at all, or a timeout"
        );
    }
}
