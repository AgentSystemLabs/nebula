# In-TUI File Tooling — 2026-08-19

**Asked:** Four asks in one evening: "when a user presses f show a fuzzy file finder…", "add the ability
for a user to press a hotkey to show a find in files search, basically it should run grep over the code
base… when a user presses enter it should show a vim terminal to allow editing that file, that vim
terminal must be a modal inside this app", "when claude code prints file paths, I want to be able to do a
option click… to actually open that file directly inside a file viewer (vim) inside nebula", and "add a
hotkey for t which shows a full tree browser modal with a view of the file content on the right…" →
refined to "in the file preview, it should be syntax highlighted, also when I select the file, it
shouldn't open a new vim modal, the right panel should just focus and let editing with vim."

**Did:** `998901f` (file finder, grep overlay, path links, in-TUI editor via
`crates/nebula-tui/src/vim_term.rs`) and `7ebc264` (tree browser with live filter and syntax preview).
Later `6787999` numbered the lines in file previews but not directory listings. The editor command is
configurable — the user asked for neovim support explicitly.
