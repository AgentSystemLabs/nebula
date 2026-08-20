//! Minimal syntax highlighter for the tree-browser preview.
//!
//! A per-line tokenizer that recognizes comments, strings, numbers, and a
//! per-language keyword set — enough color to make code scannable without
//! pulling in a highlighter crate (the `fuzzy` precedent). Classification
//! lives here; styling itself lives in ui.rs (the `classify_diff_line`
//! rule). Block comments carry state across lines; multiline strings don't
//! (an unterminated string just colors to the end of its line).

/// Token classification for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Text,
}

/// How one language tokenizes.
struct LangSpec {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// String delimiter chars (backslash-escape aware).
    strings: &'static [char],
    keywords: &'static [&'static str],
    /// Keywords match regardless of case (SQL).
    case_insensitive: bool,
}

const RUST: LangSpec = LangSpec {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    // No '\'' — lifetimes ('a) would eat the rest of the line.
    strings: &['"'],
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
    case_insensitive: false,
};

const JS: LangSpec = LangSpec {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: &['"', '\'', '`'],
    keywords: &[
        "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
        "delete", "do", "else", "enum", "export", "extends", "false", "finally", "for",
        "function", "if", "implements", "import", "in", "instanceof", "interface", "let", "new",
        "null", "of", "private", "protected", "public", "readonly", "return", "static", "super",
        "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void",
        "while", "yield",
    ],
    case_insensitive: false,
};

const PYTHON: LangSpec = LangSpec {
    line_comments: &["#"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self",
        "True", "try", "while", "with", "yield",
    ],
    case_insensitive: false,
};

const GO: LangSpec = LangSpec {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: &['"', '`'],
    keywords: &[
        "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
        "false", "for", "func", "go", "goto", "if", "import", "interface", "map", "nil",
        "package", "range", "return", "select", "struct", "switch", "true", "type", "var",
    ],
    case_insensitive: false,
};

const C: LangSpec = LangSpec {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: &['"', '\''],
    keywords: &[
        "auto", "bool", "break", "case", "char", "class", "const", "continue", "default",
        "delete", "do", "double", "else", "enum", "extern", "false", "float", "for", "goto",
        "if", "inline", "int", "long", "namespace", "new", "nullptr", "private", "protected",
        "public", "return", "short", "signed", "sizeof", "static", "struct", "switch",
        "template", "true", "typedef", "typename", "union", "unsigned", "using", "virtual",
        "void", "volatile", "while",
    ],
    case_insensitive: false,
};

const JAVA: LangSpec = LangSpec {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: &['"', '\''],
    keywords: &[
        "abstract", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
        "continue", "default", "do", "double", "else", "enum", "extends", "false", "final",
        "finally", "float", "for", "fun", "if", "implements", "import", "instanceof", "int",
        "interface", "long", "native", "new", "null", "object", "override", "package",
        "private", "protected", "public", "record", "return", "short", "static", "super",
        "switch", "synchronized", "this", "throw", "throws", "true", "try", "val", "var",
        "void", "volatile", "when", "while",
    ],
    case_insensitive: false,
};

