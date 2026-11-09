//! Line breaking of region text, shared by the GPU renderer and the inline
//! surface rasteriser (ADR 0020) so a card's cells break where its glyphs
//! did.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Word-wrap a line to fit within `max_cols` columns.
/// Breaks at spaces when possible, falls back to hard character breaks.
pub(crate) fn wrap_line(line: &str, max_cols: usize) -> Vec<String> {
    fn skip_spaces(line: &str, mut byte_idx: usize) -> usize {
        while byte_idx < line.len() {
            match line[byte_idx..].chars().next() {
                Some(' ') => byte_idx += ' '.len_utf8(),
                _ => break,
            }
        }
        byte_idx
    }

    if max_cols == 0 {
        return vec![line.to_string()];
    }

    if line.width() <= max_cols {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut row_start = 0usize;

    while row_start < line.len() {
        let mut row_end = row_start;
        let mut used_cols = 0usize;
        let mut break_at = None;

        for (rel_idx, ch) in line[row_start..].char_indices() {
            let ch_start = row_start + rel_idx;
            let ch_width = ch.width().unwrap_or(0);
            if used_cols + ch_width > max_cols {
                break;
            }
            used_cols += ch_width;
            if ch == ' ' {
                break_at = Some(ch_start);
            }
            row_end = ch_start + ch.len_utf8();
        }

        if row_end == line.len() {
            result.push(line[row_start..].to_string());
            break;
        }

        if row_end == row_start {
            let ch = line[row_start..]
                .chars()
                .next()
                .expect("row_start always within bounds");
            let next = row_start + ch.len_utf8();
            result.push(line[row_start..next].to_string());
            row_start = skip_spaces(line, next);
            continue;
        }

        match break_at {
            Some(space_idx) => {
                result.push(line[row_start..space_idx].to_string());
                row_start = skip_spaces(line, space_idx + 1);
            }
            None => {
                result.push(line[row_start..row_end].to_string());
                row_start = skip_spaces(line, row_end);
            }
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Where the next piece of rich text that fits in `cols_left` columns ends,
/// starting at `byte_pos` of `segment`: at least one character, and never
/// splitting a character from the zero-width ones and joiners after it.
pub(crate) fn next_display_width_chunk_end(
    segment: &str,
    byte_pos: usize,
    cols_left: usize,
) -> Option<usize> {
    if cols_left == 0 || byte_pos >= segment.len() {
        return None;
    }

    let mut chunk_end = byte_pos;
    let mut used_cols = 0usize;
    let mut previous_was_joiner = false;

    for (offset, ch) in segment[byte_pos..].char_indices() {
        let ch_end = byte_pos + offset + ch.len_utf8();
        let ch_width = ch.width().unwrap_or(0);
        let force_include = chunk_end == byte_pos
            || previous_was_joiner
            || ch_width == 0
            || ch == '\u{200d}'
            || is_variation_selector(ch);

        if used_cols + ch_width > cols_left && !force_include {
            break;
        }

        used_cols += ch_width;
        chunk_end = ch_end;
        previous_was_joiner = ch == '\u{200d}';
    }

    (chunk_end > byte_pos).then_some(chunk_end)
}

fn is_variation_selector(ch: char) -> bool {
    matches!(ch, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_line_short_text_unchanged() {
        assert_eq!(wrap_line("hello", 10), vec!["hello"]);
    }

    #[test]
    fn wrap_line_exact_fit() {
        assert_eq!(wrap_line("hello", 5), vec!["hello"]);
    }

    #[test]
    fn wrap_line_breaks_at_space() {
        assert_eq!(wrap_line("hello world", 7), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_line_hard_break_no_space() {
        assert_eq!(wrap_line("abcdefghij", 5), vec!["abcde", "fghij"]);
    }

    #[test]
    fn wrap_line_multiple_wraps() {
        assert_eq!(
            wrap_line("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn wrap_line_zero_cols_returns_original() {
        assert_eq!(wrap_line("hello", 0), vec!["hello"]);
    }

    #[test]
    fn wrap_line_empty_string() {
        assert_eq!(wrap_line("", 10), vec![""]);
    }

    #[test]
    fn wrap_line_single_long_word_then_short() {
        assert_eq!(
            wrap_line("abcdefghij end", 5),
            vec!["abcde", "fghij", "end"]
        );
    }

    #[test]
    fn wrap_line_cjk_uses_display_width() {
        assert_eq!(wrap_line("你好世界", 5), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_line_mixed_ascii_cjk_exact_fit() {
        assert_eq!(wrap_line("hi你好", 6), vec!["hi你好"]);
    }

    #[test]
    fn wrap_line_emoji_uses_display_width() {
        assert_eq!(wrap_line("🙂🙂🙂", 4), vec!["🙂🙂", "🙂"]);
    }

    #[test]
    fn rich_text_chunk_uses_cjk_display_width() {
        let text = "你好世界";
        let end = next_display_width_chunk_end(text, 0, 5).unwrap();

        assert_eq!(&text[..end], "你好");
    }

    #[test]
    fn rich_text_chunk_uses_emoji_display_width() {
        let text = "🙂🙂🙂";
        let end = next_display_width_chunk_end(text, 0, 4).unwrap();

        assert_eq!(&text[..end], "🙂🙂");
    }

    #[test]
    fn rich_text_chunk_keeps_joined_emoji_together() {
        let family = "👨‍👩‍👧‍👦";
        let text = format!("{family}x");
        let end = next_display_width_chunk_end(&text, 0, 2).unwrap();

        assert_eq!(&text[..end], family);
    }

    #[test]
    fn rich_text_chunk_respects_remaining_columns_after_span_boundary() {
        let text = "a你b";
        let first = next_display_width_chunk_end(text, 0, 3).unwrap();

        assert_eq!(&text[..first], "a你");

        let second = next_display_width_chunk_end(text, first, 1).unwrap();
        assert_eq!(&text[first..second], "b");
    }
}
