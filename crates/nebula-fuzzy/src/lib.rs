//! Minimal fzf-style fuzzy matcher for nebula's list filters (re-exported as
//! `nebula_tui::fuzzy`). A crate of its own only so a dev build can compile
//! it at opt-level 3 — see the workspace manifest.
//!
//! Greedy leftmost subsequence match, case-insensitive. Scoring favors
//! consecutive runs and matches that start a path segment or word, which is
//! enough to float `src/server.rs` above `crates/serde_helpers.rs` for the
//! query "srv" without pulling in a matcher crate.
//!
//! Whitespace in a query splits it into independent terms, all of which must
//! match somewhere in the candidate, in any order (fzf's extended-search AND).
//! That is what lets `neb #10` find `nebula/#10 Credit Codex…` — a single
//! subsequence pass would demand a literal space between `neb` and `#10`.

/// A successful match: the score (higher is better) and the ascending char
/// indices of `candidate` that matched, for highlighting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub positions: Vec<usize>,
}

const CONSECUTIVE_BONUS: i32 = 8;
const BOUNDARY_BONUS: i32 = 6;

/// Chars that start a new "word" in a path for the boundary bonus.
fn is_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => matches!(c, '/' | '\\' | '_' | '-' | '.' | ' '),
    }
}

/// Case-insensitive match of `query` inside `candidate`.
///
/// The query is split on whitespace; every term must match `candidate` as a
/// subsequence, but the terms are matched independently and may appear in any
/// order. Returns None when some term never matches. An empty (or all
/// whitespace) query matches everything with score 0 and no positions.
///
/// Each term runs one greedy pass from each occurrence of its first char and
/// keeps the best score, so "serv" prefers the `server` filename over a
/// scattered s…e…r…v through the directory prefix.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    Matcher::new(query).matches(candidate)
}

/// One query, prepared once, matched against many candidates: what a list
/// filter does on every keystroke. The FILE FINDER ranks every path of the
/// checkout per character typed — ten thousand of them, on a large one —
/// and matching each from scratch lower-cased the query, collected the
/// candidate's chars into a fresh buffer and allocated a position list per
/// starting point tried: 30 ms a keystroke in the build `make dev` runs,
/// felt as a finder that lags the typing. Here the terms are lower-cased
/// once, the candidate's chars go into one buffer kept across calls, and
/// starts are compared by score alone — positions are collected once, for
/// the start that won.
pub struct Matcher {
    /// The query's whitespace-separated terms, lower-cased.
    terms: Vec<Vec<char>>,
    /// The candidate in hand, lower-cased; reused between candidates.
    cand: Vec<char>,
}

impl Matcher {
    pub fn new(query: &str) -> Self {
        Self {
            terms: query
                .split_whitespace()
                .map(|term| term.chars().map(|c| c.to_ascii_lowercase()).collect())
                .collect(),
            cand: Vec::new(),
        }
    }

    /// [`fuzzy_match`] of this matcher's query against `candidate`.
    pub fn matches(&mut self, candidate: &str) -> Option<FuzzyMatch> {
        // Most candidates match nothing — a filter is typed to narrow a
        // list — so say so before filling the buffer: one walk of the
        // candidate per term, nothing allocated.
        if !self
            .terms
            .iter()
            .all(|term| has_subsequence(term, candidate))
        {
            return None;
        }
        self.cand.clear();
        self.cand
            .extend(candidate.chars().map(|c| c.to_ascii_lowercase()));
        let mut score = 0i32;
        let mut positions: Vec<usize> = Vec::new();
        for term in &self.terms {
            let m = match_term(term, &self.cand)?;
            score += m.score;
            positions.extend(m.positions);
        }
        // Terms match independently, so their spans can overlap and arrive
        // out of order; highlighting wants one ascending, deduplicated run.
        if self.terms.len() > 1 {
            positions.sort_unstable();
            positions.dedup();
        }
        Some(FuzzyMatch { score, positions })
    }
}

/// Do `term`'s chars (lower-cased) appear in `candidate` in order,
/// case-insensitively? The necessary condition for [`match_term`] to find
/// anything, without its buffers.
fn has_subsequence(term: &[char], candidate: &str) -> bool {
    // An ASCII term — nearly every one — can be looked for in the bytes:
    // no byte of a multi-byte char equals an ASCII one, so it is the same
    // question. A plain indexed loop, because this is the inner loop of
    // every list filter and the build `make dev` runs does not optimise
    // this crate: the iterator form below was 20 ms over ten thousand
    // paths there, this is 3.
    if term.iter().all(char::is_ascii) {
        let bytes = candidate.as_bytes();
        let mut want = 0;
        let mut i = 0;
        while want < term.len() && i < bytes.len() {
            if bytes[i].to_ascii_lowercase() == term[want] as u8 {
                want += 1;
            }
            i += 1;
        }
        return want == term.len();
    }
    let mut wanted = term.iter().peekable();
    for c in candidate.chars() {
        match wanted.peek() {
            None => return true,
            Some(w) if **w == c.to_ascii_lowercase() => {
                wanted.next();
            }
            Some(_) => {}
        }
    }
    wanted.peek().is_none()
}

