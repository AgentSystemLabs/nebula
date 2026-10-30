//! Canonical GitHub PR links, shared by hook capture and explicit PR lookup.

/// Read a GitHub PR URL at the start of `text`, dropping Markdown punctuation,
/// query strings and tabs such as `/files`. Never accept another host or an
/// option that could become a `gh` argument.
pub fn github_pr_url(text: &str) -> Option<String> {
    let rest = text.strip_prefix("https://github.com/")?;
    let mut parts = rest.splitn(4, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let kind = parts.next()?;
    let tail = parts.next()?;
    let component = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if !component(owner) || !component(repo) || kind != "pull" {
        return None;
    }
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let number = digits.parse::<u64>().ok().filter(|n| *n > 0)?;
    if tail[digits.len()..]
        .chars()
        .next()
        .is_some_and(|c| !"/?#)]>.,\"'".contains(c) && !c.is_whitespace())
    {
        return None;
    }
    Some(format!("https://github.com/{owner}/{repo}/pull/{number}"))
}

/// A completed assistant answer can identify its PR even when the checkout
/// belongs to another repository. Ambiguous answers deliberately bind nothing.
pub fn reported_pr_url(message: &str) -> Option<String> {
    let urls: std::collections::BTreeSet<_> = message
        .match_indices("https://github.com/")
        .filter_map(|(at, _)| github_pr_url(&message[at..]))
        .collect();
    (urls.len() == 1).then(|| urls.into_iter().next().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_the_actual_cross_repo_codex_completion() {
        assert_eq!(reported_pr_url("Opened [draft PR #384](https://github.com/webdevcody-aipulsedaily/survive-the-night-fps/pull/384) into `map3-epic`."), Some("https://github.com/webdevcody-aipulsedaily/survive-the-night-fps/pull/384".into()));
        assert_eq!(
            reported_pr_url(
                "Merged https://github.com/o/r/pull/7; see https://github.com/o/r/pull/7/files"
            ),
            Some("https://github.com/o/r/pull/7".into())
        );
    }

    #[test]
    fn ignores_ambiguous_or_untrusted_links() {
        for text in [
            "https://github.com/o/r/issues/7",
            "https://github.com.evil/o/r/pull/7",
            "https://github.com/o/r/pull/0",
            "https://github.com/o/r/pull/7evil",
            "https://github.com/o/r/pull/7 https://github.com/o/r/pull/8",
            "https://github.com/../r/pull/7",
            "no PR",
        ] {
            assert_eq!(reported_pr_url(text), None, "{text}");
        }
    }
}
