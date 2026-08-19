//! Minimal fzf-style fuzzy matcher for the diff-view file filter.
//!
//! Greedy leftmost subsequence match, case-insensitive. Scoring favors
//! consecutive runs and matches that start a path segment or word, which is
//! enough to float `src/server.rs` above `crates/serde_helpers.rs` for the
//! query "srv" without pulling in a matcher crate.

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

/// Case-insensitive subsequence match of `query` inside `candidate`.
/// Returns None when some query char never appears. An empty query matches
/// everything with score 0 and no positions.
///
/// Runs one greedy pass from each occurrence of the first query char and
/// keeps the best score, so "serv" prefers the `server` filename over a
/// scattered s…e…r…v through the directory prefix.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let query: Vec<char> = query.chars().map(|c| c.to_ascii_lowercase()).collect();
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    let cand: Vec<char> = candidate.chars().collect();
    let mut best: Option<FuzzyMatch> = None;
    for start in 0..cand.len() {
        if cand[start].to_ascii_lowercase() != query[0] {
            continue;
        }
        // A failed greedy pass from here also fails from every later start
        // (its chars are a subset), so the first miss ends the search.
        let Some(m) = greedy_from(&query, &cand, start) else {
            break;
        };
        if best.as_ref().is_none_or(|b| m.score > b.score) {
            best = Some(m);
        }
    }
    best
}

/// One greedy leftmost pass over `cand[start..]`.
fn greedy_from(query: &[char], cand: &[char], start: usize) -> Option<FuzzyMatch> {
    let mut positions = Vec::with_capacity(query.len());
    let mut score = 0i32;
    let mut qi = 0;
    let mut prev_matched = false;
    for i in start..cand.len() {
        if cand[i].to_ascii_lowercase() == query[qi] {
            score += 1;
            if prev_matched {
                score += CONSECUTIVE_BONUS;
            }
            if is_boundary((i > 0).then(|| cand[i - 1])) {
                score += BOUNDARY_BONUS;
            }
            positions.push(i);
            prev_matched = true;
            qi += 1;
            if qi == query.len() {
                return Some(FuzzyMatch { score, positions });
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
    if query.is_empty() {
        return candidates
            .into_iter()
            .enumerate()
            .map(|(i, _)| (i, Vec::new()))
            .collect();
    }
    let mut scored: Vec<(i32, usize, usize, Vec<usize>)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, text)| {
            fuzzy_match(query, text).map(|m| (m.score, text.chars().count(), i, m.positions))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    scored.into_iter().map(|(_, _, i, p)| (i, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