const RUBY: LangSpec = LangSpec {
    line_comments: &["#"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &[
        "and", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end", "ensure",
        "false", "for", "if", "in", "module", "next", "nil", "not", "or", "raise", "require",
        "rescue", "return", "self", "then", "true", "unless", "until", "when", "while", "yield",
    ],
    case_insensitive: false,
};

const SHELL: LangSpec = LangSpec {
    line_comments: &["#"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &[
        "alias", "case", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "fi",
        "for", "function", "if", "in", "local", "return", "set", "source", "then", "unset",
        "until", "while",
    ],
    case_insensitive: false,
};

const SQL: LangSpec = LangSpec {
    line_comments: &["--"],
    block_comment: Some(("/*", "*/")),
    strings: &['"', '\''],
    keywords: &[
        "all", "alter", "and", "as", "by", "constraint", "create", "default", "delete",
        "distinct", "drop", "exists", "foreign", "from", "group", "having", "in", "index",
        "inner", "insert", "into", "is", "join", "key", "left", "limit", "not", "null", "offset",
        "on", "or", "order", "outer", "primary", "references", "right", "select", "table",
        "union", "unique", "update", "values", "view", "where",
    ],
    case_insensitive: true,
};

const TOML: LangSpec = LangSpec {
    line_comments: &["#", ";"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &["true", "false"],
    case_insensitive: false,
};

const YAML: LangSpec = LangSpec {
    line_comments: &["#"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &["true", "false", "null"],
    case_insensitive: false,
};

const JSON: LangSpec = LangSpec {
    line_comments: &["//"], // jsonc; plain JSON never contains one
    block_comment: None,
    strings: &['"'],
    keywords: &["true", "false", "null"],
    case_insensitive: false,
};

const CSS: LangSpec = LangSpec {
    line_comments: &["//"], // scss/less; plain CSS never contains one
    block_comment: Some(("/*", "*/")),
    strings: &['"', '\''],
    keywords: &[],
    case_insensitive: false,
};

const DOCKER: LangSpec = LangSpec {
    line_comments: &["#"],
    block_comment: None,
    strings: &['"', '\''],
    keywords: &[
        "add", "arg", "cmd", "copy", "entrypoint", "env", "expose", "from", "healthcheck",
        "label", "run", "shell", "user", "volume", "workdir",
    ],
    case_insensitive: true,
};

/// Stateful line tokenizer; block-comment state carries across lines, so
/// feed lines top-down in order.
pub struct Highlighter {
    spec: Option<&'static LangSpec>,
    in_block_comment: bool,
}

impl Highlighter {
    /// Pick the language from the file name; unknown files get a plain
    /// single-run highlighter.
    pub fn for_path(path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let spec = match name.as_str() {
            "makefile" | "gnumakefile" => Some(&SHELL),
            "dockerfile" => Some(&DOCKER),
            _ => name.rsplit_once('.').and_then(|(_, ext)| match ext {
                "rs" => Some(&RUST),
                "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(&JS),
                "py" => Some(&PYTHON),
                "go" => Some(&GO),
                "c" | "h" | "cc" | "cpp" | "hpp" | "hh" | "m" | "mm" => Some(&C),
                "java" | "kt" | "kts" | "swift" | "scala" | "gradle" => Some(&JAVA),
                "rb" => Some(&RUBY),
                "sh" | "bash" | "zsh" | "fish" => Some(&SHELL),
                "sql" => Some(&SQL),
                "toml" | "ini" | "cfg" | "conf" => Some(&TOML),
                "yaml" | "yml" => Some(&YAML),
                "json" | "jsonc" => Some(&JSON),
                "css" | "scss" | "less" => Some(&CSS),
                _ => None,
            }),
        };
        Self {
            spec,
            in_block_comment: false,
        }
    }

    /// No language: every line is one plain-text run.
    pub fn plain() -> Self {
        Self {
            spec: None,
            in_block_comment: false,
        }
    }

    /// Split one line into (kind, text) runs covering the whole line.
    pub fn line(&mut self, line: &str) -> Vec<(TokenKind, String)> {
        let Some(spec) = self.spec else {
            if line.is_empty() {
                return Vec::new();
            }
            return vec![(TokenKind::Text, line.to_string())];
        };
        let chars: Vec<char> = line.chars().collect();
        let mut runs = Runs::default();
        let mut i = 0;
        while i < chars.len() {
            if self.in_block_comment {
                let (_, close) = spec.block_comment.expect("state only set with a block spec");
                match find_from(&chars, i, close) {
                    Some(end) => {
                        let end = end + close.chars().count();
                        runs.push(TokenKind::Comment, &chars[i..end]);
                        self.in_block_comment = false;
                        i = end;
                    }
                    None => {
                        runs.push(TokenKind::Comment, &chars[i..]);
                        i = chars.len();
                    }
                }
                continue;
            }
            if spec
                .line_comments
                .iter()
                .any(|p| starts_with_at(&chars, i, p))
            {
                runs.push(TokenKind::Comment, &chars[i..]);
                break;
            }
            if let Some((open, close)) = spec.block_comment {
                if starts_with_at(&chars, i, open) {
                    let body = i + open.chars().count();
                    match find_from(&chars, body, close) {
                        Some(end) => {
                            let end = end + close.chars().count();
                            runs.push(TokenKind::Comment, &chars[i..end]);
                            i = end;
                        }
                        None => {
                            runs.push(TokenKind::Comment, &chars[i..]);
                            self.in_block_comment = true;
                            i = chars.len();
                        }
                    }
                    continue;
                }
            }
            let c = chars[i];
            if spec.strings.contains(&c) {
                let start = i;
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                    } else if chars[i] == c {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                i = i.min(chars.len());
                runs.push(TokenKind::String, &chars[start..i]);
                continue;
            }
            if c.is_ascii_digit() {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
                {
                    i += 1;
                }
                runs.push(TokenKind::Number, &chars[start..i]);
                continue;
            }
            if c.is_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let hit = if spec.case_insensitive {
                    spec.keywords.contains(&word.to_ascii_lowercase().as_str())
                } else {
                    spec.keywords.contains(&word.as_str())
                };
                let kind = if hit { TokenKind::Keyword } else { TokenKind::Text };
                runs.push(kind, &chars[start..i]);
                continue;
            }
            runs.push(TokenKind::Text, &chars[i..i + 1]);
            i += 1;
        }
        runs.finish()
    }
}

/// Does `pat` start at `chars[i]`?
fn starts_with_at(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(k, pc)| chars.get(i + k) == Some(&pc))
}

/// First index >= `from` where `pat` starts.
fn find_from(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    (from..chars.len()).find(|&j| starts_with_at(chars, j, pat))
}

/// Accumulates (kind, text) runs, merging adjacent same-kind pieces.
#[derive(Default)]
struct Runs {
    out: Vec<(TokenKind, String)>,
    buf: String,
    kind: Option<TokenKind>,
}

impl Runs {
    fn push(&mut self, kind: TokenKind, chars: &[char]) {
        if chars.is_empty() {
            return;
        }
        if self.kind != Some(kind) && !self.buf.is_empty() {
            self.out
                .push((self.kind.unwrap_or(TokenKind::Text), std::mem::take(&mut self.buf)));
        }
        self.kind = Some(kind);
        self.buf.extend(chars);
    }

    fn finish(mut self) -> Vec<(TokenKind, String)> {
        if !self.buf.is_empty() {
            self.out
                .push((self.kind.unwrap_or(TokenKind::Text), self.buf));
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    // `use TokenKind::*` shadows the String type — spell it out.
    fn kinds(hl: &mut Highlighter, line: &str) -> Vec<(TokenKind, std::string::String)> {
        hl.line(line)
    }

    #[test]
    fn rust_keywords_strings_comments_numbers() {
        let mut hl = Highlighter::for_path("src/main.rs");
        let runs = kinds(&mut hl, r#"let x = "hi"; // done"#);
        assert_eq!(
            runs,
            vec![
                (Keyword, "let".into()),
                (Text, " x = ".into()),
                (String, "\"hi\"".into()),
                (Text, "; ".into()),
                (Comment, "// done".into()),
            ]
        );
        let runs = kinds(&mut hl, "foo(42, 0xff)");
        assert_eq!(
            runs,
            vec![
                (Text, "foo(".into()),
                (Number, "42".into()),
                (Text, ", ".into()),
                (Number, "0xff".into()),
                (Text, ")".into()),
            ]
        );
    }

    #[test]
    fn block_comment_state_spans_lines() {
        let mut hl = Highlighter::for_path("a.rs");
        assert_eq!(
            kinds(&mut hl, "fn a() /* start"),
            vec![
                (Keyword, "fn".into()),
                (Text, " a() ".into()),
                (Comment, "/* start".into()),
            ]
        );
        assert_eq!(
            kinds(&mut hl, "still comment"),
            vec![(Comment, "still comment".into())]
        );
        assert_eq!(
            kinds(&mut hl, "end */ let y"),
            vec![
                (Comment, "end */".into()),
                (Text, " ".into()),
                (Keyword, "let".into()),
                (Text, " y".into()),
            ]
        );
    }

    #[test]
    fn escaped_quote_stays_inside_the_string() {
        let mut hl = Highlighter::for_path("a.py");
        let runs = kinds(&mut hl, r#"x = "a\"b" # c"#);
        assert_eq!(
            runs,
            vec![
                (Text, "x = ".into()),
                (String, r#""a\"b""#.into()),
                (Text, " ".into()),
                (Comment, "# c".into()),
            ]
        );
    }

    #[test]
    fn rust_lifetimes_are_not_strings() {
        let mut hl = Highlighter::for_path("a.rs");
        let runs = kinds(&mut hl, "fn f<'a>(x: &'a str)");
        assert!(
            runs.iter().all(|(k, _)| *k != String),
            "lifetime misread as string: {runs:?}"
        );
    }

    #[test]
    fn sql_keywords_match_any_case() {
        let mut hl = Highlighter::for_path("q.sql");
        let runs = kinds(&mut hl, "SELECT id FROM users");
        assert_eq!(runs[0], (Keyword, "SELECT".into()));
        assert!(runs.contains(&(Keyword, "FROM".into())), "{runs:?}");
    }

    #[test]
    fn unknown_extension_is_one_plain_run() {
        let mut hl = Highlighter::for_path("notes.txt");
        assert_eq!(
            kinds(&mut hl, "let x = \"hi\""),
            vec![(Text, "let x = \"hi\"".into())]
        );
        assert!(kinds(&mut hl, "").is_empty());
    }

    #[test]
    fn special_filenames_are_recognized() {
        let mut hl = Highlighter::for_path("app/Dockerfile");
        let runs = kinds(&mut hl, "FROM rust:1.80");
        assert_eq!(runs[0], (Keyword, "FROM".into()));
        let mut hl = Highlighter::for_path("Makefile");
        assert_eq!(
            kinds(&mut hl, "# build"),
            vec![(Comment, "# build".into())]
        );
    }
}
