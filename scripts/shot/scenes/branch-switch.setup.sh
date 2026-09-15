# Branches for the BRANCH SWITCHER to list: local ones with commits of their own, dated apart so the
# ages read, and a bare origin carrying two branches nothing local has, so remote rows sit beside them.
# The stock feature-x / wheel-one-line worktrees hold their branches, which the switcher refuses.
g() { git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid "$@"; }
hours_ago() { echo "$(( $(date +%s) - $1 * 3600 )) +0000"; }
commit() { GIT_AUTHOR_DATE="$(hours_ago "$1")" GIT_COMMITTER_DATE="$(hours_ago "$1")" g commit -q -m "$2"; }
mkdir -p "$DEMO/src"
printf '# demo\n\nA project for the screenshots.\n' > "$DEMO/README.md"
printf 'pub fn run() {}\n' > "$DEMO/src/lib.rs"
g add -A && commit 2 "Scaffold the demo crate"
branch() {
  g switch -q -c "$1" main
  printf '%s\n' "$3" >> "$DEMO/CHANGELOG.md"
  g add -A && commit "$2" "$3"
  g switch -q main
}
branch fix-login-redirect 5 "Keep the redirect target through the OAuth bounce"
branch perf-replay-parse 20 "Parse the scrollback replay in one pass"
branch release-0.27 30 "Release v0.27.0"
branch docs-hotkeys-table 100 "Document every hotkey in one table"
branch theme-solarized 200 "Solarized light and dark themes"
branch spike-sqlite-wal 500 "Try WAL mode for the store"
git init -q --bare "$WORK/origin.git"
g remote add origin "$WORK/origin.git"
g push -q origin main fix-login-redirect perf-replay-parse theme-solarized
g branch -q -D perf-replay-parse theme-solarized
g fetch -q origin
