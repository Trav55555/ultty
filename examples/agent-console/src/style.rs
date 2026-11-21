//! Styled lines: what the console shows, independent of how a mode draws
//! it. ANSI mode turns a [`Line`] into SGR sequences, native mode into the
//! spans of a card's rich text.

use unicode_width::UnicodeWidthChar;

/// What a span of text is, which decides its colour in either mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Ordinary text in the terminal's colours.
    #[default]
    Plain,
    /// Headings, list markers, prompts and card titles.
    Accent,
    /// Code: inline code and the lines of a fenced block.
    Code,
    /// Markup the reader can skip: backticks, fences, the status line.
    Muted,
    /// A line a diff adds.
    Added,
    /// A line a diff removes.
    Removed,
    /// Standard error, and requests that need the user.
    Warn,
    /// A failed tool call.
    Error,
    /// A tool call that succeeded.
    Success,
}

impl Tone {
    /// The palette index (0 to 15) of this tone's foreground, if it has one.
    /// Both modes use the terminal's own palette, so the colours follow the
    /// user's theme.
    pub fn palette_index(self) -> Option<u8> {
        match self {
            Self::Plain => None,
            Self::Accent => Some(6),
            Self::Code => Some(3),
            Self::Muted => Some(8),
            Self::Added | Self::Success => Some(2),
            Self::Removed | Self::Error => Some(1),
            Self::Warn => Some(11),
        }
    }
}

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    /// The text, free of control characters.
    pub text: String,
    /// Its colour.
    pub tone: Tone,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
}

impl Span {
    /// `text` in `tone`, neither bold nor italic.
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
            italic: false,
        }
    }

    fn is_plain(&self) -> bool {
        self.tone == Tone::Plain && !self.bold && !self.italic
    }

    /// The SGR parameters that select this span's style after a reset.
    fn sgr(&self) -> String {
        let mut codes = vec!["0".to_string()];
        if self.bold {
            codes.push("1".into());
        }
        if self.italic {
            codes.push("3".into());
        }
        match self.tone.palette_index() {
            Some(n @ 0..=7) => codes.push((30 + n).to_string()),
            Some(n) => codes.push((90 + n - 8).to_string()),
            None => {}
        }
        codes.join(";")
    }
}

/// One line of the transcript: spans that never hold a newline.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    /// The spans, in order.
    pub spans: Vec<Span>,
}

impl Line {
    /// An empty line.
    pub fn new() -> Self {
        Self::default()
    }

    /// `text` as one span in `tone`.
    pub fn styled(text: impl Into<String>, tone: Tone) -> Self {
        Self::new().push(text, tone)
    }

    /// This line with `text` in `tone` appended.
    pub fn push(mut self, text: impl Into<String>, tone: Tone) -> Self {
        self.spans.push(Span::new(text, tone));
        self
    }

    /// This line with its last span made bold.
    pub fn bold(mut self) -> Self {
        if let Some(span) = self.spans.last_mut() {
            span.bold = true;
        }
        self
    }

