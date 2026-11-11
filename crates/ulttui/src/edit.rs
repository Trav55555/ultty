//! Single-line text editing on grapheme clusters.
//!
//! The pure functions here are what [`TextInput`](crate::widgets::TextInput)
//! uses for typing in insert mode, and what hosts that keep the text
//! elsewhere (such as `ulttui-lua`, where the script owns the value) call to
//! edit it. A cursor is the number of grapheme clusters before it, so an
//! edit never splits a cluster such as `e` plus a combining accent, or a
//! family emoji.
//!
//! A word is a run of non-whitespace characters, as in readline.

use unicode_segmentation::UnicodeSegmentation;

use crate::event::{Key, KeyEvent};

/// One editing operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// Insert text at the cursor (typing, or a paste).
    Insert(String),
    /// Cursor one grapheme left.
    Left,
    /// Cursor one grapheme right.
    Right,
    /// Cursor to the start of the previous word.
    WordLeft,
    /// Cursor to the end of the next word.
    WordRight,
    /// Cursor to the start.
    Home,
    /// Cursor to the end.
    End,
    /// Delete the grapheme before the cursor.
    Backspace,
    /// Delete the grapheme at the cursor.
    Delete,
    /// Delete back to the start of the previous word.
    DeleteWordBack,
    /// Delete everything before the cursor.
    DeleteToStart,
    /// Delete everything from the cursor on.
    DeleteToEnd,
}

/// The edit a key press asks for, if it is an editing key: printable
/// characters, the arrows, Home, End, Backspace, Delete, Ctrl+A/E/W/U/K,
/// Ctrl/Alt+Left/Right, Alt+B/F and Alt+Backspace. Other keys (Enter,
/// Escape, Up, Down, other Ctrl or Alt combinations, anything with Meta) are
/// not editing keys.
pub fn op_for_key(event: &KeyEvent) -> Option<EditOp> {
    let m = event.modifiers;
    // Super or Meta chords are the host's shortcuts, never text or edits.
    if m.meta {
        return None;
    }
    let word = m.ctrl || m.alt;
    match &event.key {
        Key::Char(c) if m.ctrl && !m.alt => match c.to_ascii_lowercase() {
            'a' => Some(EditOp::Home),
            'e' => Some(EditOp::End),
            'w' => Some(EditOp::DeleteWordBack),
            'u' => Some(EditOp::DeleteToStart),
            'k' => Some(EditOp::DeleteToEnd),
            _ => None,
        },
        Key::Char(c) if m.alt && !m.ctrl => match c.to_ascii_lowercase() {
            'b' => Some(EditOp::WordLeft),
            'f' => Some(EditOp::WordRight),
            _ => None,
        },
        Key::Char(c) if !m.ctrl && !m.alt && !c.is_control() => Some(EditOp::Insert(c.to_string())),
        Key::Left if word => Some(EditOp::WordLeft),
        Key::Right if word => Some(EditOp::WordRight),
        Key::Left => Some(EditOp::Left),
        Key::Right => Some(EditOp::Right),
        Key::Home => Some(EditOp::Home),
        Key::End => Some(EditOp::End),
        Key::Backspace if word => Some(EditOp::DeleteWordBack),
        Key::Backspace => Some(EditOp::Backspace),
        Key::Delete => Some(EditOp::Delete),
        _ => None,
    }
}

/// Number of grapheme clusters in `text`: the largest cursor position.
pub fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

