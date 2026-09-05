---

## 💬 ==== YOU ASKED ====

"Move the FOOTER drawing out of the UI module into its own module, KEEP MODULES SMALL style — no
behavior change, the same tests moved with it, nothing else touched."

---

## 📋 ==== OVERVIEW ====

The FOOTER — VERSION NAMEPLATE, key hints, FLASH, the tallies — now draws from its own module, and the
UI module lost exactly that block. Moved, not rewritten: the same functions, the same tests, the same
E2E TUI frames. nebula-tui 570 passed, the count unchanged; clippy clean. The SESSIONS PANEL drawing
is the next candidate if you want the same treatment.

---

## 👉 ==== NEXT STEPS ====

1. Good to commit — `git add crates/nebula-tui/src/ui.rs crates/nebula-tui/src/footer.rs crates/nebula-tui/src/lib.rs && git commit`.