    /// The text without styles.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// How many columns the line takes.
    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .flat_map(|s| s.text.chars())
            .map(|ch| ch.width().unwrap_or(0))
            .sum()
    }

    /// The line without trailing whitespace, so nothing the console prints
    /// ends in spaces.
    pub fn trim_end(mut self) -> Self {
        while let Some(last) = self.spans.last_mut() {
            let kept = last.text.trim_end().len();
            last.text.truncate(kept);
            if !last.text.is_empty() {
                break;
            }
            self.spans.pop();
        }
        self
    }

    /// The first `width` columns of the line. A wide character that would
    /// straddle the edge is left out.
    pub fn clip(&self, width: usize) -> Self {
        let mut left = width;
        let mut clipped = Line::new();
        for span in &self.spans {
            let mut text = String::new();
            for ch in span.text.chars() {
                let w = ch.width().unwrap_or(0);
                if w > left {
                    left = 0;
                    break;
                }
                left -= w;
                text.push(ch);
            }
            if !text.is_empty() {
                clipped.spans.push(Span {
                    text,
                    ..span.clone()
                });
            }
            if left == 0 {
                break;
            }
        }
        clipped.trim_end()
    }

    /// The last columns of the line in at most `width` columns, behind an
    /// ellipsis when anything was cut: the end of a line being typed is the
    /// part worth seeing.
    pub fn tail(&self, width: usize) -> Self {
        if self.width() <= width {
            return self.clone();
        }
        let mut left = width.saturating_sub(1);
        let mut spans = Vec::new();
        for span in self.spans.iter().rev() {
            let mut kept = Vec::new();
            for ch in span.text.chars().rev() {
                let w = ch.width().unwrap_or(0);
                if w > left {
                    left = 0;
                    break;
                }
                left -= w;
                kept.push(ch);
            }
            if !kept.is_empty() {
                let text = kept.into_iter().rev().collect();
                spans.push(Span {
                    text,
                    ..span.clone()
                });
            }
            if left == 0 {
                break;
            }
        }
        spans.push(Span::new("…", Tone::Muted));
        spans.reverse();
        Line { spans }
    }

    /// The line with SGR sequences for its styles, ending with the
    /// attributes reset if it set any.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        let mut styled = false;
        for span in &self.spans {
            if span.is_plain() {
                if styled {
                    out.push_str("\x1b[0m");
                    styled = false;
                }
            } else {
                out.push_str(&format!("\x1b[{}m", span.sgr()));
                styled = true;
            }
            out.push_str(&span.text);
        }
        if styled {
            out.push_str("\x1b[0m");
        }
        out
    }
}

/// `text` made safe to print: tabs expanded to the next multiple of four
/// columns, other control characters dropped, so no source text can move
/// the cursor or change modes.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut column = 0;
    for ch in text.chars() {
        if ch == '\t' {
            let pad = 4 - column % 4;
            out.extend(std::iter::repeat_n(' ', pad));
            column += pad;
        } else if !ch.is_control() {
            out.push(ch);
            column += ch.width().unwrap_or(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_keeps_whole_wide_characters_and_drops_trailing_blanks() {
        let line = Line::styled("ab漢字 x", Tone::Code);
        assert_eq!(line.clip(5).text(), "ab漢");
        assert_eq!(line.clip(7).text(), "ab漢字");
        assert_eq!(line.clip(40), line);
    }

    #[test]
    fn test_tail_shows_the_end_behind_an_ellipsis() {
        let line = Line::new()
            .push("hello ", Tone::Plain)
            .push("world", Tone::Code);
        assert_eq!(line.tail(20), line);
        let tail = line.tail(6);
        assert_eq!(tail.text(), "…world");
        assert_eq!(tail.width(), 6);
        assert_eq!(tail.spans.last().unwrap().tone, Tone::Code);
    }

    #[test]
    fn test_to_ansi_resets_after_styled_spans_only() {
        assert_eq!(Line::styled("plain", Tone::Plain).to_ansi(), "plain");
        let line = Line::new()
            .push("a", Tone::Plain)
            .push("b", Tone::Accent)
            .bold()
            .push("c", Tone::Plain)
            .push("d", Tone::Muted);
        assert_eq!(line.to_ansi(), "a\x1b[0;1;36mb\x1b[0mc\x1b[0;90md\x1b[0m");
    }

    #[test]
    fn test_trim_end_drops_whitespace_across_spans() {
        let line = Line::new()
            .push("text  ", Tone::Plain)
            .push("   ", Tone::Code)
            .trim_end();
        assert_eq!(line, Line::styled("text", Tone::Plain));
    }

    #[test]
    fn test_sanitize_expands_tabs_and_drops_escapes() {
        assert_eq!(sanitize("a\tb\x1b[2Jc"), "a   b[2Jc");
        assert_eq!(sanitize("漢\tx"), "漢  x");
    }
}
