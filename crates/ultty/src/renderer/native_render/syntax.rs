// ── Code syntax highlighting helpers ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Operator,
    Plain,
}

pub(super) struct CodeToken {
    pub(super) text: String,
    pub(super) kind: TokenKind,
}

/// Returns keyword list for a given language identifier.
pub(super) fn code_keywords(language: &str) -> Vec<&'static str> {
    match language.to_lowercase().as_str() {
        "rust" | "rs" => vec![
            "fn", "let", "mut", "const", "static", "struct", "enum", "impl", "trait", "pub", "use",
            "mod", "crate", "self", "super", "return", "if", "else", "match", "for", "while",
            "loop", "break", "continue", "where", "type", "async", "await", "move", "ref", "as",
            "in", "true", "false",
        ],
        "python" | "py" => vec![
            "def", "class", "import", "from", "return", "if", "elif", "else", "for", "while",
            "break", "continue", "pass", "with", "as", "try", "except", "finally", "raise",
            "yield", "lambda", "and", "or", "not", "in", "is", "True", "False", "None", "self",
            "async", "await",
        ],
        "javascript" | "js" | "typescript" | "ts" => vec![
            "function",
            "const",
            "let",
            "var",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "class",
            "extends",
            "import",
            "export",
            "from",
            "default",
            "new",
            "this",
            "typeof",
            "instanceof",
            "async",
            "await",
            "try",
            "catch",
            "finally",
            "throw",
            "true",
            "false",
            "null",
            "undefined",
        ],
        "go" => vec![
            "func",
            "package",
            "import",
            "return",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "break",
            "continue",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "go",
            "defer",
            "select",
            "var",
            "const",
            "true",
            "false",
            "nil",
        ],
        "c" | "cpp" | "c++" | "h" => vec![
            "int", "char", "float", "double", "void", "return", "if", "else", "for", "while", "do",
            "switch", "case", "break", "continue", "struct", "enum", "typedef", "const", "static",
            "extern", "include", "define", "true", "false", "NULL", "nullptr",
        ],
        "ruby" | "rb" => vec![
            "def",
            "end",
            "class",
            "module",
            "require",
            "include",
            "extend",
            "return",
            "if",
            "elsif",
            "else",
            "unless",
            "case",
            "when",
            "for",
            "while",
            "until",
            "do",
            "begin",
            "rescue",
            "ensure",
            "raise",
            "yield",
            "block_given?",
            "self",
            "super",
            "nil",
            "true",
            "false",
            "and",
            "or",
            "not",
            "attr_reader",
            "attr_writer",
            "attr_accessor",
            "puts",
            "print",
            "lambda",
            "proc",
        ],
        "java" => vec![
            "public",
            "private",
            "protected",
            "static",
            "final",
            "abstract",
            "class",
            "interface",
            "extends",
            "implements",
            "import",
            "package",
            "return",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "new",
            "this",
            "super",
            "try",
            "catch",
            "finally",
            "throw",
            "throws",
            "void",
            "int",
            "long",
            "double",
            "float",
            "boolean",
            "char",
            "byte",
            "short",
            "String",
            "true",
            "false",
            "null",
            "instanceof",
            "synchronized",
            "volatile",
        ],
        _ => vec![
            "fn", "function", "def", "class", "return", "if", "else", "for", "while", "import",
            "from", "let", "const", "var", "true", "false",
        ],
    }
}

/// Multi-character operators to highlight.
const OPERATORS: &[&str] = &[
    "=>", "->", "::", "+=", "-=", "*=", "/=", "%=", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>",
    "..", "?.",
];

/// Check if a trimmed line starts with a line-comment prefix for the given language.
pub(super) fn is_line_comment(trimmed: &str, language: &str) -> bool {
    // // is a comment in all languages
    if trimmed.starts_with("//") {
        return true;
    }
    // # is a comment in scripting languages; also for unknown/empty (backward compat)
    if trimmed.starts_with('#') {
        let lang = language.to_lowercase();
        let known_no_hash = matches!(
            lang.as_str(),
            "rust"
                | "rs"
                | "c"
                | "cpp"
                | "c++"
                | "h"
                | "javascript"
                | "js"
                | "typescript"
                | "ts"
                | "java"
                | "go"
                | "sql"
                | "lua"
                | "haskell"
                | "hs"
        );
        return !known_no_hash;
    }
    // -- is a comment only in SQL, Lua, Haskell
    if trimmed.starts_with("--") {
        let lang = language.to_lowercase();
        return matches!(lang.as_str(), "sql" | "lua" | "haskell" | "hs");
    }
    false
}

