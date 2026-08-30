---
paths:
  - "crates/**/*.rs"
---

# Keep modules small

A file, type or function that has grown long is a refactoring smell, not a fact of life. This repo
has a 20k-line `event_loop.rs` and 4k-line `ui.rs` / `registry.rs` precisely because every task
added a little more to the file it found; do not keep adding to the pile.

- **Split what you touch.** When the file, `impl` block, struct, enum or function you are editing is
  long — many screens, several unrelated concerns, a `match` with dozens of arms, a function that
  needs section comments to be read — extract the part you are working on (or the coherent piece
  next to it) into its own module, type or function with a name that says what it does. A `mod foo;`
  in a new file beside the old one is cheap; the next agent's grep finds `foo.rs` instead of a
  20k-line haystack.
- **Split when it makes sense, not by ruler.** There is no line limit. A long table or a long, flat
  test module is fine; a function that does three things, or a file whose name no longer describes
  its contents, is not. Prefer one module per concern (a panel, an overlay, a hook dialect, a
  subcommand) over one module per crate.
- **Behavior-preserving, tested first.** An extraction is a refactor: confirm a test covers the code
  (write one if not), run it green against the old shape, then move the code and run it again. Do
  not change behavior and layout in the same commit, and keep the public names callers use unless
  the task is to rename them.
- **Stay in your lane.** Extract from the file the task already has you in; do not launch drive-by
  refactors of files the task does not touch — the SHARED CHECKOUT has other sessions mid-edit, and
  a wholesale move of a file they are in is a merge conflict for everyone. A file that deserves a
  split but is out of scope is worth one line in the reply, not a change.