/// Apply `op` to `value` with the cursor at `cursor` (clamped to the text),
/// returning the new value and cursor.
pub fn apply(value: &str, cursor: usize, op: &EditOp) -> (String, usize) {
    let graphemes: Vec<&str> = value.graphemes(true).collect();
    let len = graphemes.len();
    let cursor = cursor.min(len);
    let is_space = |g: &str| g.chars().all(char::is_whitespace);

    let word_left = || {
        let mut at = cursor;
        while at > 0 && is_space(graphemes[at - 1]) {
            at -= 1;
        }
        while at > 0 && !is_space(graphemes[at - 1]) {
            at -= 1;
        }
        at
    };
    let word_right = || {
        let mut at = cursor;
        while at < len && is_space(graphemes[at]) {
            at += 1;
        }
        while at < len && !is_space(graphemes[at]) {
            at += 1;
        }
        at
    };
    // Remove graphemes `from..to` and leave the cursor at `from`.
    let cut = |from: usize, to: usize| {
        let text: String = graphemes[..from]
            .iter()
            .chain(&graphemes[to..])
            .copied()
            .collect();
        (text, from)
    };

    match op {
        EditOp::Insert(text) => {
            let mut out: String = graphemes[..cursor].concat();
            out.push_str(text);
            // The inserted text may fuse with what precedes it (a combining
            // mark), so measure the cursor on the result.
            let at = grapheme_count(&out);
            out.push_str(&graphemes[cursor..].concat());
            (out, at)
        }
        EditOp::Left => (value.to_string(), cursor.saturating_sub(1)),
        EditOp::Right => (value.to_string(), (cursor + 1).min(len)),
        EditOp::WordLeft => (value.to_string(), word_left()),
        EditOp::WordRight => (value.to_string(), word_right()),
        EditOp::Home => (value.to_string(), 0),
        EditOp::End => (value.to_string(), len),
        EditOp::Backspace => cut(cursor.saturating_sub(1), cursor),
        EditOp::Delete => cut(cursor, (cursor + 1).min(len)),
        EditOp::DeleteWordBack => cut(word_left(), cursor),
        EditOp::DeleteToStart => cut(0, cursor),
        EditOp::DeleteToEnd => cut(cursor, len),
    }
}

/// The grapheme cursor for a character index into `value`; an index inside
/// a cluster rounds down to the cluster's start.
pub fn cursor_from_char_index(value: &str, char_index: usize) -> usize {
    let mut chars = 0;
    for (i, g) in value.graphemes(true).enumerate() {
        let next = chars + g.chars().count();
        if next > char_index {
            return i;
        }
        chars = next;
    }
    grapheme_count(value)
}

/// The character index into `value` of a grapheme cursor.
pub fn char_index_from_cursor(value: &str, cursor: usize) -> usize {
    value
        .graphemes(true)
        .take(cursor)
        .map(|g| g.chars().count())
        .sum()
}

