//! POSIX shell helpers the daemon, the TUI and the CLI share.

/// Single-quote `arg` for a POSIX shell: `it's` → `'it'\''s'`. Inside
/// single quotes the only character needing an escape is `'` itself.
pub fn single_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// The user's shell: `$SHELL`, else `/bin/sh`.
pub fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_quote_edge_cases() {
        assert_eq!(single_quote(""), "''");
        assert_eq!(single_quote("plain"), "'plain'");
        assert_eq!(single_quote("'''"), "''\\'''\\'''\\'''");
    }
}
