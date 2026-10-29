//! Codex 0.165 confirms `/model` (including effort-only changes) with
//! `• Model changed to <model> <effort> for this session only`.
//! It fires no hook and need not have a transcript yet. Read the rendered
//! confirmation, never the picker highlight or the user's input. A screen
//! handles ANSI, incremental repaints, wrapping and split UTF-8 correctly.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub model: String,
    pub effort: Option<String>,
}

pub struct Scanner {
    parser: vt100::Parser,
    selection: Option<Selection>,
}

impl Scanner {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(2), cols.max(2), 0),
            selection: None,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(rows.max(2), cols.max(2));
    }

    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Option<Selection> {
        self.parser.process(bytes);
        let text = self.parser.screen().contents();
        let mut latest = None;
        // Ratatui wraps informational cells with two-space indentation.
        // Only a complete confirmation counts; a split repaint can leave
        // just a prefix visible until the next chunk arrives.
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let Some(rest) = line.trim_start().strip_prefix("• Model changed to ") else {
                continue;
            };
            let mut message = rest.to_string();
            while !message.contains(" for this session only") {
                let Some(next) = lines.peek().filter(|line| line.starts_with("  ")) else {
                    break;
                };
                message.push(' ');
                message.push_str(next.trim());
                lines.next();
            }
            if let Some(selection) = parse(&message) {
                latest = Some(selection);
            }
        }
        let seen = latest?;
        if self.selection.as_ref() == Some(&seen) {
            return None;
        }
        self.selection = Some(seen.clone());
        Some(seen)
    }
}

fn parse(message: &str) -> Option<Selection> {
    let value = message.strip_suffix(" for this session only")?;
    let mut parts = value.split_whitespace();
    let model = parts.next()?.to_string();
    let effort = match parts.next() {
        Some("default" | "fast" | "standard") | None => None,
        Some(
            effort @ ("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"),
        ) => Some(effort.to_string()),
        _ => return None,
    };
    Some(Selection { model, effort })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_selection_survives_every_byte_boundary() {
        let bytes =
            "\x1b[2J\x1b[H• Model changed to \x1b[1mgpt-5.5\x1b[0m xhigh for this session only\r\n"
                .as_bytes();
        for at in 0..=bytes.len() {
            let mut scanner = Scanner::new(120, 24);
            let first = scanner.feed(&bytes[..at]);
            let seen = scanner.feed(&bytes[at..]).or(first).unwrap();
            assert_eq!(seen.model, "gpt-5.5");
            assert_eq!(seen.effort.as_deref(), Some("xhigh"));
            assert!(scanner.feed(b"\r\n").is_none());
        }
    }

    #[test]
    fn wrapped_confirmation_and_effort_only_change() {
        let mut scanner = Scanner::new(50, 24);
        assert!(scanner
            .feed("• Model changed to gpt-5.5 high\r\n  for this session only\r\n".as_bytes())
            .is_some());
        let seen = scanner
            .feed("• Model changed to gpt-5.5 low\r\n  for this session only\r\n".as_bytes())
            .unwrap();
        assert_eq!(seen.effort.as_deref(), Some("low"));
        let seen = scanner
            .feed(
                "• Model changed to codex-auto-test fast\r\n  for this session only\r\n".as_bytes(),
            )
            .unwrap();
        assert_eq!(seen.model, "codex-auto-test");
        assert_eq!(seen.effort, None);
    }

    #[test]
    fn ignores_picker_input_and_incomplete_confirmation() {
        let mut scanner = Scanner::new(120, 24);
        for text in [
            "› gpt-5.5 high\r\n",
            "› Model changed to gpt-5.5 high for this session only\r\n",
            "• Model changed to gpt-5.5 high",
        ] {
            assert!(scanner.feed(text.as_bytes()).is_none());
        }
        assert!(scanner.feed(b" for this session only\r\n").is_some());
    }
}
