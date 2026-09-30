# Two plain shells on the grid whose cards read as small terminals: a /bin/sh that reads no profile
# and prompts with a coloured `demo:~$`, and two scripts in every checkout (git ignores them) that
# print a cargo test run and a coloured git log the way those tools colour them. The PREWARM POOL is
# off so `t` spawns the terminal the keys type into.
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false}
JSON
for checkout in "$DEMO" "$WORK"/demo-worktrees/*; do
  cat > "$checkout/cargo-test" <<'SH'
#!/bin/sh
printf '\033[1;32m   Compiling\033[0m nebula-tui v0.40.1\n'
printf '\033[1;32m    Finished\033[0m `test` profile in \033[33m10.31s\033[0m\n'
printf 'test result: \033[32mok\033[0m. \033[1m1208 passed\033[0m; \033[31m0 failed\033[0m; 0 ignored\n'
SH
  cat > "$checkout/git-log" <<'SH'
#!/bin/sh
printf '\033[33m9f2c1ab\033[0m (\033[1;36mHEAD -> \033[1;32mmain\033[0m) Terminal cards keep their colours\n'
printf '\033[33m331e836\033[0m (\033[1;31morigin/main\033[0m) d on an empty band deletes it\n'
printf '\033[33m0aa19b2\033[0m (\033[1;33mtag: v0.40.1\033[0m) Release v0.40.1\n'
SH
  chmod +x "$checkout/cargo-test" "$checkout/git-log"
done
mkdir -p "$DEMO/.git/info" && printf 'cargo-test\ngit-log\n' >> "$DEMO/.git/info/exclude"
cat > "$WORK/shell" <<'SH'
#!/bin/sh
# tmux starts the TUI through `$SHELL -c`; that one runs as asked.
for arg; do [ "$arg" = -c ] && exec /bin/sh "$@"; done
export PS1="$(printf '\033[1;32m')demo$(printf '\033[0m'):$(printf '\033[1;34m')~$(printf '\033[0m')\$ "
exec /bin/sh --noprofile --norc -i
SH
chmod +x "$WORK/shell"
export SHELL="$WORK/shell"
export SHOT_KEY_SECS=1
