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

/// The cells a grapheme cluster takes on a line: its width by Unicode, and
/// at least one, so a cluster of marks alone still has a cell. Both the
/// layout of an input's text and its caret count with this.
pub(crate) fn grapheme_cells(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme).max(1)
}

/// The column in cells of the caret of `input`, from the start of its text:
/// after the first `cursor` grapheme clusters of the text shown (the
/// placeholder while the value is empty). `None` when it has no caret, and
/// for a multiline input, whose lines are not laid out here.
pub(crate) fn caret_column(input: &InputWidget) -> Option<usize> {
    if input.multiline {
        return None;
    }
    let cursor = input.cursor? as usize;
    Some(
        shown_text(input)
            .graphemes(true)
            .take(cursor)
            .map(grapheme_cells)
            .sum(),
    )
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
        assert_eq!(caret_column(&input("héllo", Some(2))), Some(2));
        assert_eq!(caret_column(&input("héllo", Some(5))), Some(5));
        // A combining accent is one cluster, one cell; a wide one is two.
        assert_eq!(caret_column(&input("ae\u{301}b", Some(2))), Some(2));
        assert_eq!(caret_column(&input("你好", Some(1))), Some(2));
        let family = "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b";
        assert_eq!(caret_column(&input(family, Some(2))), Some(3));
    }

    #[test]
    fn no_cursor_no_caret_and_an_empty_input_counts_into_its_placeholder() {
        assert_eq!(caret_column(&input("ab", None)), None);
        let placeholder = InputWidget {
            placeholder: "type here".into(),
            cursor: Some(0),
            ..InputWidget::default()
        };
        assert_eq!(caret_column(&placeholder), Some(0));
        let later = InputWidget {
            cursor: Some(3),
            ..placeholder
        };
        assert_eq!(caret_column(&later), Some(3));
    }

    #[test]
    fn a_password_has_one_bullet_per_cluster_and_a_multiline_input_no_caret() {
        let password = InputWidget {
            value: "ae\u{301}b".into(),
            password: true,
            cursor: Some(3),
            ..InputWidget::default()
        };
        assert_eq!(caret_column(&password), Some(3));
        let multiline = InputWidget {
            value: "ab\ncd".into(),
            multiline: true,
            cursor: Some(4),
            ..InputWidget::default()
        };
        assert_eq!(caret_column(&multiline), None);
    }
}