/// Best subsequence match of one whitespace-free `term` anywhere in `cand`
/// (both already lower-cased): the highest-scoring greedy pass, the
/// leftmost of equals.
fn match_term(term: &[char], cand: &[char]) -> Option<FuzzyMatch> {
    if term.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    let mut best: Option<(i32, usize)> = None;
    for start in 0..cand.len() {
        if cand[start] != term[0] {
            continue;
        }
        // A failed greedy pass from here also fails from every later start
        // (its chars are a subset), so the first miss ends the search.
        let Some(score) = greedy_from(term, cand, start, None) else {
            break;
        };
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, start));
        }
    }
    let (score, start) = best?;
    let mut positions = Vec::with_capacity(term.len());
    greedy_from(term, cand, start, Some(&mut positions));
    Some(FuzzyMatch { score, positions })
}

/// One greedy leftmost pass over `cand[start..]`: its score, and — when
/// asked — the positions it matched.
fn greedy_from(
    query: &[char],
    cand: &[char],
    start: usize,
    mut positions: Option<&mut Vec<usize>>,
) -> Option<i32> {
    let mut score = 0i32;
    let mut qi = 0;
    let mut prev_matched = false;
    for i in start..cand.len() {
        if cand[i] == query[qi] {
            score += 1;
            if prev_matched {
                score += CONSECUTIVE_BONUS;
            }
            if is_boundary((i > 0).then(|| cand[i - 1])) {
                score += BOUNDARY_BONUS;
            }
            if let Some(positions) = positions.as_deref_mut() {
                positions.push(i);
            }
            prev_matched = true;
            qi += 1;
            if qi == query.len() {
                return Some(score);
            }
        } else {
            prev_matched = false;
        }
    }
    None
}

/// Rank `candidates` against `query`: matching indices best-first, each with
/// its matched char positions. Score-sorted, ties broken by shorter text
/// then original order; an empty query keeps every candidate in original
/// order with no positions.
pub fn rank<'a, I>(query: &str, candidates: I) -> Vec<(usize, Vec<usize>)>
where
    I: IntoIterator<Item = &'a str>,
{
    // Whitespace-only counts as empty: every candidate scores 0, and sorting
    // that by length would shuffle the list for a query that says nothing.
    if query.split_whitespace().next().is_none() {
        return candidates
            .into_iter()
            .enumerate()
            .map(|(i, _)| (i, Vec::new()))
            .collect();
    }
    rank_by(query, candidates, |i, text| (text.chars().count(), i))
}

