//! Bounded native transcript selection over logical rendered-text offsets.

use std::collections::BTreeMap;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A visible row supplied by a native transcript. `start` is a UTF-8 byte
/// offset in one document revision, and `hard_break` denotes a real newline
/// following the row rather than a display wrap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRow {
    pub revision: u64,
    pub start: usize,
    pub text: String,
    pub hard_break: bool,
}

impl SelectionRow {
    /// Length of the rendered text for a region with transcript metadata.
    pub fn rendered_len(region: &crate::scene::Region) -> Option<usize> {
        if !region
            .metadata
            .iter()
            .any(|(key, _)| key == "transcript-row-v1")
        {
            return None;
        }
        match &region.content {
            ultty_protocol::Content::RichText(rich) => rich
                .spans
                .iter()
                .try_fold(0usize, |size, span| size.checked_add(span.text.len())),
            ultty_protocol::Content::Text(text) => Some(text.text.len()),
            _ => None,
        }
    }

    /// Decode the bounded metadata attached to a selectable rendered row.
    pub fn from_region(region: &crate::scene::Region) -> Option<Self> {
        let (_, value) = region
            .metadata
            .iter()
            .find(|(key, _)| key == "transcript-row-v1")?;
        if value.len() > 80 {
            return None;
        }
        let mut fields = value.split(':');
        let revision = fields.next()?.parse().ok()?;
        let start = fields.next()?.parse().ok()?;
        let hard_break = match fields.next()? {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        if fields.next().is_some() {
            return None;
        }
        if Self::rendered_len(region)? > MAX_CAPTURE_BYTES {
            return None;
        }
        let text = match &region.content {
            ultty_protocol::Content::RichText(rich) => rich
                .spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            ultty_protocol::Content::Text(text) => text.text.clone(),
            _ => return None,
        };
        if text.len() > MAX_CAPTURE_BYTES {
            return None;
        }
        Some(Self {
            revision,
            start,
            text,
            hard_break,
        })
    }

    fn end(&self) -> Option<usize> {
        self.start.checked_add(self.text.len())
    }

    /// Resolve a display-cell position to a grapheme boundary in the document.
    pub fn position(&self, column: usize) -> Option<usize> {
        let mut cells = 0;
        for (byte, grapheme) in self.text.grapheme_indices(true) {
            let width = grapheme.width().max(1);
            if column < cells + width {
                return self.start.checked_add(byte);
            }
            cells += width;
        }
        self.end()
    }
}

/// Capture only rows encountered during the current drag. A changed revision,
/// contradictory row, missing row, or size overflow invalidates the drag.
#[derive(Default)]
pub struct NativeSelection {
    revision: Option<u64>,
    anchor: usize,
    focus: usize,
    rows: BTreeMap<usize, SelectionRow>,
    bytes: usize,
    valid: bool,
}

/// Maximum rendered text retained during one drag.
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
/// Maximum number of rendered rows retained during one drag.
pub const MAX_CAPTURE_ROWS: usize = 65_536;

impl NativeSelection {
    pub fn begin(&mut self, row: SelectionRow, column: usize) -> bool {
        *self = Self::default();
        let Some(anchor) = row.position(column) else {
            return false;
        };
        self.revision = Some(row.revision);
        self.anchor = anchor;
        self.focus = anchor;
        self.valid = true;
        self.observe(row)
    }

    pub fn observe(&mut self, row: SelectionRow) -> bool {
        if !self.valid || Some(row.revision) != self.revision || row.end().is_none() {
            self.valid = false;
            return false;
        }
        if let Some(previous) = self.rows.get(&row.start) {
            if previous != &row {
                self.valid = false;
            }
            return self.valid;
        }
        let row_end = row
            .end()
            .and_then(|end| end.checked_add(usize::from(row.hard_break)));
        let overlaps_previous =
            self.rows
                .range(..row.start)
                .next_back()
                .is_some_and(|(_, previous)| {
                    previous
                        .end()
                        .and_then(|end| end.checked_add(usize::from(previous.hard_break)))
                        .is_none_or(|end| end > row.start)
                });
        let overlaps_next = self
            .rows
            .range(row.start..)
            .next()
            .is_some_and(|(start, _)| row_end.is_none_or(|end| end > *start));
        if overlaps_previous || overlaps_next {
            self.valid = false;
            return false;
        }
        if self.rows.len() >= MAX_CAPTURE_ROWS {
            self.valid = false;
            return false;
        }
        let Some(bytes) = self.bytes.checked_add(row.text.len()) else {
            self.valid = false;
            return false;
        };
        if bytes > MAX_CAPTURE_BYTES {
            self.valid = false;
            return false;
        }
        self.bytes = bytes;
        self.rows.insert(row.start, row);
        true
    }

