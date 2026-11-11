//! Where an input widget's caret falls in the text it shows.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use ultty_protocol::InputWidget;

/// The text an input draws: its placeholder while it is empty, else its
/// value, masked by one bullet per grapheme cluster for a password.
pub(crate) fn shown_text(input: &InputWidget) -> Cow<'_, str> {
    if input.value.is_empty() {
        input.placeholder.as_str().into()
    } else if input.password {
        "•".repeat(input.value.graphemes(true).count()).into()
    } else {
        input.value.as_str().into()
    }
}

/// The (column, row) offset in cells of the caret of `input`, from the
/// start of its text: after the first `cursor` grapheme clusters of the
/// text shown (the placeholder while the value is empty), a new line
/// moving it to the next row; `None` when it has no caret.
pub(crate) fn caret_offset(input: &InputWidget) -> Option<(usize, usize)> {
    let cursor = input.cursor? as usize;
    let text = shown_text(input);
    let (mut col, mut row) = (0, 0);
    for grapheme in text.graphemes(true).take(cursor) {
        if grapheme.contains('\n') {
            col = 0;
            row += 1;
        } else {
            col += UnicodeWidthStr::width(grapheme);
        }
    }
    Some((col, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(value: &str, cursor: Option<u32>) -> InputWidget {
        InputWidget {
            value: value.into(),
            cursor,
            ..InputWidget::default()
        }
    }

    #[test]
    fn the_caret_is_after_its_clusters_counted_in_cells() {
        assert_eq!(caret_offset(&input("héllo", Some(2))), Some((2, 0)));
        assert_eq!(caret_offset(&input("héllo", Some(5))), Some((5, 0)));
        // A combining accent is one cluster, one cell; a wide one is two.
        assert_eq!(caret_offset(&input("ae\u{301}b", Some(2))), Some((2, 0)));
        assert_eq!(caret_offset(&input("你好", Some(1))), Some((2, 0)));
    }

    #[test]
    fn no_cursor_no_caret_and_an_empty_input_counts_into_its_placeholder() {
        assert_eq!(caret_offset(&input("ab", None)), None);
        let placeholder = InputWidget {
            placeholder: "type here".into(),
            cursor: Some(0),
            ..InputWidget::default()
        };
        assert_eq!(caret_offset(&placeholder), Some((0, 0)));
        let after_marker = InputWidget {
            cursor: Some(3),
            ..placeholder
        };
        assert_eq!(caret_offset(&after_marker), Some((3, 0)));
    }

    #[test]
    fn a_password_has_one_bullet_per_cluster_and_new_lines_move_down() {
        let password = InputWidget {
            value: "ae\u{301}b".into(),
            password: true,
            cursor: Some(3),
            ..InputWidget::default()
        };
        assert_eq!(caret_offset(&password), Some((3, 0)));
        assert_eq!(caret_offset(&input("ab\ncd", Some(4))), Some((1, 1)));
    }
}