/// [`rank`] with the caller's own tiebreak: equal scores sort by ascending
/// `key(index, text)`, and an empty (or all-whitespace) query lists every
/// candidate in key order with no positions. For a list that has an order
/// of its own — the `/` PALETTE's attention order — the key keeps that
/// order wherever the score has nothing to say.
pub fn rank_by<'a, I, K>(
    query: &str,
    candidates: I,
    key: impl Fn(usize, &str) -> K,
) -> Vec<(usize, Vec<usize>)>
where
    I: IntoIterator<Item = &'a str>,
    K: Ord,
{
    if query.split_whitespace().next().is_none() {
        let mut all: Vec<(K, usize)> = candidates
            .into_iter()
            .enumerate()
            .map(|(i, text)| (key(i, text), i))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        return all.into_iter().map(|(_, i)| (i, Vec::new())).collect();
    }
    let mut matcher = Matcher::new(query);
    let mut scored: Vec<(i32, K, usize, Vec<usize>)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, text)| {
            matcher
                .matches(text)
                .map(|m| (m.score, key(i, text), i, m.positions))
        })
        .collect();
    // Stable, so original order is the final fallback under an equal key.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, i, p)| (i, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reject pass must never disagree with the scorer it guards: a
    /// candidate it turns away is one the scorer would have, and the other
    /// way round.
    #[test]
    fn the_reject_pass_agrees_with_the_scorer() {
        let candidates = [
            "src/server.rs",
            "crates/serde_helpers.rs",
            "README.md",
            "Ünïcode/Päth.RS",
            "a",
            "",
            "nebula/#10 Credit Codex",
        ];
        let terms = [
            "srv", "SRV", "rs", "md", "x", "a", "#10", "päth", "zzz", "serverr",
        ];
        for candidate in candidates {
            let cand: Vec<char> = candidate.chars().map(|c| c.to_ascii_lowercase()).collect();
            for term in terms {
                let lowered: Vec<char> = term.chars().map(|c| c.to_ascii_lowercase()).collect();
                assert_eq!(
                    has_subsequence(&lowered, candidate),
                    match_term(&lowered, &cand).is_some(),
                    "{term:?} in {candidate:?}"
                );
            }
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let m = fuzzy_match("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
    }

    #[test]
    fn subsequence_matches_and_reports_positions() {
        // Ties keep the leftmost start ("src…" here scores the same as the
        // start at "server").
        let m = fuzzy_match("srv", "src/server.rs").unwrap();
        assert_eq!(m.positions, vec![0, 1, 7]);
    }

    #[test]
    fn best_start_prefers_the_filename_run() {
        // Greedy from the leftmost 's' would scatter across "src/"; the
        // best-of-starts pass lands on the consecutive "serv" in "server".
        let m = fuzzy_match("serv", "src/server.rs").unwrap();
        assert_eq!(m.positions, vec![4, 5, 6, 7]);
    }

    #[test]
    fn missing_char_fails() {
        assert!(fuzzy_match("xyz", "src/server.rs").is_none());
        assert!(fuzzy_match("abc", "ab").is_none());
    }

    #[test]
    fn match_is_case_insensitive() {
        assert!(fuzzy_match("READ", "readme.md").is_some());
        assert!(fuzzy_match("read", "README.md").is_some());
    }

    #[test]
    fn consecutive_run_beats_scattered_match() {
        let run = fuzzy_match("serv", "src/server.rs").unwrap();
        let scattered = fuzzy_match("serv", "s_e_r_v.rs").unwrap();
        assert!(run.score > scattered.score, "{run:?} vs {scattered:?}");
    }

    #[test]
    fn segment_start_beats_mid_word() {
        let boundary = fuzzy_match("ui", "src/ui.rs").unwrap();
        let mid = fuzzy_match("ui", "build.rs").unwrap();
        assert!(boundary.score > mid.score, "{boundary:?} vs {mid:?}");
    }

    #[test]
    fn space_separated_terms_match_independently() {
        // The reported case: one subsequence pass wants a literal space
        // between "neb" and "#10", which the PR row does not have.
        let m = fuzzy_match(
            "neb #10",
            "nebula/#10 Credit Codex and Cursor in the README",
        )
        .unwrap();
        assert_eq!(m.positions, vec![0, 1, 2, 7, 8, 9]);
    }

    #[test]
    fn terms_may_appear_in_any_order() {
        assert!(fuzzy_match("#10 neb", "nebula/#10 Credit Codex").is_some());
        assert!(fuzzy_match("requests show", "nebula/main/Show Open Pull Requests").is_some());
    }

    #[test]
    fn every_term_must_match() {
        assert!(fuzzy_match("neb #11", "nebula/#10 Credit Codex").is_none());
        assert!(fuzzy_match("neb zzz", "nebula/#10 Credit Codex").is_none());
    }

    #[test]
    fn positions_are_ascending_and_deduped_across_overlapping_terms() {
        // "ne" and "neb" both land on the same leading chars.
        let m = fuzzy_match("ne neb", "nebula/main").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
    }

    #[test]
    fn whitespace_only_query_matches_everything_in_order() {
        let m = fuzzy_match("   ", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
        let ranked = rank("  ", vec!["a-longer-one", "ab"]);
        assert_eq!(ranked, vec![(0, vec![]), (1, vec![])]);
    }

    #[test]
    fn trailing_space_behaves_like_the_bare_term() {
        assert_eq!(
            fuzzy_match("serv ", "src/server.rs"),
            fuzzy_match("serv", "src/server.rs")
        );
    }

    #[test]
    fn multi_term_ranking_floats_the_row_that_matches_both() {
        let rows = vec![
            "nebula/main/Show Open Pull Requests",
            "nebula/worktree-readme-tweak/Readme Tweak Pull Request",
            "nebula/#10 Credit Codex and Cursor in the README tagline",
        ];
        let ranked = rank("neb #10", rows.clone());
        assert_eq!(ranked.len(), 1, "only the #10 row has both terms");
        assert_eq!(ranked[0].0, 2);
    }

    #[test]
    fn rank_by_lists_an_empty_query_in_key_order_and_breaks_ties_by_key() {
        // Empty query: pure key order, no positions.
        let ranked = rank_by("", vec!["b", "a", "c"], |i, _| [2usize, 0, 1][i]);
        assert_eq!(
            ranked,
            vec![(1, vec![]), (2, vec![]), (0, vec![])],
            "key order, not original order"
        );
        // Equal scores: the key decides, not the text length.
        let ranked = rank_by("main", vec!["demo/main", "demo/main/agent-1"], |i, _| {
            [1usize, 0][i]
        });
        assert_eq!(ranked[0].0, 1, "the longer row wins on key");
        // A better score still beats a better key.
        let ranked = rank_by("read", vec!["feat/unread", "feat/read"], |i, _| i);
        assert_eq!(ranked[0].0, 1, "the boundary match outranks the key");
    }
}