/// `value` split at the cursor into (before, grapheme under it, after). At
/// the end of the text there is no grapheme under the cursor.
pub fn split_at_cursor(value: &str, cursor: usize) -> (&str, &str, &str) {
    let mut starts = value.grapheme_indices(true).map(|(i, _)| i);
    let at = starts.nth(cursor).unwrap_or(value.len());
    let next = starts.next().unwrap_or(value.len());
    (&value[..at], &value[at..next], &value[next..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;

    const ACCENTED: &str = "héllo wörld";
    // "e" + combining acute: two chars, one grapheme.
    const COMBINED: &str = "ae\u{301}b";
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";

    fn run(value: &str, cursor: usize, op: EditOp) -> (String, usize) {
        apply(value, cursor, &op)
    }

    fn key(key: Key, ctrl: bool, alt: bool) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                alt,
                ..Modifiers::default()
            },
            focused_id: None,
        }
    }

    #[test]
    fn insert_goes_at_the_cursor_and_moves_it_after_the_text() {
        assert_eq!(
            run("held", 3, EditOp::Insert("l".into())),
            ("helld".into(), 4)
        );
        assert_eq!(run("", 0, EditOp::Insert("é".into())), ("é".into(), 1));
        assert_eq!(run("ab", 99, EditOp::Insert("c".into())), ("abc".into(), 3));
    }

    #[test]
    fn insert_of_several_characters_is_a_paste() {
        assert_eq!(
            run("ac", 1, EditOp::Insert("b\u{1F600}d".into())),
            ("ab\u{1F600}dc".into(), 4)
        );
    }

    #[test]
    fn left_and_right_step_over_whole_graphemes() {
        assert_eq!(run(ACCENTED, 2, EditOp::Left).1, 1);
        assert_eq!(run(ACCENTED, 0, EditOp::Left).1, 0);
        assert_eq!(run(ACCENTED, 10, EditOp::Right).1, 11);
        assert_eq!(run(ACCENTED, 11, EditOp::Right).1, 11);
        assert_eq!(run(COMBINED, 1, EditOp::Right).1, 2);
        assert_eq!(run(&format!("{FAMILY}x"), 0, EditOp::Right).1, 1);
    }

    #[test]
    fn home_and_end() {
        assert_eq!(run(ACCENTED, 5, EditOp::Home).1, 0);
        assert_eq!(run(ACCENTED, 5, EditOp::End).1, 11);
        assert_eq!(run(&format!("a{FAMILY}"), 0, EditOp::End).1, 2);
    }

    #[test]
    fn word_motions_use_whitespace_boundaries() {
        let v = "héllo  wörld  x";
        assert_eq!(run(v, 15, EditOp::WordLeft).1, 14);
        assert_eq!(run(v, 14, EditOp::WordLeft).1, 7);
        assert_eq!(run(v, 7, EditOp::WordLeft).1, 0);
        assert_eq!(run(v, 0, EditOp::WordLeft).1, 0);
        assert_eq!(run(v, 0, EditOp::WordRight).1, 5);
        assert_eq!(run(v, 5, EditOp::WordRight).1, 12);
        assert_eq!(run(v, 12, EditOp::WordRight).1, 15);
        assert_eq!(run(v, 15, EditOp::WordRight).1, 15);
    }

    #[test]
    fn backspace_removes_the_grapheme_before_the_cursor() {
        assert_eq!(
            run(ACCENTED, 2, EditOp::Backspace),
            ("hllo wörld".into(), 1)
        );
        assert_eq!(run(ACCENTED, 0, EditOp::Backspace), (ACCENTED.into(), 0));
        assert_eq!(run(COMBINED, 2, EditOp::Backspace), ("ab".into(), 1));
        assert_eq!(
            run(&format!("a{FAMILY}"), 2, EditOp::Backspace),
            ("a".into(), 1)
        );
    }

    #[test]
    fn delete_removes_the_grapheme_at_the_cursor() {
        assert_eq!(run(ACCENTED, 1, EditOp::Delete), ("hllo wörld".into(), 1));
        assert_eq!(run(ACCENTED, 11, EditOp::Delete), (ACCENTED.into(), 11));
        assert_eq!(run(COMBINED, 1, EditOp::Delete), ("ab".into(), 1));
        assert_eq!(
            run(&format!("{FAMILY}b"), 0, EditOp::Delete),
            ("b".into(), 0)
        );
    }

    #[test]
    fn delete_word_back_removes_trailing_space_and_the_word() {
        assert_eq!(
            run(ACCENTED, 11, EditOp::DeleteWordBack),
            ("héllo ".into(), 6)
        );
        assert_eq!(run("héllo   ", 8, EditOp::DeleteWordBack), ("".into(), 0));
        assert_eq!(
            run("héllo wörld", 8, EditOp::DeleteWordBack),
            ("héllo rld".into(), 6)
        );
        assert_eq!(run("abc", 0, EditOp::DeleteWordBack), ("abc".into(), 0));
    }

    #[test]
    fn delete_to_start_and_end_split_at_the_cursor() {
        assert_eq!(run(ACCENTED, 6, EditOp::DeleteToStart), ("wörld".into(), 0));
        assert_eq!(run(ACCENTED, 5, EditOp::DeleteToEnd), ("héllo".into(), 5));
        assert_eq!(
            run(ACCENTED, 0, EditOp::DeleteToStart),
            (ACCENTED.into(), 0)
        );
    }

    #[test]
    fn an_out_of_range_cursor_is_clamped() {
        assert_eq!(run("ab", 50, EditOp::Backspace), ("a".into(), 1));
        assert_eq!(run("ab", 50, EditOp::Left).1, 1);
    }

    #[test]
    fn keys_map_to_operations() {
        let plain = |k| op_for_key(&key(k, false, false));
        assert_eq!(plain(Key::Char('é')), Some(EditOp::Insert("é".into())));
        assert_eq!(plain(Key::Left), Some(EditOp::Left));
        assert_eq!(plain(Key::Right), Some(EditOp::Right));
        assert_eq!(plain(Key::Home), Some(EditOp::Home));
        assert_eq!(plain(Key::End), Some(EditOp::End));
        assert_eq!(plain(Key::Backspace), Some(EditOp::Backspace));
        assert_eq!(plain(Key::Delete), Some(EditOp::Delete));
        let ctrl = |c| op_for_key(&key(Key::Char(c), true, false));
        assert_eq!(ctrl('a'), Some(EditOp::Home));
        assert_eq!(ctrl('e'), Some(EditOp::End));
        assert_eq!(ctrl('w'), Some(EditOp::DeleteWordBack));
        assert_eq!(ctrl('u'), Some(EditOp::DeleteToStart));
        assert_eq!(ctrl('k'), Some(EditOp::DeleteToEnd));
        let ctrl_left = key(Key::Left, true, false);
        assert_eq!(op_for_key(&ctrl_left), Some(EditOp::WordLeft));
        let alt_right = key(Key::Right, false, true);
        assert_eq!(op_for_key(&alt_right), Some(EditOp::WordRight));
        let alt_b = key(Key::Char('b'), false, true);
        assert_eq!(op_for_key(&alt_b), Some(EditOp::WordLeft));
        let alt_f = key(Key::Char('f'), false, true);
        assert_eq!(op_for_key(&alt_f), Some(EditOp::WordRight));
        let alt_bs = key(Key::Backspace, false, true);
        assert_eq!(op_for_key(&alt_bs), Some(EditOp::DeleteWordBack));
    }

    #[test]
    fn other_keys_are_not_editing_keys() {
        for k in [
            Key::Enter,
            Key::Escape,
            Key::Up,
            Key::Down,
            Key::Tab,
            Key::PageUp,
        ] {
            assert_eq!(op_for_key(&key(k, false, false)), None);
        }
        assert_eq!(op_for_key(&key(Key::Char('c'), true, false)), None);
        assert_eq!(op_for_key(&key(Key::Char('x'), false, true)), None);
        assert_eq!(op_for_key(&key(Key::Char('\u{7}'), false, false)), None);
        let mut meta = key(Key::Char('v'), false, false);
        meta.modifiers.meta = true;
        assert_eq!(op_for_key(&meta), None);
        let mut meta_left = key(Key::Left, false, false);
        meta_left.modifiers.meta = true;
        assert_eq!(op_for_key(&meta_left), None);
    }

    #[test]
    fn char_index_and_cursor_convert_both_ways() {
        // "a", "e\u{301}" (2 chars), "b": cursor 2 sits after 3 chars.
        assert_eq!(char_index_from_cursor(COMBINED, 2), 3);
        assert_eq!(cursor_from_char_index(COMBINED, 3), 2);
        assert_eq!(cursor_from_char_index(COMBINED, 2), 1);
        assert_eq!(cursor_from_char_index(COMBINED, 99), 3);
        assert_eq!(char_index_from_cursor(COMBINED, 99), 4);
    }

    #[test]
    fn split_at_cursor_yields_the_grapheme_under_it() {
        assert_eq!(split_at_cursor(ACCENTED, 1), ("h", "é", "llo wörld"));
        assert_eq!(split_at_cursor(ACCENTED, 11), (ACCENTED, "", ""));
        assert_eq!(split_at_cursor(COMBINED, 1), ("a", "e\u{301}", "b"));
        assert_eq!(split_at_cursor("", 0), ("", "", ""));
    }
}
