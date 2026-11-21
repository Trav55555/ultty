//! Vim text-object range resolution (`iw`, `aw`, `i"`, `a(`, `ip`, `is`, etc.).
//!
//! Computes half-open character index ranges `(start, end)` for text-object
//! [`VimMotion`] variants on a slice of `char`s.

use super::VimMotion;

/// Resolve a text-object [`VimMotion`] into a half-open char-index range
/// `(start, end)` within `chars`, relative to `cursor`.
///
/// Returns `None` when `motion` is not a text object, `chars` is empty, or no
/// matching object exists around (or ahead of) `cursor`.
pub fn resolve_text_object(
    chars: &[char],
    cursor: usize,
    motion: &VimMotion,
    count: usize,
) -> Option<(usize, usize)> {
    if chars.is_empty() {
        return None;
    }
    let count = count.max(1);
    let pos = cursor.min(chars.len() - 1);

    match *motion {
        VimMotion::InnerWord => resolve_word(chars, pos, false, false, count),
        VimMotion::AWord => resolve_word(chars, pos, true, false, count),
        VimMotion::InnerWordBig => resolve_word(chars, pos, false, true, count),
        VimMotion::AWordBig => resolve_word(chars, pos, true, true, count),
        VimMotion::InnerSentence => resolve_sentence(chars, pos, false, count),
        VimMotion::ASentence => resolve_sentence(chars, pos, true, count),
        VimMotion::InnerParagraph => resolve_paragraph(chars, pos, false, count),
        VimMotion::AParagraph => resolve_paragraph(chars, pos, true, count),
        VimMotion::InnerBlock(delim) => resolve_block(chars, pos, delim, false, count),
        VimMotion::ABlock(delim) => resolve_block(chars, pos, delim, true, count),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Punct,
    Space,
    Newline,
}

fn char_class(c: char, big: bool) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if big || c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn run_bounds(chars: &[char], pos: usize, big: bool) -> (usize, usize, CharClass) {
    let cls = char_class(chars[pos], big);
    let mut start = pos;
    while start > 0 && char_class(chars[start - 1], big) == cls {
        start -= 1;
    }
    let mut end = pos + 1;
    while end < chars.len() && char_class(chars[end], big) == cls {
        end += 1;
    }
    (start, end, cls)
}

fn resolve_word(
    chars: &[char],
    pos: usize,
    around: bool,
    big: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let (mut start, mut end, cls) = run_bounds(chars, pos, big);
    if cls == CharClass::Newline {
        return None;
    }

    if !around {
        for _ in 1..count {
            if end >= chars.len() || char_class(chars[end], big) == CharClass::Newline {
                break;
            }
            let (_, next_end, _) = run_bounds(chars, end, big);
            end = next_end;
        }
        return Some((start, end));
    }

    if cls == CharClass::Space {
        if end < chars.len() && char_class(chars[end], big) != CharClass::Newline {
            let (_, word_end, _) = run_bounds(chars, end, big);
            end = word_end;
        }
    } else if end < chars.len() && char_class(chars[end], big) == CharClass::Space {
        let (_, space_end, _) = run_bounds(chars, end, big);
        end = space_end;
    } else if start > 0 && char_class(chars[start - 1], big) == CharClass::Space {
        let (space_start, _, _) = run_bounds(chars, start - 1, big);
        start = space_start;
    }

    for _ in 1..count {
        if end >= chars.len() || char_class(chars[end], big) == CharClass::Newline {
            break;
        }
        let (_, next_end, next_cls) = run_bounds(chars, end, big);
        end = next_end;
        if next_cls != CharClass::Space
            && end < chars.len()
            && char_class(chars[end], big) == CharClass::Space
        {
            let (_, space_end, _) = run_bounds(chars, end, big);
            end = space_end;
        }
    }

    Some((start, end))
}

fn line_bounds(chars: &[char], pos: usize) -> (usize, usize) {
    let start = chars[..pos]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1);
    let end = chars[pos..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |i| pos + i);
    (start, end)
}

fn resolve_block(
    chars: &[char],
    pos: usize,
    delim: char,
    around: bool,
    count: usize,
) -> Option<(usize, usize)> {
    match delim {
        '"' | '\'' | '`' => resolve_quote(chars, pos, delim, around),
        '(' | ')' | 'b' => resolve_pair(chars, pos, '(', ')', around, count),
        '[' | ']' => resolve_pair(chars, pos, '[', ']', around, count),
        '{' | '}' | 'B' => resolve_pair(chars, pos, '{', '}', around, count),
        '<' | '>' => resolve_pair(chars, pos, '<', '>', around, count),
        _ => None,
    }
}