/// Check if a word looks like a type name (starts with uppercase letter).
pub(super) fn is_type_like(word: &str) -> bool {
    let first = match word.chars().next() {
        Some(c) => c,
        None => return false,
    };
    first.is_ascii_uppercase()
        && word.len() > 1
        && word.chars().skip(1).any(|c| c.is_ascii_lowercase())
}

/// Tokenize a line of code into styled segments.
#[allow(clippy::too_many_lines)]
pub(super) fn tokenize_code_line(
    line: &str,
    keywords: &[&str],
    in_block_comment: &mut bool,
) -> Vec<CodeToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Block comment start: /*
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            let mut s = String::from("/*");
            i += 2;
            while i < len {
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                    s.push_str("*/");
                    i += 2;
                    // Block comment closed on same line
                    tokens.push(CodeToken {
                        text: s,
                        kind: TokenKind::Comment,
                    });
                    s = String::new();
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            if !s.is_empty() {
                // Block comment continues to next line
                *in_block_comment = true;
                tokens.push(CodeToken {
                    text: s,
                    kind: TokenKind::Comment,
                });
            }
            continue;
        }

        // Inline comment: //
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            tokens.push(CodeToken {
                text: chars[i..].iter().collect(),
                kind: TokenKind::Comment,
            });
            break;
        }

        // Python triple-quoted strings: """...""" or '''...'''
        if i + 2 < len
            && (chars[i] == '"' || chars[i] == '\'')
            && chars[i + 1] == chars[i]
            && chars[i + 2] == chars[i]
        {
            let quote = chars[i];
            let mut s: String = chars[i..i + 3].iter().collect();
            i += 3;
            loop {
                if i + 2 < len
                    && chars[i] == quote
                    && chars[i + 1] == quote
                    && chars[i + 2] == quote
                {
                    s.push(quote);
                    s.push(quote);
                    s.push(quote);
                    i += 3;
                    break;
                }
                if i >= len {
                    break;
                }
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // Rust raw strings: r"..." or r#"..."#
        if chars[i] == 'r' && i + 1 < len && (chars[i + 1] == '"' || chars[i + 1] == '#') {
            let mut s = String::from('r');
            let mut j = i + 1;
            let mut hash_count = 0usize;
            while j < len && chars[j] == '#' {
                s.push('#');
                hash_count += 1;
                j += 1;
            }
            if j < len && chars[j] == '"' {
                s.push('"');
                j += 1;
                // Consume until closing " followed by hash_count #'s
                'raw_outer: loop {
                    if j >= len {
                        break;
                    }
                    if chars[j] == '"' {
                        // Check for matching closing hashes
                        let mut ok = true;
                        for k in 1..=hash_count {
                            if j + k >= len || chars[j + k] != '#' {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            s.push('"');
                            j += 1;
                            for _ in 0..hash_count {
                                s.push('#');
                                j += 1;
                            }
                            break 'raw_outer;
                        }
                    }
                    s.push(chars[j]);
                    j += 1;
                }
                i = j;
                tokens.push(CodeToken {
                    text: s,
                    kind: TokenKind::String,
                });
                continue;
            }
            // Not a raw string (e.g. just identifier starting with 'r'), fall through
        }

        // Backtick template literals (JS/TS): `...`
        if chars[i] == '`' {
            let mut s = String::from('`');
            i += 1;
            while i < len && chars[i] != '`' {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < len {
                s.push('`');
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // String literals: "..." or '...'
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            let mut s = String::new();
            s.push(chars[i]);
            i += 1;
            while i < len && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < len {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::String,
            });
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit())
        {
            let mut s = String::new();
            while i < len
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(CodeToken {
                text: s,
                kind: TokenKind::Number,
            });
            continue;
        }

        // Identifiers / keywords / types
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            let mut word = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                i += 1;
            }
            let kind = if keywords.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if is_type_like(&word) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            tokens.push(CodeToken { text: word, kind });
            continue;
        }

        // Multi-character operators
        let remaining: String = chars[i..].iter().collect();
        let mut matched_op = false;
        for op in OPERATORS {
            if remaining.starts_with(op) {
                tokens.push(CodeToken {
                    text: op.to_string(),
                    kind: TokenKind::Operator,
                });
                i += op.len();
                matched_op = true;
                break;
            }
        }
        if matched_op {
            continue;
        }

        // Whitespace and single-char operators — accumulate as plain
        let mut plain = String::new();
        while i < len
            && !chars[i].is_alphanumeric()
            && chars[i] != '_'
            && chars[i] != '"'
            && chars[i] != '\''
            && chars[i] != '`'
            && !(i + 1 < len && chars[i] == '/' && chars[i + 1] == '/')
            && !(i + 1 < len && chars[i] == '/' && chars[i + 1] == '*')
        {
            // Check for multi-char operators before consuming
            let rest: String = chars[i..].iter().collect();
            if OPERATORS.iter().any(|op| rest.starts_with(op)) {
                break;
            }
            plain.push(chars[i]);
            i += 1;
        }
        if !plain.is_empty() {
            tokens.push(CodeToken {
                text: plain,
                kind: TokenKind::Plain,
            });
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── code_keywords tests ─────────────────────────────────────────

    #[test]
    fn code_keywords_rust_and_rs_are_equivalent() {
        let rust = code_keywords("rust");
        let rs = code_keywords("rs");
        assert_eq!(rust, rs);
        assert!(rust.contains(&"fn"));
        assert!(rust.contains(&"let"));
        assert!(rust.contains(&"struct"));
    }

    #[test]
    fn code_keywords_python() {
        let py = code_keywords("python");
        assert_eq!(py, code_keywords("py"));
        assert!(py.contains(&"def"));
        assert!(py.contains(&"class"));
        assert!(py.contains(&"lambda"));
    }

    #[test]
    fn code_keywords_javascript_typescript() {
        let js = code_keywords("javascript");
        assert_eq!(js, code_keywords("js"));
        assert_eq!(js, code_keywords("typescript"));
        assert_eq!(js, code_keywords("ts"));
        assert!(js.contains(&"function"));
        assert!(js.contains(&"const"));
        assert!(js.contains(&"async"));
    }

    #[test]
    fn code_keywords_unknown_returns_default_and_known_non_empty() {
        let default = code_keywords("brainfuck");
        assert!(default.contains(&"fn"));
        assert!(default.contains(&"function"));
        assert!(default.contains(&"def"));

        // Known languages return non-empty lists
        for lang in &["go", "c", "ruby", "java"] {
            assert!(
                !code_keywords(lang).is_empty(),
                "{lang} should have keywords"
            );
        }
    }

    // ── is_type_like tests ──────────────────────────────────────────

    #[test]
    fn is_type_like_uppercase_start_with_lowercase() {
        assert!(is_type_like("String"));
        assert!(is_type_like("Vec"));
        assert!(is_type_like("MyStruct"));
        assert!(is_type_like("Ab"));
    }

    #[test]
    fn is_type_like_rejects_lowercase_start() {
        assert!(!is_type_like("string"));
        assert!(!is_type_like("myStruct"));
    }

    #[test]
    fn is_type_like_rejects_all_uppercase() {
        assert!(!is_type_like("HTTP"));
        assert!(!is_type_like("ABC"));
    }

    #[test]
    fn is_type_like_rejects_empty_and_single_char() {
        assert!(!is_type_like(""));
        assert!(!is_type_like("A"));
    }

    #[test]
    fn is_type_like_edge_cases() {
        assert!(!is_type_like("IO")); // all uppercase, no lowercase after first
        assert!(is_type_like("Io")); // upper + lower
        assert!(!is_type_like("123")); // digits
    }

    // ── tokenize_code_line tests ────────────────────────────────────

    fn rust_keywords() -> Vec<&'static str> {
        code_keywords("rust")
    }

    #[test]
    fn tokenize_keyword_in_rust() {
        let mut bc = false;
        let tokens = tokenize_code_line("let x = 5", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].text, "let");
        assert!(!bc);
    }

    #[test]
    fn tokenize_string_double_quotes() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""hello world""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "\"hello world\"");
    }

    #[test]
    fn tokenize_string_with_escaped_quote() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""hello \"world\"""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.contains("\\\""));
    }

    #[test]
    fn tokenize_single_quoted_string() {
        let mut bc = false;
        let tokens = tokenize_code_line("'c'", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "'c'");
    }

    #[test]
    fn tokenize_inline_comment() {
        let mut bc = false;
        let tokens = tokenize_code_line("x // comment", &rust_keywords(), &mut bc);
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert_eq!(comment_token.text, "// comment");
        assert!(!bc);
    }

    #[test]
    fn tokenize_block_comment_single_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("a /* b */ c", &rust_keywords(), &mut bc);
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert!(comment_token.text.contains("/* b */"));
        assert!(!bc, "block comment should close on same line");
    }

    #[test]
    fn tokenize_block_comment_spanning_lines() {
        let mut bc = false;
        let tokens = tokenize_code_line("a /* b", &rust_keywords(), &mut bc);
        assert!(bc, "block comment should remain open");
        let comment_token = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .unwrap();
        assert!(comment_token.text.starts_with("/*"));
    }

    #[test]
    fn tokenize_integer_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("42", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "42");
    }

    #[test]
    fn tokenize_float_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("3.14", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "3.14");
    }

    #[test]
    fn tokenize_hex_number() {
        let mut bc = false;
        let tokens = tokenize_code_line("0xFF", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].text, "0xFF");
    }

    #[test]
    fn tokenize_type_like_word() {
        let mut bc = false;
        let tokens = tokenize_code_line("Vec", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Type);
        assert_eq!(tokens[0].text, "Vec");
    }

    #[test]
    fn tokenize_multi_char_operator() {
        let mut bc = false;
        let tokens = tokenize_code_line("=>", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Operator);
        assert_eq!(tokens[0].text, "=>");
    }

    #[test]
    fn tokenize_fn_main_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("fn main() {", &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].text, "fn");
        // "main" should be Plain (not a keyword)
        let main_tok = tokens.iter().find(|t| t.text == "main").unwrap();
        assert_eq!(main_tok.kind, TokenKind::Plain);
    }

    #[test]
    fn tokenize_empty_line() {
        let mut bc = false;
        let tokens = tokenize_code_line("", &rust_keywords(), &mut bc);
        assert!(tokens.is_empty());
    }

    #[test]
    fn tokenize_whitespace_only() {
        let mut bc = false;
        let tokens = tokenize_code_line("   ", &rust_keywords(), &mut bc);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Plain);
        assert_eq!(tokens[0].text, "   ");
    }

    #[test]
    fn tokenize_operator_and_and() {
        let mut bc = false;
        let tokens = tokenize_code_line("a && b", &rust_keywords(), &mut bc);
        let op = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Operator)
            .unwrap();
        assert_eq!(op.text, "&&");
    }

    // ── is_line_comment tests ───────────────────────────────────────

    #[test]
    fn rust_attribute_not_comment() {
        assert!(!is_line_comment("#[derive(Debug)]", "rust"));
        assert!(!is_line_comment("#[cfg(test)]", "rs"));
    }

    #[test]
    fn c_include_not_comment() {
        assert!(!is_line_comment("#include <stdio.h>", "c"));
        assert!(!is_line_comment("#define FOO", "cpp"));
    }

    #[test]
    fn python_hash_is_comment() {
        assert!(is_line_comment("# this is a comment", "python"));
        assert!(is_line_comment("# comment", "py"));
    }

    #[test]
    fn sql_double_dash_is_comment() {
        assert!(is_line_comment("-- select all", "sql"));
        assert!(is_line_comment("-- comment", "lua"));
    }

    #[test]
    fn unknown_language_hash_is_comment() {
        assert!(is_line_comment("# comment", ""));
        assert!(is_line_comment("# comment", "unknown_lang"));
        // But -- is not a comment for unknown languages
        assert!(!is_line_comment("-- something", ""));
    }

    #[test]
    fn double_slash_comment_all_languages() {
        assert!(is_line_comment("// comment", "rust"));
        assert!(is_line_comment("// comment", "python"));
        assert!(is_line_comment("// comment", "sql"));
        assert!(is_line_comment("// comment", ""));
    }

    // ── tokenizer string literal improvement tests ──────────────────

    #[test]
    fn tokenize_backtick_simple() {
        let mut bc = false;
        let tokens = tokenize_code_line("`hello world`", &code_keywords("js"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, "`hello world`");
    }

    #[test]
    fn tokenize_backtick_with_escape() {
        let mut bc = false;
        let tokens = tokenize_code_line(r"`hello \` world`", &code_keywords("js"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with('`'));
        assert!(tokens[0].text.ends_with('`'));
    }

    #[test]
    fn tokenize_raw_string_no_hash() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#"r"hello""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#"r"hello""#);
    }

    #[test]
    fn tokenize_raw_string_with_hashes() {
        let mut bc = false;
        let input = r###"r#"hello "world""#"###;
        let tokens = tokenize_code_line(input, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("r#\""));
        assert!(tokens[0].text.ends_with("\"#"));
    }

    #[test]
    fn tokenize_triple_quote_double() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""""hello world""""#, &code_keywords("python"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("\"\"\""));
        assert!(tokens[0].text.ends_with("\"\"\""));
    }

    #[test]
    fn tokenize_triple_quote_single() {
        let mut bc = false;
        let tokens = tokenize_code_line("'''hello world'''", &code_keywords("python"), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].text.starts_with("'''"));
        assert!(tokens[0].text.ends_with("'''"));
    }

    // ── tokenizer edge case tests ───────────────────────────────────

    #[test]
    fn tokenize_unclosed_string_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#"let s = "unterminated"#, &rust_keywords(), &mut bc);
        // Should still produce a String token (without closing quote)
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with('"'));
        assert!(!string_tok.text.ends_with('"'));
    }

    #[test]
    fn tokenize_escaped_backslash_at_end() {
        let mut bc = false;
        // "test\\" — the \\ is an escaped backslash, string should close properly
        let tokens = tokenize_code_line(r#""test\\""#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#""test\\""#);
    }

    #[test]
    fn tokenize_nested_block_comment() {
        let mut bc = false;
        // /* /* */ */ — first */ closes the comment, second */ is plain text
        let tokens = tokenize_code_line("/* /* */ */", &rust_keywords(), &mut bc);
        let comment = tokens
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .expect("should have comment token");
        assert_eq!(comment.text, "/* /* */");
        assert!(!bc, "block comment should be closed");
        // Remaining " */" should be plain tokens
        let rest: String = tokens.iter().skip(1).map(|t| t.text.as_str()).collect();
        assert!(rest.contains("*/"));
    }

    #[test]
    fn tokenize_unicode_identifier() {
        let mut bc = false;
        let tokens = tokenize_code_line("let café = 42", &rust_keywords(), &mut bc);
        let kw = &tokens[0];
        assert_eq!(kw.kind, TokenKind::Keyword);
        assert_eq!(kw.text, "let");
        let ident = tokens
            .iter()
            .find(|t| t.text.contains("caf"))
            .expect("should find café identifier");
        assert_eq!(ident.text, "café");
        assert_eq!(ident.kind, TokenKind::Plain);
    }

    #[test]
    fn tokenize_mixed_quotes() {
        let mut bc = false;
        // 'hello"world' — single-quoted string containing a double quote
        let tokens = tokenize_code_line(r#"'hello"world'"#, &rust_keywords(), &mut bc);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].text, r#"'hello"world'"#);
    }

    #[test]
    fn tokenize_unclosed_backtick_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line("`unterminated", &code_keywords("js"), &mut bc);
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with('`'));
        assert!(!string_tok.text.ends_with('`'));
    }

    #[test]
    fn tokenize_unclosed_triple_quote_at_eol() {
        let mut bc = false;
        let tokens = tokenize_code_line(r#""""unterminated"#, &code_keywords("python"), &mut bc);
        let string_tok = tokens
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .expect("should have a string token");
        assert!(string_tok.text.starts_with("\"\"\""));
        // Not closed, so should contain the rest of the line
        assert!(string_tok.text.contains("unterminated"));
    }
}
