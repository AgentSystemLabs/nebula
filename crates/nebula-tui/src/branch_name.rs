//! Branch names for the new-worktree prompt: slugifying what the user
//! typed, and inventing a name when they typed nothing.
//!
//! Git refuses spaces in a ref, but "fix login redirect" is how a branch
//! gets described out loud — so the prompt takes the sentence and hands
//! git `fix-login-redirect`. Enter on an empty prompt is the other half:
//! a throwaway `<adj>-<noun>-<verb>` name for the worktrees that only
//! ever needed *a* name, not the right one.

use std::sync::atomic::{AtomicU64, Ordering};

/// Whitespace runs become single hyphens; leading/trailing hyphens and
/// whitespace are trimmed. Everything else is left alone — a branch name
/// is the user's to spell, and git says its own piece about the rest.
pub fn slugify(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .trim_matches('-')
        .to_string()
}

const ADJECTIVES: &[&str] = &[
    "amber",
    "brave",
    "brisk",
    "calm",
    "clever",
    "cosmic",
    "crimson",
    "curious",
    "dapper",
    "eager",
    "electric",
    "fuzzy",
    "gentle",
    "golden",
    "hidden",
    "humble",
    "indigo",
    "jolly",
    "lucky",
    "mellow",
    "nimble",
    "polar",
    "quiet",
    "rapid",
    "rustic",
    "silent",
    "solar",
    "sunny",
    "teal",
    "velvet",
    "wandering",
    "yellow",
];

const NOUNS: &[&str] = &[
    "badger", "beacon", "cactus", "comet", "otter", "falcon", "fox", "gadget", "harbor", "heron",
    "island", "jaguar", "kestrel", "lantern", "lemur", "marble", "meadow", "narwhal", "orbit",
    "panda", "pebble", "quasar", "raven", "river", "sparrow", "tiger", "turtle", "walrus",
    "willow", "wombat", "yak", "zebra",
];

const VERBS: &[&str] = &[
    "banks", "bounds", "climbs", "coasts", "dances", "dashes", "dives", "drifts", "escapes",
    "floats", "flies", "gallops", "glides", "hops", "hums", "jumps", "leaps", "lingers", "paddles",
    "prowls", "races", "rambles", "roams", "sails", "scampers", "settles", "skips", "soars",
    "sprints", "strolls", "wanders", "waltzes",
];

/// `<adj>-<noun>-<verb>` from `seed` — 32³ ≈ 33k combinations.
pub fn name_from_seed(seed: u64) -> String {
    // Three independent slices of a scrambled seed, so neighbouring seeds
    // (two prompts opened in the same millisecond) don't share a word.
    let s = splitmix64(seed);
    let adj = ADJECTIVES[(s % ADJECTIVES.len() as u64) as usize];
    let noun = NOUNS[((s >> 21) % NOUNS.len() as u64) as usize];
    let verb = VERBS[((s >> 42) % VERBS.len() as u64) as usize];
    format!("{adj}-{noun}-{verb}")
}

/// A random name no branch in `taken` is already using. Falls back to
/// suffixing after enough collisions, so this always terminates.
pub fn random_name(taken: &[String]) -> String {
    for _ in 0..64 {
        let candidate = name_from_seed(next_seed());
        if !taken.iter().any(|t| t == &candidate) {
            return candidate;
        }
    }
    let base = name_from_seed(next_seed());
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.iter().any(|t| t == c))
        .expect("infinite range yields an untaken name")
}

/// Clock nanos mixed with a per-process counter: two names minted inside
/// the same clock tick still differ.
fn next_seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ splitmix64(COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_become_hyphens() {
        assert_eq!(slugify("fix login redirect"), "fix-login-redirect");
        assert_eq!(slugify("  padded  out  "), "padded-out");
        assert_eq!(slugify("tabs\tand\nnewlines"), "tabs-and-newlines");
    }

    /// Only whitespace is rewritten: slashes stay (git namespaces branches
    /// with them) and an already-hyphenated name round-trips.
    #[test]
    fn non_space_text_is_left_alone() {
        assert_eq!(slugify("feat/login"), "feat/login");
        assert_eq!(slugify("already-hyphenated"), "already-hyphenated");
        assert_eq!(slugify("feat/two words"), "feat/two-words");
    }

    /// Whitespace-only input is empty after slugifying — the caller reads
    /// that as "no name typed" and reaches for a random one.
    #[test]
    fn whitespace_only_slugifies_to_empty() {
        assert_eq!(slugify("   "), "");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn random_names_are_three_hyphenated_words() {
        for seed in 0..500u64 {
            let name = name_from_seed(seed);
            let words: Vec<&str> = name.split('-').collect();
            assert_eq!(words.len(), 3, "not three words: {name}");
            assert!(ADJECTIVES.contains(&words[0]), "{name}");
            assert!(NOUNS.contains(&words[1]), "{name}");
            assert!(VERBS.contains(&words[2]), "{name}");
            assert_eq!(slugify(&name), name, "not already a slug: {name}");
        }
    }

    /// The generator has to actually vary — a fixed word in any position
    /// would quietly collapse the name space.
    #[test]
    fn all_three_positions_vary() {
        let names: Vec<Vec<String>> = (0..500u64)
            .map(|s| name_from_seed(s).split('-').map(String::from).collect())
            .collect();
        for pos in 0..3 {
            let distinct: std::collections::HashSet<&String> =
                names.iter().map(|n| &n[pos]).collect();
            assert!(distinct.len() > 8, "position {pos} barely varies");
        }
    }

    #[test]
    fn random_name_avoids_taken_branches() {
        let taken: Vec<String> = (0..2000u64).map(name_from_seed).collect();
        let name = random_name(&taken);
        assert!(!taken.contains(&name), "handed back a taken name: {name}");
    }

    /// With every combination already a branch, the suffix fallback still
    /// hands back something usable instead of spinning.
    #[test]
    fn exhausted_name_space_falls_back_to_a_suffix() {
        let taken: Vec<String> = ADJECTIVES
            .iter()
            .flat_map(|a| {
                NOUNS
                    .iter()
                    .flat_map(move |n| VERBS.iter().map(move |v| format!("{a}-{n}-{v}")))
            })
            .collect();
        let name = random_name(&taken);
        assert!(name.ends_with("-2"), "not the suffix fallback: {name}");
        assert!(!taken.contains(&name));
    }

    #[test]
    fn successive_random_names_differ() {
        let a = random_name(&[]);
        let b = random_name(&[]);
        assert_ne!(a, b);
    }
}