fn is_escaped(chars: &[char], idx: usize, line_start: usize) -> bool {
    let mut backslashes = 0usize;
    let mut i = idx;
    while i > line_start {
        i -= 1;
        if chars[i] == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn resolve_quote(chars: &[char], pos: usize, quote: char, around: bool) -> Option<(usize, usize)> {
    let (line_start, line_end) = line_bounds(chars, pos);
    let allow_escape = quote != '`';
    let mut quotes = Vec::new();
    for (idx, &ch) in chars.iter().enumerate().take(line_end).skip(line_start) {
        if ch == quote && (!allow_escape || !is_escaped(chars, idx, line_start)) {
            quotes.push(idx);
        }
    }

    let mut pairs = quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[open, close]| (open, close));
    let (open, close) = pairs
        .clone()
        .find(|&(open, close)| open <= pos && pos <= close)
        .or_else(|| pairs.find(|&(open, _)| open > pos))?;

    if around {
        Some((open, close + 1))
    } else {
        Some((open + 1, close))
    }
}

fn find_matching_close(chars: &[char], open_idx: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    for (idx, &ch) in chars.iter().enumerate().skip(open_idx + 1) {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn find_matching_open(chars: &[char], close_idx: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    let mut idx = close_idx;
    while idx > 0 {
        idx -= 1;
        if chars[idx] == close {
            depth += 1;
        } else if chars[idx] == open {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn find_enclosing_pair(
    chars: &[char],
    from: usize,
    open: char,
    close: char,
) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut idx = from;
    while idx > 0 {
        idx -= 1;
        if chars[idx] == close {
            depth += 1;
        } else if chars[idx] == open {
            if depth == 0 {
                let close_idx = find_matching_close(chars, idx, open, close)?;
                if close_idx >= from {
                    return Some((idx, close_idx));
                }
            } else {
                depth -= 1;
            }
        }
    }
    None
}

fn resolve_pair(
    chars: &[char],
    pos: usize,
    open: char,
    close: char,
    around: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let mut current = if chars[pos] == open {
        find_matching_close(chars, pos, open, close).map(|c| (pos, c))
    } else if chars[pos] == close {
        find_matching_open(chars, pos, open, close).map(|o| (o, pos))
    } else {
        find_enclosing_pair(chars, pos, open, close)
    };

    if current.is_none() {
        let (_, line_end) = line_bounds(chars, pos);
        for (idx, &ch) in chars.iter().enumerate().take(line_end).skip(pos + 1) {
            if ch == open {
                if let Some(close_idx) = find_matching_close(chars, idx, open, close) {
                    current = Some((idx, close_idx));
                    break;
                }
            }
        }
    }

    let mut pair = current?;
    for _ in 1..count {
        if let Some(outer) = find_enclosing_pair(chars, pair.0, open, close) {
            pair = outer;
        } else {
            break;
        }
    }

    if around {
        Some((pair.0, pair.1 + 1))
    } else {
        Some((pair.0 + 1, pair.1))
    }
}

struct LineSpan {
    start: usize,
    end_incl_nl: usize,
    is_blank: bool,
}

fn split_line_spans(chars: &[char]) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (idx, &ch) in chars.iter().enumerate() {
        if ch == '\n' {
            let is_blank = chars[start..idx].iter().all(|c| c.is_whitespace());
            lines.push(LineSpan {
                start,
                end_incl_nl: idx + 1,
                is_blank,
            });
            start = idx + 1;
        }
    }
    if start < chars.len() {
        let is_blank = chars[start..].iter().all(|c| c.is_whitespace());
        lines.push(LineSpan {
            start,
            end_incl_nl: chars.len(),
            is_blank,
        });
    }
    lines
}

fn resolve_paragraph(
    chars: &[char],
    pos: usize,
    around: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let lines = split_line_spans(chars);
    if lines.is_empty() {
        return None;
    }

    let cur = lines
        .iter()
        .position(|line| pos >= line.start && pos < line.end_incl_nl)
        .unwrap_or(lines.len() - 1);

    let cur_blank = lines[cur].is_blank;
    let mut first = cur;
    while first > 0 && lines[first - 1].is_blank == cur_blank {
        first -= 1;
    }
    let mut last = cur;
    while last + 1 < lines.len() && lines[last + 1].is_blank == cur_blank {
        last += 1;
    }

    if !around {
        for _ in 1..count {
            if last + 1 >= lines.len() {
                break;
            }
            let next_blank = lines[last + 1].is_blank;
            last += 1;
            while last + 1 < lines.len() && lines[last + 1].is_blank == next_blank {
                last += 1;
            }
        }
        return Some((lines[first].start, lines[last].end_incl_nl));
    }

    if cur_blank {
        if last + 1 < lines.len() {
            last += 1;
            while last + 1 < lines.len() && !lines[last + 1].is_blank {
                last += 1;
            }
        }
    } else if last + 1 < lines.len() && lines[last + 1].is_blank {
        while last + 1 < lines.len() && lines[last + 1].is_blank {
            last += 1;
        }
    } else if first > 0 && lines[first - 1].is_blank {
        while first > 0 && lines[first - 1].is_blank {
            first -= 1;
        }
    }

    for _ in 1..count {
        if last + 1 >= lines.len() {
            break;
        }
        let next_blank = lines[last + 1].is_blank;
        last += 1;
        while last + 1 < lines.len() && lines[last + 1].is_blank == next_blank {
            last += 1;
        }
        if !next_blank && last + 1 < lines.len() && lines[last + 1].is_blank {
            while last + 1 < lines.len() && lines[last + 1].is_blank {
                last += 1;
            }
        }
    }

    Some((lines[first].start, lines[last].end_incl_nl))
}

struct SentenceSpan {
    start: usize,
    end: usize,
    ws_end: usize,
}

fn collect_sentences(chars: &[char]) -> Vec<SentenceSpan> {
    let len = chars.len();
    let mut sentences = Vec::new();
    let mut idx = 0usize;

    while idx < len {
        while idx < len && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= len {
            break;
        }

        let start = idx;
        let mut end = len;
        while idx < len {
            let ch = chars[idx];
            if matches!(ch, '.' | '!' | '?') {
                let mut after = idx + 1;
                while after < len && matches!(chars[after], ')' | ']' | '"' | '\'') {
                    after += 1;
                }
                if after == len || chars[after].is_whitespace() {
                    end = after;
                    break;
                }
            }
            if ch == '\n' && idx + 1 < len && chars[idx + 1] == '\n' {
                end = idx;
                break;
            }
            idx += 1;
        }

        let mut ws_end = end;
        while ws_end < len && chars[ws_end].is_whitespace() && chars[ws_end] != '\n' {
            ws_end += 1;
        }
        if ws_end < len && chars[ws_end] == '\n' && (ws_end + 1 >= len || chars[ws_end + 1] != '\n')
        {
            ws_end += 1;
        }

        sentences.push(SentenceSpan { start, end, ws_end });
        idx = ws_end;
    }

    sentences
}

fn resolve_sentence(
    chars: &[char],
    pos: usize,
    around: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let sentences = collect_sentences(chars);
    if sentences.is_empty() {
        return None;
    }

    let idx = sentences
        .iter()
        .position(|s| pos >= s.start && pos < s.ws_end)
        .or_else(|| {
            sentences
                .iter()
                .position(|s| pos < s.start)
                .map(|next| next.saturating_sub(1))
        })
        .unwrap_or(sentences.len() - 1);

    let last = (idx + count - 1).min(sentences.len() - 1);
    let first_s = &sentences[idx];
    let last_s = &sentences[last];

    if !around {
        if pos >= first_s.end && pos < first_s.ws_end {
            return Some((first_s.end, first_s.ws_end));
        }
        return Some((first_s.start, last_s.end));
    }

    if last_s.ws_end > last_s.end {
        Some((first_s.start, last_s.ws_end))
    } else if idx > 0 && sentences[idx - 1].ws_end > sentences[idx - 1].end {
        Some((sentences[idx - 1].end, last_s.end))
    } else if first_s.start > 0 {
        let mut lead = first_s.start;
        while lead > 0 && chars[lead - 1].is_whitespace() && chars[lead - 1] != '\n' {
            lead -= 1;
        }
        Some((lead, last_s.end))
    } else {
        Some((first_s.start, last_s.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, cursor: usize, motion: VimMotion) -> Option<&str> {
        let chars: Vec<char> = text.chars().collect();
        let (s, e) = resolve_text_object(&chars, cursor, &motion, 1)?;
        let byte_start = text.char_indices().nth(s).map_or(text.len(), |(i, _)| i);
        let byte_end = text.char_indices().nth(e).map_or(text.len(), |(i, _)| i);
        Some(&text[byte_start..byte_end])
    }

    fn slice_count(text: &str, cursor: usize, motion: VimMotion, count: usize) -> Option<&str> {
        let chars: Vec<char> = text.chars().collect();
        let (s, e) = resolve_text_object(&chars, cursor, &motion, count)?;
        let byte_start = text.char_indices().nth(s).map_or(text.len(), |(i, _)| i);
        let byte_end = text.char_indices().nth(e).map_or(text.len(), |(i, _)| i);
        Some(&text[byte_start..byte_end])
    }

    #[test]
    fn test_inner_and_around_word() {
        assert_eq!(slice("hello world", 1, VimMotion::InnerWord), Some("hello"));
        assert_eq!(slice("hello world", 1, VimMotion::AWord), Some("hello "));
        assert_eq!(slice("hello world", 8, VimMotion::AWord), Some(" world"));
        assert_eq!(
            slice("foo_bar baz", 3, VimMotion::InnerWord),
            Some("foo_bar")
        );
        assert_eq!(slice("foo::bar", 3, VimMotion::InnerWord), Some("::"));
        assert_eq!(slice("a   b", 2, VimMotion::InnerWord), Some("   "));
        assert_eq!(slice("a   b", 2, VimMotion::AWord), Some("   b"));
    }

    #[test]
    fn test_inner_and_around_big_word() {
        assert_eq!(
            slice("foo::bar baz", 2, VimMotion::InnerWordBig),
            Some("foo::bar")
        );
        assert_eq!(
            slice("foo::bar baz", 2, VimMotion::AWordBig),
            Some("foo::bar ")
        );
        assert_eq!(slice("foo::bar baz", 10, VimMotion::AWordBig), Some(" baz"));
    }

    #[test]
    fn test_quotes_and_escaped_quotes() {
        let text = r#"say "he\"llo" world"#;
        assert_eq!(
            slice(text, 6, VimMotion::InnerBlock('"')),
            Some(r#"he\"llo"#)
        );
        assert_eq!(slice(text, 6, VimMotion::ABlock('"')), Some(r#""he\"llo""#));
        assert_eq!(
            slice(text, 0, VimMotion::InnerBlock('"')),
            Some(r#"he\"llo"#)
        );
        assert_eq!(
            slice("a 'bc' d", 3, VimMotion::InnerBlock('\'')),
            Some("bc")
        );
        assert_eq!(
            slice("a `code` d", 4, VimMotion::ABlock('`')),
            Some("`code`")
        );
        assert_eq!(slice("empty \"\"", 7, VimMotion::InnerBlock('"')), Some(""));
        assert_eq!(slice("no quotes here", 2, VimMotion::InnerBlock('"')), None);
    }

    #[test]
    fn test_nested_delimiters_and_counts() {
        let text = "fn((a + b) * [c, {d < e}])";
        assert_eq!(slice(text, 4, VimMotion::InnerBlock('(')), Some("a + b"));
        assert_eq!(slice(text, 4, VimMotion::ABlock('(')), Some("(a + b)"));
        assert_eq!(
            slice_count(text, 4, VimMotion::InnerBlock('('), 2),
            Some("(a + b) * [c, {d < e}]")
        );
        assert_eq!(
            slice(text, 14, VimMotion::InnerBlock('[')),
            Some("c, {d < e}")
        );
        assert_eq!(slice(text, 18, VimMotion::InnerBlock('{')), Some("d < e"));
        assert_eq!(
            slice(text, 0, VimMotion::InnerBlock('(')),
            Some("(a + b) * [c, {d < e}]")
        );
        assert_eq!(slice("vec<T>", 4, VimMotion::InnerBlock('<')), Some("T"));
        assert_eq!(slice("vec<T>", 4, VimMotion::ABlock('>')), Some("<T>"));
    }

    #[test]
    fn test_paragraphs() {
        let text = "line 1\nline 2\n\nline 3";
        assert_eq!(
            slice(text, 2, VimMotion::InnerParagraph),
            Some("line 1\nline 2\n")
        );
        assert_eq!(
            slice(text, 2, VimMotion::AParagraph),
            Some("line 1\nline 2\n\n")
        );
        assert_eq!(slice(text, 17, VimMotion::InnerParagraph), Some("line 3"));
        assert_eq!(slice(text, 17, VimMotion::AParagraph), Some("\nline 3"));
    }

    #[test]
    fn test_sentences() {
        let text = "First sentence. Second sentence! Third?";
        assert_eq!(
            slice(text, 20, VimMotion::InnerSentence),
            Some("Second sentence!")
        );
        assert_eq!(
            slice(text, 20, VimMotion::ASentence),
            Some("Second sentence! ")
        );
        assert_eq!(slice(text, 35, VimMotion::InnerSentence), Some("Third?"));
        assert_eq!(slice(text, 35, VimMotion::ASentence), Some(" Third?"));
    }
}
