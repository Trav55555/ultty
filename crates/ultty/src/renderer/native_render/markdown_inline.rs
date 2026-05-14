//! Native markdown inline span planning.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkdownInlineColor {
    Line,
    Code,
    Link,
    Dim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownInlineSpan {
    pub(super) text: String,
    pub(super) color: MarkdownInlineColor,
    pub(super) bold: bool,
    pub(super) italic: bool,
}

pub(super) fn plan_markdown_inline_spans(text: &str, line_bold: bool) -> Vec<MarkdownInlineSpan> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut spans = Vec::new();
    let mut i = 0;
    let mut segment = String::new();

    while i < len {
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            flush_line_segment(&mut spans, &mut segment, line_bold);
            i += 2;

            let mut bold_text = String::new();
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold_text.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            }
            spans.push(span(bold_text, MarkdownInlineColor::Line, true, false));
            continue;
        }

        if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
            flush_line_segment(&mut spans, &mut segment, line_bold);
            i += 1;

            let mut italic_text = String::new();
            while i < len && chars[i] != '*' {
                italic_text.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            spans.push(span(
                italic_text,
                MarkdownInlineColor::Line,
                line_bold,
                true,
            ));
            continue;
        }

        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            flush_line_segment(&mut spans, &mut segment, line_bold);
            i += 2;

            let mut strike_text = String::new();
            while i + 1 < len && !(chars[i] == '~' && chars[i + 1] == '~') {
                strike_text.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            }
            spans.push(span(strike_text, MarkdownInlineColor::Dim, false, false));
            continue;
        }

        if chars[i] == '`' {
            flush_line_segment(&mut spans, &mut segment, line_bold);
            i += 1;

            let mut code_text = String::new();
            while i < len && chars[i] != '`' {
                code_text.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            spans.push(span(code_text, MarkdownInlineColor::Code, false, false));
            continue;
        }

        if chars[i] == '[' {
            let mut link_text = String::new();
            let mut j = i + 1;
            while j < len && chars[j] != ']' {
                link_text.push(chars[j]);
                j += 1;
            }
            if j + 1 < len && chars[j] == ']' && chars[j + 1] == '(' {
                j += 2;
                while j < len && chars[j] != ')' {
                    j += 1;
                }
                if j < len {
                    j += 1;
                }

                flush_line_segment(&mut spans, &mut segment, line_bold);
                spans.push(span(link_text, MarkdownInlineColor::Link, false, false));
                i = j;
                continue;
            }
        }

        segment.push(chars[i]);
        i += 1;
    }

    flush_line_segment(&mut spans, &mut segment, line_bold);
    spans
}

fn flush_line_segment(spans: &mut Vec<MarkdownInlineSpan>, segment: &mut String, line_bold: bool) {
    if segment.is_empty() {
        return;
    }

    spans.push(span(
        std::mem::take(segment),
        MarkdownInlineColor::Line,
        line_bold,
        false,
    ));
}

fn span(text: String, color: MarkdownInlineColor, bold: bool, italic: bool) -> MarkdownInlineSpan {
    MarkdownInlineSpan {
        text,
        color,
        bold,
        italic,
    }
}

#[cfg(test)]
mod tests {
    use super::{plan_markdown_inline_spans, MarkdownInlineColor};

    #[test]
    fn plans_representative_inline_spans() {
        let spans = plan_markdown_inline_spans(
            "plain **bold** *italic* `code` [link](url) ~~strike~~",
            false,
        );

        assert_eq!(spans[0].text, "plain ");
        assert_eq!(spans[0].color, MarkdownInlineColor::Line);
        assert_eq!(spans[1].text, "bold");
        assert!(spans[1].bold);
        assert_eq!(spans[3].text, "italic");
        assert!(spans[3].italic);
        assert_eq!(spans[5].text, "code");
        assert_eq!(spans[5].color, MarkdownInlineColor::Code);
        assert_eq!(spans[7].text, "link");
        assert_eq!(spans[7].color, MarkdownInlineColor::Link);
        assert_eq!(spans[9].text, "strike");
        assert_eq!(spans[9].color, MarkdownInlineColor::Dim);
    }

    #[test]
    fn line_bold_applies_to_plain_and_italic_line_spans() {
        let spans = plan_markdown_inline_spans("plain *em*", true);

        assert!(spans[0].bold);
        assert_eq!(spans[1].text, "em");
        assert!(spans[1].bold);
        assert!(spans[1].italic);
    }

    #[test]
    fn unmatched_link_marker_stays_plain() {
        let spans = plan_markdown_inline_spans("see [broken", false);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "see [broken");
        assert_eq!(spans[0].color, MarkdownInlineColor::Line);
    }

    #[test]
    fn unclosed_code_marker_matches_existing_consuming_behavior() {
        let spans = plan_markdown_inline_spans("a `code", false);

        assert_eq!(spans[0].text, "a ");
        assert_eq!(spans[1].text, "code");
        assert_eq!(spans[1].color, MarkdownInlineColor::Code);
    }
}
