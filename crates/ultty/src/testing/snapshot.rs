//! Snapshot testing for terminal rendering verification.
//!
//! This module provides tools for capturing terminal state and comparing
//! against expected output, allowing autonomous verification of rendering.

use crate::grid::{CellFlags, Terminal};
use std::fmt::Write;

/// A snapshot of terminal state for comparison.
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    /// Text content (characters only)
    pub text: Vec<String>,
    /// Attributes per cell (for color/style verification)
    pub attrs: Vec<Vec<CellAttrs>>,
    /// Terminal dimensions
    pub cols: u16,
    pub rows: u16,
    /// Cursor position
    pub cursor_col: u16,
    pub cursor_row: u16,
    /// Whether in alternate screen
    pub alternate_screen: bool,
    /// Mouse tracking enabled
    pub mouse_tracking: bool,
}

/// Cell attributes for snapshot comparison.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub has_fg: bool,
    pub has_bg: bool,
}

impl TerminalSnapshot {
    /// Capture current terminal state.
    pub fn capture(terminal: &Terminal) -> Self {
        let grid = terminal.grid();
        let dims = grid.dimensions();
        let cursor = grid.cursor();

        let mut text = Vec::with_capacity(dims.rows as usize);
        let mut attrs = Vec::with_capacity(dims.rows as usize);

        for row_idx in 0..dims.rows {
            let mut row_text = String::with_capacity(dims.cols as usize);
            let mut row_attrs = Vec::with_capacity(dims.cols as usize);

            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.get(col_idx) {
                        row_text.push(cell.char());
                        row_attrs.push(CellAttrs {
                            bold: cell.has_flag(CellFlags::BOLD),
                            dim: cell.has_flag(CellFlags::DIM),
                            italic: cell.has_flag(CellFlags::ITALIC),
                            underline: cell.has_flag(CellFlags::UNDERLINE),
                            reverse: cell.has_flag(CellFlags::REVERSE),
                            has_fg: !matches!(cell.fg(), crate::grid::Color::Default),
                            has_bg: !matches!(cell.bg(), crate::grid::Color::Default),
                        });
                    } else {
                        row_text.push(' ');
                        row_attrs.push(CellAttrs::default());
                    }
                }
            }

            text.push(row_text);
            attrs.push(row_attrs);
        }

        Self {
            text,
            attrs,
            cols: dims.cols,
            rows: dims.rows,
            cursor_col: cursor.position.col,
            cursor_row: cursor.position.row,
            alternate_screen: terminal.is_alternate_screen(),
            mouse_tracking: terminal.modes().mouse_mode != crate::grid::MouseMode::None,
        }
    }

    /// Get text content as a single string.
    pub fn text_content(&self) -> String {
        self.text.join("\n")
    }

    /// Get trimmed text (remove trailing whitespace from each line).
    pub fn text_trimmed(&self) -> Vec<String> {
        self.text.iter().map(|s| s.trim_end().to_string()).collect()
    }

    /// Check if any row contains the given text.
    pub fn contains(&self, needle: &str) -> bool {
        self.text.iter().any(|row| row.contains(needle))
    }

    /// Check if a specific row contains text.
    pub fn row_contains(&self, row: u16, needle: &str) -> bool {
        self.text
            .get(row as usize)
            .map(|r| r.contains(needle))
            .unwrap_or(false)
    }

    /// Count non-empty rows.
    pub fn non_empty_rows(&self) -> usize {
        self.text.iter().filter(|r| !r.trim().is_empty()).count()
    }

    /// Check if there are cells with non-default colors.
    pub fn has_colors(&self) -> bool {
        self.attrs
            .iter()
            .any(|row| row.iter().any(|a| a.has_fg || a.has_bg))
    }

    /// Check if there are cells with reverse video.
    pub fn has_reverse(&self) -> bool {
        self.attrs.iter().any(|row| row.iter().any(|a| a.reverse))
    }

    /// Count cells with specific attribute.
    pub fn count_bold(&self) -> usize {
        self.attrs
            .iter()
            .flat_map(|r| r.iter())
            .filter(|a| a.bold)
            .count()
    }

    /// Generate a visual representation for debugging.
    pub fn to_debug_string(&self) -> String {
        let mut out = String::new();
        writeln!(out, "=== Terminal Snapshot ===").unwrap();
        writeln!(out, "Size: {}x{}", self.cols, self.rows).unwrap();
        writeln!(out, "Cursor: ({}, {})", self.cursor_col, self.cursor_row).unwrap();
        writeln!(out, "Alternate screen: {}", self.alternate_screen).unwrap();
        writeln!(out, "Mouse tracking: {}", self.mouse_tracking).unwrap();
        writeln!(out, "Has colors: {}", self.has_colors()).unwrap();
        writeln!(out, "Non-empty rows: {}", self.non_empty_rows()).unwrap();
        writeln!(out, "--- Content ---").unwrap();
        for (i, line) in self.text_trimmed().iter().enumerate() {
            if !line.is_empty() {
                writeln!(out, "{:3}|{}", i, line).unwrap();
            }
        }
        writeln!(out, "================").unwrap();
        out
    }

    /// Compare with another snapshot, return differences.
    pub fn diff(&self, other: &TerminalSnapshot) -> SnapshotDiff {
        let mut differences = Vec::new();

        if self.cols != other.cols || self.rows != other.rows {
            differences.push(format!(
                "Size differs: {}x{} vs {}x{}",
                self.cols, self.rows, other.cols, other.rows
            ));
        }

        if self.alternate_screen != other.alternate_screen {
            differences.push(format!(
                "Alternate screen differs: {} vs {}",
                self.alternate_screen, other.alternate_screen
            ));
        }

        if self.cursor_col != other.cursor_col || self.cursor_row != other.cursor_row {
            differences.push(format!(
                "Cursor position differs: ({},{}) vs ({},{})",
                self.cursor_col, self.cursor_row, other.cursor_col, other.cursor_row
            ));
        }

        // Compare text content
        let max_rows = self.text.len().max(other.text.len());
        for i in 0..max_rows {
            let self_row = self.text.get(i).map(|s| s.as_str()).unwrap_or("");
            let other_row = other.text.get(i).map(|s| s.as_str()).unwrap_or("");
            if self_row.trim() != other_row.trim() {
                differences.push(format!("Row {} differs", i));
            }
        }

        SnapshotDiff {
            matches: differences.is_empty(),
            differences,
        }
    }
}

