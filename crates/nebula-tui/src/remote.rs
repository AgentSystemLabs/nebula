//! Where a checkout lives on the web, derived from its git remote.
//!
//! Every hosting form of the same repository — `git@github.com:o/r.git`,
//! `ssh://git@github.com/o/r`, `https://github.com/o/r.git` — points at one
//! browsable page, so the hotkey works out that page rather than asking the
//! user to keep a link around for it.
//!
//! Synchronous `std::process` like git_diff.rs: this runs on a key press,
//! and `git remote` is a config read with no network in it.
//!
//! Two details the naive `s/git@/https:\/\//` version gets wrong:
//!
//! * **Credentials.** A remote cloned with a token in it
//!   (`https://x:ghp_…@github.com/o/r`) would hand that token to the
//!   browser — and to its history and sync. The userinfo is dropped.
//!
//! * **Local clones.** `/srv/git/repo` and `../sibling` are perfectly good
//!   remotes with no web page at all. Those are a flash, not a bad URL.

use std::path::Path;
use std::process::Command;

/// The web page for the repository checked out at `root`. `Err` is a
/// user-facing flash message.
pub fn repo_url(root: &Path) -> Result<String, String> {
    let remote = configured_remote(root)?;
    web_url(&remote).ok_or_else(|| format!("remote has no web page: {remote}"))
}

/// The remote to follow: `origin` when it exists, else whichever one git
/// lists first. A repo with several remotes and no `origin` is rare enough
/// that guessing beats a picker.
fn configured_remote(root: &Path) -> Result<String, String> {
    if let Some(url) = remote_url(root, "origin") {
        return Ok(url);
    }
    let names = run_git(root, &["remote"])?;
    let first = names
        .lines()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .ok_or_else(|| "no git remote on this repo".to_string())?;
    remote_url(root, first).ok_or_else(|| format!("no URL configured for remote {first}"))
}

fn remote_url(root: &Path, name: &str) -> Option<String> {
    let url = run_git(root, &["remote", "get-url", name]).ok()?;
    let url = url.trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// `git -C root <args>`, stdout on success. A failing git (no remote by
/// that name, not a repo) is `Err` carrying its own complaint.
fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(match stderr.trim() {
            "" => format!("git {} failed", args.join(" ")),
            msg => msg.to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A git remote as a browser address, or `None` when it doesn't name one:
/// a local path, a `file://` clone, an unfamiliar transport.
pub fn web_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    match remote.split_once("://") {
        // ssh and git carry the same host/path a browser wants, just over
        // a transport it can't speak — https is the page behind them.
        // A port on those is the ssh/git daemon's, and means nothing to
        // a browser — unlike an http port, which is the address itself.
        Some(("ssh" | "git" | "git+ssh", rest)) => assemble("https", rest, false),
        Some(("https", rest)) => assemble("https", rest, true),
        // Self-hosted forges on a LAN are still served over http; the
        // scheme is the user's call, not ours to upgrade.
        Some(("http", rest)) => assemble("http", rest, true),
        // file://, and whatever else git grows.
        Some(_) => None,
        // scp-like shorthand: [user@]host:path, the form GitHub hands out.
        None => {
            let (host, path) = remote.split_once(':')?;
            let host = host.rsplit('@').next()?;
            // A leading `.` or `/` (or a bare Windows drive) is a path with
            // a colon in it, not a host.
            if host.len() < 2 || host.starts_with('.') || host.starts_with('/') {
                return None;
            }
            assemble("https", &format!("{host}/{path}"), false)
        }
    }
}

/// Build the page URL from the host/path half of a remote, dropping the
/// parts a browser has no use for: credentials, a transport port, the
/// trailing `.git` and any slashes around the path. `port_is_web` keeps
/// the port for schemes a browser speaks — `git.lan:3000` is where the
/// forge answers, while `:22` on an ssh remote is not.
fn assemble(scheme: &str, rest: &str, port_is_web: bool) -> Option<String> {
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => return None,
    };
    // Credentials belong to the transport — never to the address bar.
    let host = authority.rsplit('@').next()?;
    // `ssh://git@host:22/o/r` — strip the port, but not an IPv6 literal
    // or anything else that isn't a plain number.
    let host = match host.rsplit_once(':') {
        Some((h, port))
            if !port_is_web && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            h
        }
        _ => host,
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::web_url;

    #[test]
    fn scp_shorthand_becomes_https() {
        assert_eq!(
            web_url("git@github.com:AgentSystemLabs/nebula.git").as_deref(),
            Some("https://github.com/AgentSystemLabs/nebula")
        );
    }

    #[test]
    fn ssh_and_git_urls_become_https() {
        for remote in [
            "ssh://git@github.com/o/r.git",
            "ssh://git@github.com:22/o/r.git",
            "git://github.com/o/r.git",
            "git+ssh://git@github.com/o/r",
        ] {
            assert_eq!(
                web_url(remote).as_deref(),
                Some("https://github.com/o/r"),
                "{remote}"
            );
        }
    }

    #[test]
    fn https_remotes_lose_the_git_suffix_and_stray_slashes() {
        assert_eq!(
            web_url("https://github.com/o/r.git").as_deref(),
            Some("https://github.com/o/r")
        );
        assert_eq!(
            web_url("  https://github.com/o/r/  ").as_deref(),
            Some("https://github.com/o/r")
        );
    }

    /// The whole reason userinfo is stripped: a cloned-with-token remote
    /// must not reach the browser's address bar or its history.
    #[test]
    fn credentials_never_reach_the_browser() {
        assert_eq!(
            web_url("https://someone:ghp_secrettoken@github.com/o/r.git").as_deref(),
            Some("https://github.com/o/r")
        );
        assert_eq!(
            web_url("https://token@gitlab.com/group/sub/proj.git").as_deref(),
            Some("https://gitlab.com/group/sub/proj")
        );
    }

    #[test]
    fn other_forges_work_the_same_way() {
        assert_eq!(
            web_url("git@gitlab.com:group/sub/proj.git").as_deref(),
            Some("https://gitlab.com/group/sub/proj")
        );
        assert_eq!(
            web_url("http://git.lan:3000/team/repo.git").as_deref(),
            Some("http://git.lan:3000/team/repo")
        );
    }

    #[test]
    fn local_clones_have_no_page() {
        for remote in [
            "/srv/git/repo.git",
            "../sibling",
            "./repo",
            "file:///srv/git/repo.git",
            "C:/repos/thing",
            "",
        ] {
            assert_eq!(web_url(remote), None, "{remote}");
        }
    }

    #[test]
    fn a_host_with_no_path_has_no_page() {
        assert_eq!(web_url("https://github.com"), None);
        assert_eq!(web_url("git@github.com:"), None);
    }
}
