# Removed The NOTES Feature Outright — 2026-08-26

**Asked:** "remove the ability to add notes" — asked twice, identically. The literal words name only the
*add* path, so I put the scope to the user before cutting: remove adding only (leaving a list that can
shrink but never grow) vs. remove the feature entirely. They chose **entirely**.

**Did:** Full-stack removal. `nebula-core`: `Note`/`NoteOwner` + `Entity::Note`/`EntityId::Note`
(`entities.rs`), `id_newtype!(NoteId)` (`ids.rs`), the four `ClientRequest::{Create,Update,Delete}Note` /
`SetNoteDone` variants and `Snapshot.notes` (`protocol.rs`), `PROTOCOL_VERSION` 26 → 27. `nebula-daemon`:
the `// ---- notes ----` blocks in `store.rs` (7 fns) and `registry.rs` (4 fns), the 4 `server.rs` arms,
plus **MIGRATION 21 `DROP TABLE IF EXISTS notes`**. `nebula-tui`: `Action::Notes` and its `e` binding
(`keymap.rs`), `NoteView`/`NoteInput`/`Overlay::Notes`/`PendingIntent::SelectCreatedNote`/`Tree.notes`
(`app.rs`), the modal draw + `note_badge` + both FOOTER hints (`ui.rs`), and in `event_loop.rs` the
`NoteCmd` key handler, the mouse handler, `open_note_view`/`open_notes_for_owner`/`select_note_by_id`,
both CONTEXT MENU rows, and the two delete-cascade `retain`s. Docs: README key table x2 + the SQLite
bullet, ARCHITECTURE.md's note-list paragraph. Tests: deleted `store::note_crud_roundtrip_and_cascade`,
the 3 `event_loop` note tests, and e2e `tui_note_modal_crud_and_badge`. 645 tests green, clippy clean
(7 pre-existing warning sites, none mine), `cargo fmt` applied.

**Gotchas:**
- **`row_badges` in `ui.rs` lost an argument.** It was `(unseen, notes, th)` feeding two badge makers;
  it is now `(unseen, th)`. `ProjectRowData`/`WorktreeRowData` each lost their `(usize, usize)` note-stats
  tuple slot, so the destructuring at both call sites had to shrink with them.
- **Don't cut a match arm by slicing to the *next* arm's name without checking arm order.**
  `Overlay::Metrics` sits **before** `Overlay::Notes` in `ui.rs`'s draw match, so slicing
  `[index(Notes) .. index(Metrics)]` had a negative span and silently **duplicated ~750 lines** instead of
  deleting any. The tell is the file getting *longer*: `wc -l` went 4467 → 5199 and
  `grep -c "Overlay::Metrics(view) => {"` returned 2. Find the arm's own closing brace instead.
- **Three keymap/settings tests used `Action::Notes` as an arbitrary subject**, not because they were
  about notes — they bind `g` to it to collide with `Git diff`. Swapped to `Action::OpenRepo`
  (`keymap.rs`) and `Action::Help` (`event_loop.rs`); Help was the right stand-in for
  `confirming_a_duplicate_moves_the_chord_off_its_old_action` because its final assertion needs an action
  that **opens an overlay**, which `OpenRepo` (spawns a browser) does not.
- **A stale `"keybindings": {"notes": "…"}` in a user's `config.json` is harmless** — `Keymap::from_overrides`
  already ignores unknown action ids, covered by `a_broken_override_falls_back_instead_of_stranding_the_user`.
- `e` is now **unbound**. `splash_footer_lists_only_keys_that_work` asserted `"e: notes"` was *absent*
  from the SPLASH; that string can never appear now, so it came out of the dead-key list.
- The SHARED CHECKOUT carried ~1700 lines of another session's uncommitted work (including a new
  `crates/nebula-daemon/src/pty/cloud.rs`). Baseline `cargo check` was green **before** starting, which is
  what made it safe to attribute every later error to my own edits — do that check first in the SHARED CHECKOUT.