    pub fn extend(&mut self, row: SelectionRow, column: usize) -> bool {
        let Some(focus) = row.position(column) else {
            self.valid = false;
            return false;
        };
        if !self.observe(row) {
            return false;
        }
        self.focus = focus;
        true
    }

    /// The source revision captured when selection began.
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }

    pub fn range(&self) -> Option<Range<usize>> {
        self.valid
            .then(|| self.anchor.min(self.focus)..self.anchor.max(self.focus))
    }

    /// Find an HTTP(S) URL at a document byte position in captured contiguous rows.
    pub fn url_at(&self, position: usize) -> Option<String> {
        if !self.valid {
            return None;
        }
        let mut text = String::new();
        let mut base = None;
        let mut end = 0;
        for row in self.rows.values() {
            if base.is_none() || row.start != end {
                if let Some(start) = base {
                    if position >= start && position < end {
                        return crate::url::url_at_text(&text, position - start);
                    }
                }
                text.clear();
                base = Some(row.start);
            }
            text.push_str(&row.text);
            end = row.end()?;
            if row.hard_break {
                text.push('\n');
                end = end.checked_add(1)?;
            }
        }
        let start = base?;
        (position >= start && position < end)
            .then(|| crate::url::url_at_text(&text, position - start))
            .flatten()
    }

    /// Return selected rendered text only when all intervening rows are known.
    pub fn text(&self) -> Option<String> {
        let selection = self.range()?;
        if selection.is_empty() {
            return Some(String::new());
        }
        let mut result = String::new();
        let mut cursor = selection.start;
        for row in self.rows.values() {
            let end = row.end()?;
            if end < cursor || row.start > cursor {
                continue;
            }
            let from = cursor - row.start;
            let to = (selection.end.min(end)).checked_sub(row.start)?;
            if from > to || !row.text.is_char_boundary(from) || !row.text.is_char_boundary(to) {
                return None;
            }
            result.push_str(&row.text[from..to]);
            cursor = row.start + to;
            if cursor == selection.end {
                return Some(result);
            }
            if cursor == end && row.hard_break {
                result.push('\n');
                cursor = cursor.checked_add(1)?;
                if cursor == selection.end {
                    return Some(result);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start: usize, text: &str, hard_break: bool) -> SelectionRow {
        SelectionRow {
            revision: 7,
            start,
            text: text.into(),
            hard_break,
        }
    }

    #[test]
    fn cell_positions_follow_graphemes_with_wide_combining_and_emoji_text() {
        for (text, column) in [("雪A", 2), ("e\u{301}A", 1), ("👩\u{200d}💻A", 2)] {
            let row = row(0, text, false);
            assert_eq!(&text[row.position(column).unwrap()..], "A");
        }
    }

    #[test]
    fn selects_across_soft_wrap_and_hard_newline_without_losing_unicode_or_spaces() {
        let mut selection = NativeSelection::default();
        assert!(selection.begin(row(0, "a雪 ", false), 1));
        assert!(selection.observe(row(5, "e\u{301}  ", true)));
        assert!(selection.extend(row(11, "  code  ", false), 6));
        assert_eq!(selection.text().as_deref(), Some("雪 e\u{301}  \n  code"));
    }

    #[test]
    fn overlapping_reflow_rows_invalidate_active_capture() {
        let mut selection = NativeSelection::default();
        assert!(selection.begin(row(0, "abcdefgh", false), 0));
        assert!(!selection.observe(row(4, "efghijkl", false)));
        assert_eq!(selection.text(), None);
    }

    #[test]
    fn missing_rows_or_changed_revision_never_return_partial_text() {
        let mut selection = NativeSelection::default();
        assert!(selection.begin(row(0, "abc", false), 0));
        assert!(selection.extend(row(10, "tail", false), 4));
        assert_eq!(selection.text(), None);
        let mut changed = row(0, "abc", false);
        changed.revision += 1;
        assert!(!selection.observe(changed));
        assert_eq!(selection.text(), None);
    }
}
