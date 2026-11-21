//! Markdown, one line at a time, styled without changing its text.
//!
//! The console shows the source as the agent wrote it, with styles: a
//! heading is bold, inline code and fenced blocks are coloured, and the
//! markup (backticks, `**`, fences, list markers) stays, dimmed or
//! coloured. Keeping every character means a copy of the transcript is the
//! Markdown the agent wrote, indentation and all, and a line the terminal
//! wraps re-wraps like the source when the window is resized.

use crate::style::{Line, Span, Tone};

/// Styles Markdown lines in order, remembering whether a fenced code block
/// is open.
#[derive(Debug, Default)]
pub struct Markdown {
    /// The fence (three or more backticks or tildes) of the open code
    /// block, if one is open.
    fence: Option<String>,
}

impl Markdown {
    /// A styler outside any code block.
    pub fn new() -> Self {
        Self::default()
    }

    /// Style `source`, a complete line, and move past it.
    pub fn line(&mut self, source: &str) -> Line {
        let (line, fence) = self.style(source);
        self.fence = fence;
        line
    }

    /// Style `source`, a line still being typed, as it would be if it ended
    /// here.
    pub fn peek(&self, source: &str) -> Line {
        self.style(source).0
    }

    /// Close any code block left open, at the end of a turn.
    pub fn end(&mut self) {
        self.fence = None;
    }

    /// `source` styled, and the open fence after it.
    fn style(&self, source: &str) -> (Line, Option<String>) {
        let trimmed = source.trim_start();
        if let Some(open) = &self.fence {
            let closes =
                trimmed.starts_with(open.as_str()) && trimmed[open.len()..].trim().is_empty();
            let tone = if closes { Tone::Muted } else { Tone::Code };
            let fence = (!closes).then(|| open.clone());
            return (Line::styled(source, tone), fence);
        }
        if let Some(fence) = opening_fence(trimmed) {
            return (Line::styled(source, Tone::Muted), Some(fence));
        }
        let line = if heading_level(source).is_some() {
            Line::styled(source, Tone::Accent).bold()
        } else if let Some(rest) = source.strip_prefix("> ") {
            let mut line = Line::styled("> ", Tone::Muted);
            line.spans.extend(inline(rest));
            line
        } else if let Some(marker) = list_marker(source) {
            let mut line = Line::styled(&source[..marker], Tone::Accent);
            line.spans.extend(inline(&source[marker..]));
            line
        } else {
            Line {
                spans: inline(source),
            }
        };
        (line, None)
    }
}

/// The fence a line opens a code block with: three or more backticks or
/// tildes, then an optional info string.
fn opening_fence(trimmed: &str) -> Option<String> {
    let first = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = trimmed.chars().take_while(|&c| c == first).count();
    (run >= 3).then(|| first.to_string().repeat(run))
}

/// The level of an ATX heading (`#` to `######` and a space).
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// The byte length of a list item's indentation, marker and space (`- `,
/// `* `, `+ `, `1. `, `2) `), if the line is one.
fn list_marker(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let marker = if rest.starts_with(['-', '*', '+']) {
        1
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || !rest[digits..].starts_with(['.', ')']) {
            return None;
        }
        digits + 1
    };
    rest[marker..]
        .starts_with(' ')
        .then_some(indent + marker + 1)
}

