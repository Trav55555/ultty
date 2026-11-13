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

/// Visible wrapped rows and a caret kept inside the input's viewport.
pub(crate) fn viewport(
    input: &InputWidget,
    width: usize,
    height: usize,
) -> (Vec<String>, Option<(usize, usize)>) {
    let text = shown_text(input);
    let cursor = input.cursor.map(|c| c as usize);
    let (rows, caret) = if input.multiline {
        input_rows(&text, width.max(1), cursor)
    } else {
        single_line(&text, width.max(1), cursor)
    };
    let height = height.max(1);
    let first = caret.map_or(0, |(_, row)| row.saturating_sub(height - 1));
    (
        rows.into_iter().skip(first).take(height).collect(),
        caret.map(|(col, row)| (col, row - first)),
    )
}

fn single_line(
    text: &str,
    width: usize,
    cursor: Option<usize>,
) -> (Vec<String>, Option<(usize, usize)>) {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let width = width.max(1);
    let clusters: Vec<&str> = text.graphemes(true).collect();
    let cells = |g: &str| UnicodeWidthStr::width(g).max(1).min(width);
    let at = cursor.unwrap_or(0).min(clusters.len());
    let mut start = at;
    let mut col = 0;
    while start > 0 && col + cells(clusters[start - 1]) < width {
        start -= 1;
        col += cells(clusters[start]);
    }
    if cursor.is_none() {
        start = 0;
    }
    let mut row = String::new();
    let mut used = 0;
    for g in &clusters[start..] {
        let n = cells(g);
        if used + n > width {
            break;
        }
        row.push_str(
            if *g == "\t" || *g == "\n" || UnicodeWidthStr::width(*g) > width {
                " "
            } else {
                g
            },
        );
        used += n;
    }
    (vec![row], cursor.map(|_| (col, 0)))
}

fn input_rows(
    text: &str,
    width: usize,
    cursor: Option<usize>,
) -> (Vec<String>, Option<(usize, usize)>) {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut col = 0;
    let mut caret = None;
    let clusters: Vec<&str> = text.graphemes(true).collect();
    for (index, cluster) in clusters.iter().copied().enumerate() {
        let shown = if cluster == "\t" { " " } else { cluster };
        let shown = if UnicodeWidthStr::width(shown) > width {
            " "
        } else {
            shown
        };
        let cells = UnicodeWidthStr::width(shown).max(1);
        if cluster != "\n" && col + cells > width {
            rows.push(String::new());
            col = 0;
        }
        if cursor == Some(index) {
            caret = Some((col, rows.len() - 1));
        }
        if cluster == "\n" {
            rows.push(String::new());
            col = 0;
        } else {
            if let Some(row) = rows.last_mut() {
                row.push_str(shown);
            }
            col += cells;
            if col == width && clusters.get(index + 1) != Some(&"\n") {
                rows.push(String::new());
                col = 0;
            }
        }
    }
    if cursor.is_some() && caret.is_none() {
        caret = Some((col, rows.len() - 1));
    }
    (rows, caret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_caret_counts_clusters_cells_password_and_placeholder() {
        for (value, cursor, expected) in [
            ("héllo", 2, 2),
            ("ae\u{301}b", 2, 2),
            ("你好", 1, 2),
            ("a👨‍👩‍👧b", 2, 3),
        ] {
            let input = InputWidget {
                value: value.into(),
                cursor: Some(cursor),
                ..Default::default()
            };
            assert_eq!(viewport(&input, 40, 1).1, Some((expected, 0)));
        }
        let input = InputWidget {
            value: "ae\u{301}b".into(),
            password: true,
            cursor: Some(3),
            ..Default::default()
        };
        assert_eq!(viewport(&input, 40, 1), (vec!["•••".into()], Some((3, 0))));
        let empty = InputWidget {
            placeholder: "type here".into(),
            cursor: Some(0),
            ..Default::default()
        };
        assert_eq!(viewport(&empty, 40, 1).1, Some((0, 0)));
        assert_eq!(
            viewport(
                &InputWidget {
                    cursor: None,
                    ..empty
                },
                40,
                1
            )
            .1,
            None
        );
    }

    #[test]
    fn test_too_wide_grapheme_becomes_one_clipped_cell() {
        let input = InputWidget {
            value: "你".into(),
            multiline: true,
            ..Default::default()
        };
        assert_eq!(viewport(&input, 1, 1).0, [" "]);
    }

    #[test]
    fn test_multiline_viewport_follows_unicode_caret() {
        let input = InputWidget {
            value: "ab\n你e\u{301}z".into(),
            multiline: true,
            cursor: Some(6),
            ..Default::default()
        };
        let (rows, caret) = viewport(&input, 3, 2);
        assert_eq!(rows, ["你e\u{301}", "z"]);
        assert_eq!(caret, Some((1, 1)));
    }

    #[test]
    fn test_single_line_exact_width_scrolls_suffix_and_tabs_are_cells() {
        let input = InputWidget {
            value: "a\tb".into(),
            cursor: Some(3),
            ..Default::default()
        };
        assert_eq!(viewport(&input, 3, 1), (vec![" b".into()], Some((2, 0))));
        assert_eq!(
            viewport(
                &InputWidget {
                    cursor: None,
                    ..input
                },
                3,
                1
            )
            .0,
            ["a b"]
        );
    }
}
