# Space In A Fuzzy Query Is An AND, Not A Char — 2026-08-24

**Asked:** "I want the / fuzzy finder to be more fuzzy, like I should be able to type neb #10 and it
would have displayed the pr that had the #10 in it, right now it shows nothing if I type \"neb #10\""

**Did:** `crates/nebula-tui/src/fuzzy.rs::fuzzy_match` now splits the query on whitespace and requires
every term to subsequence-match the candidate *independently* — fzf's extended-search AND. The
best-of-starts greedy pass moved into a new `match_term(term, cand)`; `fuzzy_match` sums the term scores
and unions their positions. `rank`'s empty-query guard became
`query.split_whitespace().next().is_none()`. One matcher change covers all four call sites (`/` palette,
diff-view file filter, `f` file finder, `tree_browser.rs:300`). 8 unit tests in `fuzzy.rs` plus
`event_loop.rs::palette_query_terms_match_independently_across_a_space`; workspace suite 578 green.

**Gotchas:**
- The bug was never in the palette — it was one line of matcher semantics. `neb #10` against
  `nebula/#10 Credit…` failed because the greedy pass demanded a *literal space* between `neb` and
  `#10`, and the only space in that row sits after the `#10`. Nothing about the palette, the PR rows,
  or `TextInput` was wrong; `KeyCode::Char(' ')` reaches `palette.query` fine via the catch-all at
  `text_input.rs:183`.
- Terms match independently, so their spans can **overlap and arrive out of order** (`"ne neb"` yields
  `[0,1,2,0,1]`). `positions` feeds `ui.rs::fuzzy_highlight_spans`, which wants one ascending run —
  sort + dedup before returning or the highlight breaks.
- Whitespace-only queries needed their own guard in `rank`. `"  "` is not `is_empty()`, so it fell into
  the scoring path where every candidate scores 0 and the length tiebreak silently **re-sorted the whole
  list shortest-first** — a query that says nothing must not reorder anything.
- The three clippy warnings on `nebula-tui` (`ui.rs:2316`, `ui.rs:2446`, `config.rs:895`) are
  pre-existing, not from this change.
