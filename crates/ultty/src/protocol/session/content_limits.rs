//! Bounds on the text a session keeps in its scene.
//!
//! Each message is size-limited by the codec, but `AppendContent` adds up:
//! without these bounds an app (or, in-band, a `cat`ed file or a remote
//! host) could grow the terminal's memory without end. A region's text is
//! a ring like scrollback, and a session's text as a whole has a ceiling
//! (spec §8.2).

use ultty_protocol::{Content, WidgetContent};

use super::NativeSession;

/// A region whose text passes this many bytes drops its oldest text.
pub(super) const MAX_REGION_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// What a trimmed region keeps: dropping a quarter at a time means the
/// text is moved once per megabyte appended, not on every append.
pub(super) const TRIMMED_REGION_TEXT_BYTES: usize = MAX_REGION_TEXT_BYTES / 4 * 3;

/// Text across all of a session's regions. `SetContent` or `AppendContent`
/// that would pass it is refused with a protocol error.
pub(super) const MAX_SESSION_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// How far past the cut a trim looks for a line break to cut after instead.
const LINE_SEARCH_BYTES: usize = 64 * 1024;

/// Bytes of text `content` holds: what the session limit counts.
pub(super) fn text_bytes(content: &Content) -> usize {
    match content {
        Content::None => 0,
        Content::Text(text) => text.text.len(),
        Content::RichText(rich) => rich.spans.iter().map(|span| span.text.len()).sum(),
        Content::Widget(WidgetContent::Markdown(md)) => md.source.len(),
        Content::Widget(WidgetContent::Code(code)) => code.source.len(),
        Content::Widget(WidgetContent::Input(input)) => input.value.len() + input.placeholder.len(),
        Content::Widget(_) => 0,
    }
}

/// Drop the oldest text of an appendable region once it passes
/// [`MAX_REGION_TEXT_BYTES`], keeping about [`TRIMMED_REGION_TEXT_BYTES`].
/// Content that cannot be appended to is left alone.
pub(super) fn trim_region_text(content: &mut Content) {
    match content {
        Content::Text(text) => trim_string(&mut text.text),
        Content::Widget(WidgetContent::Markdown(md)) => trim_string(&mut md.source),
        Content::RichText(rich) => {
            let total: usize = rich.spans.iter().map(|span| span.text.len()).sum();
            if total <= MAX_REGION_TEXT_BYTES {
                return;
            }
            let mut excess = total - TRIMMED_REGION_TEXT_BYTES;
            let whole = rich
                .spans
                .iter()
                .take_while(|span| {
                    let drop = span.text.len() <= excess;
                    if drop {
                        excess -= span.text.len();
                    }
                    drop
                })
                .count();
            rich.spans.drain(..whole);
            if let Some(first) = rich.spans.first_mut() {
                let keep = first.text.len() - excess;
                drop_front(&mut first.text, keep);
            }
        }
        _ => {}
    }
}

fn trim_string(text: &mut String) {
    if text.len() > MAX_REGION_TEXT_BYTES {
        drop_front(text, TRIMMED_REGION_TEXT_BYTES);
    }
}

/// Keep at most the last `keep` bytes of `text`, starting on a character
/// boundary and, when a line break is near, on a line.
fn drop_front(text: &mut String, keep: usize) {
    let Some(mut cut) = text.len().checked_sub(keep) else {
        return;
    };
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    let search_end = (cut + LINE_SEARCH_BYTES).min(text.len());
    if let Some(newline) = text.as_bytes()[cut..search_end]
        .iter()
        .position(|&b| b == b'\n')
    {
        cut += newline + 1;
    }
    text.drain(..cut);
}

impl NativeSession {
    /// Whether the scene may take `added` more bytes of text without
    /// passing [`MAX_SESSION_TEXT_BYTES`]; if so, they are counted.
    ///
    /// The session keeps an upper bound on its text instead of an exact
    /// count, since removing and replacing content only lowers it. When the
    /// bound would pass the limit, the scene is counted again.
    pub(super) fn admit_text(&mut self, added: usize) -> bool {
        if self.text_bytes_bound + added > MAX_SESSION_TEXT_BYTES {
            self.text_bytes_bound = self
                .scene
                .iter_depth_first()
                .map(|region| text_bytes(&region.content))
                .sum();
        }
        if self.text_bytes_bound + added > MAX_SESSION_TEXT_BYTES {
            return false;
        }
        self.text_bytes_bound += added;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::{RichText, Style, TextContent};

    #[test]
    fn text_under_the_region_limit_is_kept_whole() {
        let mut content = Content::Text(TextContent::new("a".repeat(MAX_REGION_TEXT_BYTES)));
        trim_region_text(&mut content);
        assert_eq!(text_bytes(&content), MAX_REGION_TEXT_BYTES);
    }

    #[test]
    fn text_over_the_region_limit_drops_its_oldest_lines() {
        let line = "0123456789abcde\n";
        let mut text = String::from("OLDEST\n");
        while text.len() <= MAX_REGION_TEXT_BYTES {
            text.push_str(line);
        }
        text.push_str("NEWEST");
        let mut content = Content::Text(TextContent::new(text));

        trim_region_text(&mut content);

        let Content::Text(text) = &content else {
            panic!("content kind changed");
        };
        assert!(text.text.len() <= TRIMMED_REGION_TEXT_BYTES);
        assert!(text.text.len() > TRIMMED_REGION_TEXT_BYTES - line.len());
        assert!(text.text.starts_with(line), "cut on a line boundary");
        assert!(text.text.ends_with("NEWEST"));
        assert!(!text.text.contains("OLDEST"));
    }

    #[test]
    fn trim_without_line_breaks_cuts_on_a_char_boundary() {
        let mut content = Content::Text(TextContent::new("é".repeat(MAX_REGION_TEXT_BYTES)));
        trim_region_text(&mut content);
        let Content::Text(text) = &content else {
            panic!("content kind changed");
        };
        assert!(text.text.len() <= TRIMMED_REGION_TEXT_BYTES);
        assert!(text.text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn rich_text_over_the_limit_drops_whole_and_partial_spans() {
        let big = "x".repeat(MAX_REGION_TEXT_BYTES / 2);
        let mut content = Content::RichText(
            RichText::new()
                .span("first", Style::new())
                .span(big.clone(), Style::new())
                .span(big, Style::new())
                .span("last", Style::new()),
        );

        trim_region_text(&mut content);

        let Content::RichText(rich) = &content else {
            panic!("content kind changed");
        };
        assert_eq!(text_bytes(&content), TRIMMED_REGION_TEXT_BYTES);
        assert_eq!(rich.spans.len(), 3, "the first span is gone");
        assert_eq!(rich.spans.last().unwrap().text, "last");
    }
}
