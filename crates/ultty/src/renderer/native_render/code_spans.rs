//! Native code styled span planning.

use super::syntax::{is_line_comment, tokenize_code_line, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodeColor {
    LineNumber,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Operator,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeSpan {
    pub(super) text: String,
    pub(super) color: CodeColor,
    pub(super) bold: bool,
    pub(super) italic: bool,
}

pub(super) fn plan_code_content_spans(
    source: &str,
    language: &str,
    line_numbers: bool,
) -> Vec<Vec<CodeSpan>> {
    let keywords = super::syntax::code_keywords(language);
    let mut in_block_comment = false;

    source
        .lines()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = Vec::new();
            if line_numbers {
                spans.push(code_span(
                    format!("{:4} ", idx + 1),
                    CodeColor::LineNumber,
                    false,
                    false,
                ));
            }
            spans.extend(plan_code_line_spans(
                line,
                language,
                &keywords,
                &mut in_block_comment,
            ));
            spans
        })
        .collect()
}

pub(super) fn plan_code_line_spans(
    line: &str,
    language: &str,
    keywords: &[&str],
    in_block_comment: &mut bool,
) -> Vec<CodeSpan> {
    if *in_block_comment {
        if let Some(end_pos) = line.find("*/") {
            let comment_part = &line[..end_pos + 2];
            let rest = &line[end_pos + 2..];
            *in_block_comment = false;

            let mut spans = vec![comment_span(comment_part)];
            if !rest.is_empty() {
                spans.extend(plan_code_line_spans(
                    rest,
                    language,
                    keywords,
                    in_block_comment,
                ));
            }
            return spans;
        }

        return vec![comment_span(line)];
    }

    let trimmed = line.trim_start();
    if is_line_comment(trimmed, language) {
        return vec![comment_span(line)];
    }

    tokenize_code_line(line, keywords, in_block_comment)
        .into_iter()
        .map(|token| span_for_token(token.text, token.kind))
        .collect()
}

fn span_for_token(text: String, kind: TokenKind) -> CodeSpan {
    let (color, bold, italic) = match kind {
        TokenKind::Keyword => (CodeColor::Keyword, true, false),
        TokenKind::String => (CodeColor::String, false, false),
        TokenKind::Comment => (CodeColor::Comment, false, true),
        TokenKind::Number => (CodeColor::Number, false, false),
        TokenKind::Type => (CodeColor::Type, false, false),
        TokenKind::Operator => (CodeColor::Operator, false, false),
        TokenKind::Plain => (CodeColor::Plain, false, false),
    };

    CodeSpan {
        text,
        color,
        bold,
        italic,
    }
}

fn comment_span(text: &str) -> CodeSpan {
    code_span(text.to_string(), CodeColor::Comment, false, true)
}

fn code_span(text: String, color: CodeColor, bold: bool, italic: bool) -> CodeSpan {
    CodeSpan {
        text,
        color,
        bold,
        italic,
    }
}

#[cfg(test)]
mod tests {
    use super::{plan_code_line_spans, CodeColor};

    #[test]
    fn plans_representative_styled_spans() {
        let mut in_block_comment = false;
        let keywords = super::super::syntax::code_keywords("rust");
        let spans = plan_code_line_spans(
            "let Name = 42 // ok",
            "rust",
            &keywords,
            &mut in_block_comment,
        );

        assert_eq!(spans[0].text, "let");
        assert_eq!(spans[0].color, CodeColor::Keyword);
        assert!(spans[0].bold);
        assert_eq!(spans[2].text, "Name");
        assert_eq!(spans[2].color, CodeColor::Type);
        assert!(spans.iter().any(|span| span.color == CodeColor::Number));
        assert_eq!(
            spans.last().map(|span| span.color),
            Some(CodeColor::Comment)
        );
        assert_eq!(spans.last().map(|span| span.italic), Some(true));
        assert!(!in_block_comment);
    }

    #[test]
    fn plans_content_line_numbers_without_changing_code_spans() {
        let lines = super::plan_code_content_spans("let x = 1", "rust", true);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "   1 ");
        assert_eq!(lines[0][0].color, CodeColor::LineNumber);
        assert!(lines[0]
            .iter()
            .any(|span| span.text == "let" && span.color == CodeColor::Keyword));
    }

    #[test]
    fn plans_markdown_fenced_code_with_shared_code_spans() {
        let blocks = super::super::markdown_blocks::plan_markdown_blocks("```rust\nlet x = 1\n```");
        let (language, lines) = match &blocks[0] {
            super::super::markdown_blocks::MarkdownBlock::CodeBlock { language, lines } => {
                (*language, lines)
            }
            _ => panic!("expected fenced code block"),
        };

        let source = lines.join("\n");
        let planned_lines = super::plan_code_content_spans(&source, language, false);

        assert_eq!(planned_lines.len(), 1);
        assert_ne!(planned_lines[0][0].color, CodeColor::LineNumber);
        assert!(planned_lines[0]
            .iter()
            .any(|span| span.text == "let" && span.color == CodeColor::Keyword));
    }

    #[test]
    fn plans_full_line_comment_as_single_comment_span() {
        let mut in_block_comment = false;
        let keywords = super::super::syntax::code_keywords("rust");
        let spans = plan_code_line_spans("  // comment", "rust", &keywords, &mut in_block_comment);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "  // comment");
        assert_eq!(spans[0].color, CodeColor::Comment);
        assert!(spans[0].italic);
    }

    #[test]
    fn plans_block_comment_continuation_and_rest() {
        let mut in_block_comment = true;
        let keywords = super::super::syntax::code_keywords("rust");
        let spans =
            plan_code_line_spans("still */ let x", "rust", &keywords, &mut in_block_comment);

        assert_eq!(spans[0].text, "still */");
        assert_eq!(spans[0].color, CodeColor::Comment);
        assert!(spans
            .iter()
            .any(|span| span.text == "let" && span.color == CodeColor::Keyword && span.bold));
        assert!(!in_block_comment);
    }

    #[test]
    fn plans_unclosed_block_comment_continuation() {
        let mut in_block_comment = false;
        let keywords = super::super::syntax::code_keywords("rust");
        let spans =
            plan_code_line_spans("let x = /* wait", "rust", &keywords, &mut in_block_comment);

        assert_eq!(spans.last().map(|span| span.text.as_str()), Some("/* wait"));
        assert_eq!(
            spans.last().map(|span| span.color),
            Some(CodeColor::Comment)
        );
        assert!(in_block_comment);
    }
}