/// Spans for inline Markdown: `code`, **bold** and *italic*. The markers
/// stay in the text, muted.
fn inline(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        let styled = match ch {
            '`' => delimited(rest, "`")
                .map(|(inner, len)| (vec![Span::new(inner, Tone::Code)], "`", len)),
            '*' if rest.starts_with("**") => delimited(rest, "**").map(|(inner, len)| {
                let mut span = Span::new(inner, Tone::Plain);
                span.bold = true;
                (vec![span], "**", len)
            }),
            '*' => delimited(rest, "*")
                .filter(|(inner, _)| !inner.starts_with(' ') && !inner.ends_with(' '))
                .map(|(inner, len)| {
                    let mut span = Span::new(inner, Tone::Plain);
                    span.italic = true;
                    (vec![span], "*", len)
                }),
            _ => None,
        };
        match styled {
            Some((inner, marker, len)) => {
                if !plain.is_empty() {
                    spans.push(Span::new(std::mem::take(&mut plain), Tone::Plain));
                }
                spans.push(Span::new(marker, Tone::Muted));
                spans.extend(inner);
                spans.push(Span::new(marker, Tone::Muted));
                rest = &rest[len..];
            }
            None => {
                plain.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    if !plain.is_empty() {
        spans.push(Span::new(plain, Tone::Plain));
    }
    spans
}

/// If `text` starts with `marker`, some text and `marker` again: the text
/// between, and the byte length of the whole run.
fn delimited<'a>(text: &'a str, marker: &str) -> Option<(&'a str, usize)> {
    let after = text.strip_prefix(marker)?;
    let end = after.find(marker).filter(|&end| end > 0)?;
    Some((&after[..end], marker.len() * 2 + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(line: &Line) -> Vec<(String, Tone, bool, bool)> {
        line.spans
            .iter()
            .map(|s| (s.text.clone(), s.tone, s.bold, s.italic))
            .collect()
    }

    #[test]
    fn test_every_line_keeps_its_source_text() {
        let source = "# Title\n\nSome `code`, **bold** and *it*.\n\n1. one\n   - nested `x`\n> quoted\n```rust\nfn f() {\n    let a = `b`;\n}\n```\nafter * star and ** pair";
        let mut md = Markdown::new();
        for line in source.lines() {
            assert_eq!(md.line(line).text(), line);
        }
    }

    #[test]
    fn test_inline_code_bold_and_italic_are_styled_with_muted_markers() {
        let line = Markdown::new().line("a `b` **c** *d*");
        assert_eq!(
            tones(&line),
            [
                ("a ".into(), Tone::Plain, false, false),
                ("`".into(), Tone::Muted, false, false),
                ("b".into(), Tone::Code, false, false),
                ("`".into(), Tone::Muted, false, false),
                (" ".into(), Tone::Plain, false, false),
                ("**".into(), Tone::Muted, false, false),
                ("c".into(), Tone::Plain, true, false),
                ("**".into(), Tone::Muted, false, false),
                (" ".into(), Tone::Plain, false, false),
                ("*".into(), Tone::Muted, false, false),
                ("d".into(), Tone::Plain, false, true),
                ("*".into(), Tone::Muted, false, false),
            ]
        );
    }

    #[test]
    fn test_unpaired_markers_stay_plain() {
        let line = Markdown::new().line("2 * 3 and a ` tick");
        assert_eq!(
            tones(&line),
            [("2 * 3 and a ` tick".into(), Tone::Plain, false, false)]
        );
    }

    #[test]
    fn test_fenced_block_lines_are_code_until_the_matching_fence() {
        let mut md = Markdown::new();
        assert_eq!(md.line("````md").spans[0].tone, Tone::Muted);
        assert_eq!(md.line("```").spans[0].tone, Tone::Code);
        assert_eq!(md.line("# not a heading").spans[0].tone, Tone::Code);
        assert_eq!(md.line("````").spans[0].tone, Tone::Muted);
        assert_eq!(md.line("# heading").spans[0].tone, Tone::Accent);
    }

    #[test]
    fn test_peek_does_not_open_a_block() {
        let mut md = Markdown::new();
        assert_eq!(md.peek("```rust").spans[0].tone, Tone::Muted);
        assert_eq!(md.line("`x`").spans[1].tone, Tone::Code);
        md.line("```");
        assert_eq!(md.peek("let a").spans[0].tone, Tone::Code);
        md.end();
        assert_eq!(md.line("let a").spans[0].tone, Tone::Plain);
    }

    #[test]
    fn test_list_markers_are_accented() {
        let mut md = Markdown::new();
        assert_eq!(md.line("1. one").spans[0].text, "1. ");
        assert_eq!(md.line("   - two").spans[0].text, "   - ");
        assert_eq!(md.line("-not a list").spans[0].tone, Tone::Plain);
        assert_eq!(md.line("12) twelve").spans[0].tone, Tone::Accent);
    }
}
