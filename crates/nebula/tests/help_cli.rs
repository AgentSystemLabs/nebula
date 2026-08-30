//! `--help` is a product surface, not a side effect of the parser. These tests
//! hold the three properties that make it usable: every command has a page of
//! its own, that page is about that command only, and nothing on it runs past
//! the terminal.
//!
//! Every invocation pins COLUMNS, because clap reads the real terminal size
//! first and falls back to the variable only when stdout is a pipe (as it is
//! here). Widths are counted in `chars()`, not bytes — the help is full of
//! em-dashes, which are three bytes each.

use std::process::Command;

/// Every command `nebula --help` lists, plus the `workspace` subcommands.
/// Hidden ones (`_raw-attach`, `_stale-daemon-note`) are deliberately absent.
const VISIBLE: &[&[&str]] = &[
    &["add"],
    &["daemon"],
    &["kill"],
    &["rename"],
    &["worktree"],
    &["spawn"],
    &["workspace"],
    &["workspace", "add"],
    &["workspace", "open"],
    &["workspace", "list"],
    &["workspace", "delete"],
    &["workspace", "rename"],
    &["browser"],
    &["ssh"],
    &["tunnel"],
    &["upgrade"],
];

fn help_at(columns: &str, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nebula"))
        .args(args)
        .env("COLUMNS", columns)
        .output()
        .unwrap_or_else(|e| panic!("failed to run nebula {}: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`nebula {}` exited {:?}: {}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn widest(help: &str) -> usize {
    help.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}

#[test]
fn every_visible_command_has_its_own_help_page() {
    for args in VISIBLE {
        let mut with_help = args.to_vec();
        with_help.push("--help");
        let help = help_at("100", &with_help);
        let usage = format!("Usage: nebula {}", args.join(" "));
        assert!(
            help.contains(&usage),
            "`nebula {} --help` is missing `{usage}`:\n{help}",
            args.join(" ")
        );
        assert!(
            help.contains("Example:\n") || help.contains("Examples:\n"),
            "`nebula {} --help` shows no example:\n{help}",
            args.join(" ")
        );
    }
}

/// The literal ask: `nebula browser --help` prints browser information and
/// nothing else. Cross-references by name (`nebula tunnel` runs the remote
/// half) are fine; another command's summary, or the root's command index,
/// is not.
#[test]
fn a_subcommand_page_is_about_that_subcommand_only() {
    let help = help_at("100", &["browser", "--help"]);
    assert!(
        !help.contains("Commands:"),
        "the root command index leaked into `browser --help`:\n{help}"
    );
    for summary in [
        "Terminal multiplexer for Claude Code agents",
        "Open nebula on a remote host over ssh",
        "Install the latest published nebula over this one",
        "Register a git checkout as a project",
    ] {
        assert!(
            !help.contains(summary),
            "`browser --help` carries another command's summary ({summary}):\n{help}"
        );
    }
}

/// The root is an index, so a command's entry has to be one line — that is
/// what the short first paragraph of each doc comment buys.
#[test]
fn the_root_help_lists_one_line_per_command() {
    let help = help_at("100", &["--help"]);
    let commands = help
        .split("Commands:\n")
        .nth(1)
        .expect("no command list")
        .split("\n\n")
        .next()
        .expect("empty command list");
    for line in commands.lines() {
        let (name, summary) = line
            .trim()
            .split_once("  ")
            .unwrap_or_else(|| panic!("no summary on `{line}`"));
        assert!(
            !summary.trim().is_empty(),
            "`{name}` has no summary in the root index"
        );
    }
    assert_eq!(
        commands.lines().count(),
        12,
        "eleven commands plus `help`:\n{commands}"
    );
}

/// Wrapping is a compile-time feature (`wrap_help`), and losing it is silent:
/// the width computation stays, `StyledStr::wrap` just becomes a no-op. The
/// tell is help that ignores the width it was given.
#[test]
fn help_wraps_to_the_width_it_is_given() {
    for args in VISIBLE {
        let mut with_help = args.to_vec();
        with_help.push("--help");
        let wide = widest(&help_at("100", &with_help));
        let narrow = widest(&help_at("60", &with_help));
        assert!(
            wide <= 100,
            "`nebula {} --help` runs {wide} columns wide at 100",
            args.join(" ")
        );
        assert!(
            narrow <= 60,
            "`nebula {} --help` runs {narrow} columns wide on a 60-column terminal",
            args.join(" ")
        );
    }
    // A page short enough to fit either width proves nothing, so one prose-heavy
    // page has to visibly reflow: that is the difference between wrapping and a
    // no-op `StyledStr::wrap`.
    let wide = widest(&help_at("100", &["tunnel", "--help"]));
    let narrow = widest(&help_at("60", &["tunnel", "--help"]));
    assert!(
        narrow < wide,
        "`nebula tunnel --help` did not reflow ({narrow} vs {wide}) \
         — has clap's `wrap_help` feature been dropped?"
    );
}

#[test]
fn hidden_commands_stay_hidden() {
    let mut pages = vec![help_at("100", &["--help"]), help_at("100", &["-h"])];
    for args in VISIBLE {
        let mut with_help = args.to_vec();
        with_help.push("--help");
        pages.push(help_at("100", &with_help));
    }
    for page in pages {
        for hidden in ["_raw-attach", "_stale-daemon-note"] {
            assert!(!page.contains(hidden), "{hidden} is listed in:\n{page}");
        }
    }
}

/// `-h` and `--help` are two audiences: a reminder and a manual. They stay
/// distinct only while every doc comment keeps its short first paragraph.
#[test]
fn short_help_is_terser_than_long_help() {
    for args in [&["browser"], &["worktree"], &["tunnel"]] {
        let short = help_at("100", &[args[0], "-h"]);
        let long = help_at("100", &[args[0], "--help"]);
        assert!(
            short.lines().count() < long.lines().count(),
            "`nebula {} -h` is no shorter than its --help",
            args[0]
        );
    }
}
