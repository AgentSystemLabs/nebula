//! The check behind the FOOTER's update indicator: is a newer nebula
//! published on GitHub than the one running?
//!
//! "Latest" is resolved the way `install.sh` resolves it — GitHub's
//! `releases/latest` URL answers with a redirect to `/releases/tag/vX.Y.Z`
//! (drafts and pre-releases excluded), so one `HEAD` request names the
//! newest release. That is a plain web redirect, not an API call: it spends
//! no `gh` token (the Claude sessions on this box share that token with the
//! pull-request poll) and none of the API quota. The probe is `curl`, which
//! `install.sh` and `nebula upgrade` already require.
//!
//! The indicator is a nudge, not a clock: a check runs at start and then on
//! a slow beat, off the event loop, and every failure — no network, no
//! curl, a redirect that is not a release tag — is "no news", not an error
//! worth a flash. Nothing here installs anything; `nebula upgrade` does.

use std::time::Duration;

/// GitHub's "latest release" page for this repo.
pub const LATEST_URL: &str = "https://github.com/AgentSystemLabs/nebula/releases/latest";

/// How often the check re-runs once the TUI is up; the first runs at start.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// How long the probe may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The check's cadence: `NEBULA_UPDATE_CHECK_SECS` when set, `None` when it
/// is `0` (off — the e2e tests, whose footers must not depend on what
/// GitHub has published), else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    match nebula_core::env::non_empty(nebula_core::env::UPDATE_CHECK_SECS) {
        None => Some(DEFAULT_INTERVAL),
        Some(v) => match v.parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => Some(DEFAULT_INTERVAL),
        },
    }
}

/// Run one check off the loop. `tx` hears the newer version when there is
/// one and nothing otherwise, so a re-check that can't ask never clears an
/// indicator an earlier one lit (a release does not un-publish).
pub fn spawn(tx: tokio::sync::mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        if let Some(version) = newer_release().await {
            let _ = tx.send(version);
        }
    });
}

/// The latest published version (`0.22.0`) when it is strictly newer than
/// this build; `None` for up to date, ahead of it (a dev build past the
/// last release), or couldn't ask.
pub async fn newer_release() -> Option<String> {
    let redirect = probe(LATEST_URL).await?;
    let tag = tag_from_redirect(&redirect)?;
    newer_than(tag, env!("CARGO_PKG_VERSION"))
}

/// Where `url` redirects to, without following it — GitHub's answer to
/// `releases/latest` is the redirect itself. Not a redirect, and every
/// failure, are `None`.
async fn probe(url: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-sS",
        "-I", // HEAD: the Location header is the whole answer
        "-o",
        "/dev/null",
        "-w",
        "%{redirect_url}",
        "--max-time",
        "15",
        url,
    ])
    .stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let redirect = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!redirect.is_empty()).then_some(redirect)
}

/// The tag a `…/releases/tag/<tag>` URL names; `None` for any other shape
/// (a repo with no releases yet redirects to `/releases`).
pub fn tag_from_redirect(url: &str) -> Option<&str> {
    let (_, tag) = url.split_once("/releases/tag/")?;
    let tag = tag.trim_end_matches('/');
    (!tag.is_empty()).then_some(tag)
}

/// `vX.Y.Z` / `X.Y.Z` as its three numbers; anything else — a pre-release
/// suffix, a two-part tag — is `None`, so it never compares.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// `published` normalised to `X.Y.Z` when it is strictly newer than
/// `running`.
pub fn newer_than(published: &str, running: &str) -> Option<String> {
    let p = parse_version(published)?;
    let r = parse_version(running)?;
    (p > r).then(|| format!("{}.{}.{}", p.0, p.1, p.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_names_the_tag() {
        let base = "https://github.com/AgentSystemLabs/nebula";
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases/tag/v0.22.0")),
            Some("v0.22.0")
        );
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases/tag/v0.22.0/")),
            Some("v0.22.0")
        );
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases")),
            None,
            "no releases yet"
        );
        assert_eq!(tag_from_redirect(""), None);
    }

    #[test]
    fn versions_parse_with_or_without_the_v() {
        assert_eq!(parse_version("v0.22.0"), Some((0, 22, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(
            parse_version("v0.22.0-rc1"),
            None,
            "pre-releases never compare"
        );
        assert_eq!(parse_version("v0.22"), None);
        assert_eq!(parse_version("v0.22.0.1"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn only_a_strictly_newer_release_counts() {
        assert_eq!(newer_than("v0.22.0", "0.21.0").as_deref(), Some("0.22.0"));
        assert_eq!(newer_than("v1.0.0", "0.99.9").as_deref(), Some("1.0.0"));
        assert_eq!(
            newer_than("v0.21.10", "0.21.9").as_deref(),
            Some("0.21.10"),
            "numeric, not lexical"
        );
        assert_eq!(newer_than("v0.21.0", "0.21.0"), None, "up to date");
        assert_eq!(
            newer_than("v0.21.0", "0.22.0"),
            None,
            "a dev build ahead of the last release"
        );
        assert_eq!(newer_than("garbage", "0.21.0"), None);
    }

    /// One test for every state of the override, since it is one
    /// process-wide variable nothing else in this crate reads.
    #[test]
    fn interval_reads_the_env_override() {
        let var = nebula_core::env::UPDATE_CHECK_SECS;
        std::env::remove_var(var);
        assert_eq!(interval(), Some(DEFAULT_INTERVAL));
        std::env::set_var(var, "0");
        assert_eq!(interval(), None, "0 turns it off");
        std::env::set_var(var, "90");
        assert_eq!(interval(), Some(Duration::from_secs(90)));
        std::env::set_var(var, "soon");
        assert_eq!(
            interval(),
            Some(DEFAULT_INTERVAL),
            "nonsense is the default"
        );
        std::env::remove_var(var);
    }
}
