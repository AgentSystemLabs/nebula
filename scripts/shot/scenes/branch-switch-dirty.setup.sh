# Same branches as branch-switch, with the root checkout dirty: two edits and an untracked file.
. "$HERE/scenes/branch-switch.setup.sh"
printf '# demo\n\nA project for the screenshots, now with a changelog.\n' > "$DEMO/README.md"
printf 'pub fn run() { todo!() }\n' > "$DEMO/src/lib.rs"
printf 'ship the switcher\n' > "$DEMO/TODO.md"