/// Result of comparing two snapshots.
#[derive(Debug)]
pub struct SnapshotDiff {
    pub matches: bool,
    pub differences: Vec<String>,
}

/// Verify terminal behavior with assertions.
pub struct TerminalVerifier<'a> {
    snapshot: TerminalSnapshot,
    terminal: &'a Terminal,
}

impl<'a> TerminalVerifier<'a> {
    pub fn new(terminal: &'a Terminal) -> Self {
        Self {
            snapshot: TerminalSnapshot::capture(terminal),
            terminal,
        }
    }

    /// Refresh the snapshot.
    pub fn refresh(&mut self) {
        self.snapshot = TerminalSnapshot::capture(self.terminal);
    }

    /// Print debug info.
    pub fn debug(&self) -> String {
        self.snapshot.to_debug_string()
    }

    /// Assert terminal contains text.
    pub fn assert_contains(&self, needle: &str) -> &Self {
        assert!(
            self.snapshot.contains(needle),
            "Expected to find '{}' in terminal output.\n{}",
            needle,
            self.debug()
        );
        self
    }

    /// Assert terminal is in alternate screen.
    pub fn assert_alternate_screen(&self) -> &Self {
        assert!(
            self.snapshot.alternate_screen,
            "Expected alternate screen to be active.\n{}",
            self.debug()
        );
        self
    }

    /// Assert terminal is NOT in alternate screen.
    pub fn assert_main_screen(&self) -> &Self {
        assert!(
            !self.snapshot.alternate_screen,
            "Expected main screen to be active.\n{}",
            self.debug()
        );
        self
    }

    /// Assert mouse tracking is enabled.
    pub fn assert_mouse_tracking(&self) -> &Self {
        assert!(
            self.snapshot.mouse_tracking,
            "Expected mouse tracking to be enabled.\n{}",
            self.debug()
        );
        self
    }

    /// Assert cursor is at position.
    pub fn assert_cursor_at(&self, col: u16, row: u16) -> &Self {
        assert_eq!(
            (self.snapshot.cursor_col, self.snapshot.cursor_row),
            (col, row),
            "Cursor position mismatch.\n{}",
            self.debug()
        );
        self
    }

    /// Assert there are colored cells.
    pub fn assert_has_colors(&self) -> &Self {
        assert!(
            self.snapshot.has_colors(),
            "Expected colored cells.\n{}",
            self.debug()
        );
        self
    }

    /// Assert specific row contains text.
    pub fn assert_row_contains(&self, row: u16, needle: &str) -> &Self {
        assert!(
            self.snapshot.row_contains(row, needle),
            "Row {} should contain '{}'\n{}",
            row,
            needle,
            self.debug()
        );
        self
    }

    /// Assert non-empty rows count.
    pub fn assert_min_rows(&self, min: usize) -> &Self {
        let actual = self.snapshot.non_empty_rows();
        assert!(
            actual >= min,
            "Expected at least {} non-empty rows, got {}\n{}",
            min,
            actual,
            self.debug()
        );
        self
    }

    /// Get the snapshot for custom checks.
    pub fn snapshot(&self) -> &TerminalSnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Dimensions;

    #[test]
    fn test_snapshot_capture() {
        let terminal = Terminal::new(Dimensions::new(80, 24));
        let snapshot = TerminalSnapshot::capture(&terminal);

        assert_eq!(snapshot.cols, 80);
        assert_eq!(snapshot.rows, 24);
        assert!(!snapshot.alternate_screen);
        assert!(!snapshot.mouse_tracking);
    }

    #[test]
    fn test_snapshot_debug() {
        let terminal = Terminal::new(Dimensions::new(80, 24));
        let snapshot = TerminalSnapshot::capture(&terminal);
        let debug = snapshot.to_debug_string();

        assert!(debug.contains("Terminal Snapshot"));
        assert!(debug.contains("80x24"));
    }
}
