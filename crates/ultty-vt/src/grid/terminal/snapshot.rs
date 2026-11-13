//! Versioned rendering snapshots. These contain semantic rows, not parser or
//! process state, and can only reconstruct a viewer model. The owner keeps both
//! screens and all application connections. Selection is local to each viewer.

use serde::{Deserialize, Serialize};

use super::{ShellIntegration, Terminal, TerminalModes};
use crate::grid::image::MAX_SIXEL_DIM;
use crate::grid::{
    Anchor, AnchorTag, Buffer, Cell, CellFlags, Color, CursorShape, Dimensions, Row, RowMarks,
    TerminalImage, ThemeColors, MAX_ANCHORS, MAX_COMBINING_PER_CELL,
};

/// The wire version of the explicit terminal-view schema.
pub const VIEW_VERSION: u16 = 1;
/// Maximum rows retained by a single viewer snapshot, including both screens.
pub const MAX_VIEW_ROWS: usize = 20_000;
/// Maximum cells expanded when accepting a snapshot.
pub const MAX_VIEW_CELLS: usize = 2_000_000;
/// Maximum semantic payload copied before the transport's encoded byte limit.
pub const MAX_VIEW_BYTES: usize = 16 * 1024 * 1024;

/// A terminal's complete rendering and copying state at one observation point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    /// Schema version, checked before constructing any grid.
    pub version: u16,
    /// Main screen and retained scrollback.
    pub main: BufferSnapshot,
    /// Alternate screen, even when inactive.
    pub alternate: BufferSnapshot,
    /// Which screen is currently displayed.
    pub using_alternate: bool,
    /// Input-encoding and display modes; never replayed as escape sequences.
    pub modes: TerminalModes,
    /// Window/pane title reported by the application.
    pub title: String,
    /// Theme defaults.
    pub theme: ThemeColors,
    /// Application palette overrides, keyed by palette index.
    pub palette: Vec<(u8, [u8; 3])>,
    /// Retained decoded images with placement and identity.
    pub images: Vec<TerminalImage>,
    /// OSC 8 links, keyed by the IDs referenced by cells.
    pub hyperlinks: Vec<(u32, String)>,
    /// Shell-reported command boundaries and working directory.
    pub shell: ShellIntegration,
    /// Full program status hierarchy; consumed separately from the viewer grid.
    pub program_status: crate::program_status::ProgramStatusSnapshot,
}

/// One screen, with oldest history rows followed by the live grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferSnapshot {
    /// Canonical viewport columns.
    pub cols: u16,
    /// Canonical viewport rows.
    pub screen_rows: u16,
    /// Rows, oldest first. Default trailing cells are implicit.
    pub rows: Vec<RowSnapshot>,
    /// Cursor in the live grid.
    pub cursor: CursorSnapshot,
    /// Owner's viewport offset, clamped to retained history on hydration.
    pub scroll_offset: usize,
    /// Live inline anchors, independent of the native surface trees.
    pub anchors: Vec<AnchorSnapshot>,
}

/// Cursor appearance and position; saved parser cursor stacks remain owner-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorSnapshot {
    /// Column and row, respectively.
    pub position: [u16; 2],
    /// Whether applications request a visible cursor.
    pub visible: bool,
    /// Block=0, underline=1, bar=2.
    pub shape: u8,
    /// Whether the cursor should blink.
    pub blinking: bool,
    /// Cursor at a right-margin wrap boundary.
    pub wrap_pending: bool,
}

/// A semantic row, independent of packed cell/history representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowSnapshot {
    /// Cells through the last non-default cell; trailing blanks are implicit.
    pub cells: Vec<CellSnapshot>,
    /// Soft-wrap continuation, used by copy and reflow.
    pub wrapped: bool,
    /// OSC 133 input-start, output-start and command-end tags.
    pub marks: [u8; 3],
    /// Inline anchor tag, or zero.
    pub anchor: u16,
    /// Whether this row starts its anchor reservation.
    pub anchor_first: bool,
}

/// One character cell. Colors use [kind,r,g,b]: default=0, named=1,
/// indexed=2, RGB=3; named/indexed place their index in r.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellSnapshot {
    /// Base character followed by bounded combining/cluster codepoints.
    pub grapheme: String,
    /// Foreground color.
    pub fg: [u8; 4],
    /// Background color.
    pub bg: [u8; 4],
    /// Underline color.
    pub underline: [u8; 4],
    /// Semantic flags as defined by snapshot version 1.
    pub flags: u16,
    /// Hyperlink identity, zero for none.
    pub hyperlink: u32,
}

/// A live inline surface reservation and its first absolute row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorSnapshot {
    /// Buffer-local tag.
    pub tag: u16,
    /// Placement generation; prevents reused tags matching old surfaces.
    pub serial: u64,
    /// Native session identity.
    pub session: u64,
    /// Surface identity within that session.
    pub surface: u32,
    /// Reserved rows.
    pub rows: u16,
    /// First row, counted from the oldest retained row.
    pub line: usize,
}

fn color(value: Color) -> [u8; 4] {
    match value {
        Color::Default => [0, 0, 0, 0],
        Color::Named(n) => [1, n, 0, 0],
        Color::Indexed(n) => [2, n, 0, 0],
        Color::Rgb(r, g, b) => [3, r, g, b],
    }
}
fn decode_color(value: [u8; 4]) -> Result<Color, String> {
    match value {
        [0, 0, 0, 0] => Ok(Color::Default),
        [1, n @ 0..=15, 0, 0] => Ok(Color::Named(n)),
        [2, n, 0, 0] => Ok(Color::Indexed(n)),
        [3, r, g, b] => Ok(Color::Rgb(r, g, b)),
        _ => Err("invalid snapshot cell color".into()),
    }
}

impl TerminalSnapshot {
    /// Capture both screens under the caller's terminal lock, with fixed bounds.
    pub fn capture(terminal: &Terminal) -> Result<Self, String> {
        let rows = terminal
            .main
            .total_lines()
            .saturating_add(terminal.alternate.total_lines());
        let cells = terminal
            .main
            .total_lines()
            .saturating_mul(usize::from(terminal.main.dimensions().cols))
            .saturating_add(
                terminal
                    .alternate
                    .total_lines()
                    .saturating_mul(usize::from(terminal.alternate.dimensions().cols)),
            );
        if rows > MAX_VIEW_ROWS
            || cells > MAX_VIEW_CELLS
            || terminal.images.total_bytes() > MAX_VIEW_BYTES
        {
            return Err("terminal exceeds viewer snapshot bounds".into());
        }
        let mut budget = MAX_VIEW_BYTES.saturating_sub(terminal.images.total_bytes());
        let main = BufferSnapshot::capture(&terminal.main, &mut budget)?;
        let alternate = BufferSnapshot::capture(&terminal.alternate, &mut budget)?;
        Ok(Self {
            version: VIEW_VERSION,
            main,
            alternate,
            using_alternate: terminal.using_alternate,
            modes: terminal.modes,
            title: terminal.title.clone(),
            theme: terminal.palette.theme().clone(),
            palette: (0..=255)
                .filter_map(|i| terminal.palette.override_for(i).map(|rgb| (i, rgb)))
                .collect(),
            images: terminal.images.images().cloned().collect(),
            hyperlinks: terminal
                .hyperlinks
                .iter()
                .map(|(&id, uri)| (id, uri.clone()))
                .collect(),
            shell: terminal.shell_integration.clone(),
            program_status: terminal.program_status.snapshot(),
        })
    }

    /// Validate and construct a render/copy model. It has no parser, processes,
    /// transport, pending responses or authority to execute the captured state.
    pub fn to_view(&self) -> Result<Terminal, String> {
        if self.version != VIEW_VERSION {
            return Err("unsupported terminal snapshot version".into());
        }
        let rows = self
            .main
            .rows
            .len()
            .saturating_add(self.alternate.rows.len());
        let cells = self
            .main
            .rows
            .len()
            .saturating_mul(usize::from(self.main.cols))
            .saturating_add(
                self.alternate
                    .rows
                    .len()
                    .saturating_mul(usize::from(self.alternate.cols)),
            );
        if rows > MAX_VIEW_ROWS
            || cells > MAX_VIEW_CELLS
            || self.title.len() > 65536
            || self.hyperlinks.len() > super::MAX_HYPERLINKS
            || self.palette.len() > 256
            || self.images.len() > 1024
        {
            return Err("terminal snapshot exceeds bounds".into());
        }
        let mut palette_indices = std::collections::HashSet::new();
        if self
            .palette
            .iter()
            .any(|(index, _)| !palette_indices.insert(*index))
        {
            return Err("duplicate snapshot palette index".into());
        }
        let mut image_ids = std::collections::HashSet::new();
        let mut budget = MAX_VIEW_BYTES;
        for image in &self.images {
            let bytes = u64::from(image.width)
                .checked_mul(u64::from(image.height))
                .and_then(|n| n.checked_mul(4))
                .ok_or("snapshot image size overflow")?;
            if image.width == 0
                || image.height == 0
                || image.width > MAX_SIXEL_DIM
                || image.height > MAX_SIXEL_DIM
                || !image_ids.insert(image.id)
                || bytes != image.data.len() as u64
                || bytes > budget as u64
            {
                return Err("invalid snapshot image".into());
            }
            budget -= bytes as usize;
        }
        let mut terminal = Terminal::new(Dimensions::new(1, 1));
        terminal.main = self.main.to_view(&mut budget)?;
        terminal.alternate = self.alternate.to_view(&mut budget)?;
        terminal.using_alternate = self.using_alternate;
        terminal.modes = self.modes;
        terminal.title = self.title.clone();
        terminal.palette.set_theme(self.theme.clone());
        for &(index, rgb) in &self.palette {
            terminal.palette.set(index, rgb);
        }
        for image in &self.images {
            terminal.images.add(image.clone());
        }
        for (id, uri) in &self.hyperlinks {
            if *id == 0
                || uri.len() > 65536
                || terminal.hyperlinks.insert(*id, uri.clone()).is_some()
            {
                return Err("invalid snapshot hyperlink identity".into());
            }
        }
        terminal.shell_integration = self.shell.clone();
        terminal.check_integrity()?;
        Ok(terminal)
    }
}

impl BufferSnapshot {
    fn capture(buffer: &Buffer, budget: &mut usize) -> Result<Self, String> {
        let dims = buffer.dimensions();
        let mut rows = Vec::with_capacity(buffer.total_lines());
        for index in 0..buffer.total_lines() {
            let row = buffer.row_at(index).ok_or("missing snapshot row")?;
            let used = (0..row.width())
                .rev()
                .find(|&col| row.cell(col).is_some_and(|cell| !cell.is_default()))
                .map_or(0, |col| col + 1);
            let mut cells = Vec::with_capacity(usize::from(used));
            *budget = budget
                .checked_sub(32)
                .ok_or("terminal snapshot exceeds byte budget")?;
            for col in 0..used {
                let cell = row.cell(col).ok_or("missing snapshot cell")?;
                let mut grapheme = cell.char().to_string();
                if let Some(extra) = row.grapheme_extra(col) {
                    grapheme.extend(extra);
                }
                let cost = 32 + grapheme.len();
                *budget = budget
                    .checked_sub(cost)
                    .ok_or("terminal snapshot exceeds byte budget")?;
                cells.push(CellSnapshot {
                    grapheme,
                    fg: color(cell.fg()),
                    bg: color(cell.bg()),
                    underline: color(cell.underline_color()),
                    flags: cell.flags().bits(),
                    hyperlink: cell.hyperlink_id(),
                });
            }
            let marks = row.marks();
            rows.push(RowSnapshot {
                cells,
                wrapped: row.is_wrapped(),
                marks: [marks.input_start, marks.output_start, marks.command_end],
                anchor: row.anchor().tag(),
                anchor_first: row.anchor().is_first(),
            });
        }
        let cursor = buffer.grid().cursor();
        let anchors = buffer
            .anchors()
            .filter_map(|(tag, anchor)| {
                Some(AnchorSnapshot {
                    tag,
                    serial: buffer.anchor_serial(tag)?,
                    session: anchor.session_id,
                    surface: anchor.surface_id,
                    rows: anchor.rows,
                    line: buffer.anchor_line(tag)?,
                })
            })
            .collect();
        Ok(Self {
            cols: dims.cols,
            screen_rows: dims.rows,
            rows,
            scroll_offset: buffer.scroll_offset(),
            anchors,
            cursor: CursorSnapshot {
                position: [cursor.position.col, cursor.position.row],
                visible: cursor.visible,
                shape: match cursor.shape {
                    CursorShape::Block => 0,
                    CursorShape::Underline => 1,
                    CursorShape::Bar => 2,
                },
                blinking: cursor.blinking,
                wrap_pending: cursor.wrap_pending,
            },
        })
    }

    fn to_view(&self, budget: &mut usize) -> Result<Buffer, String> {
        if self.cols == 0
            || self.screen_rows == 0
            || self.rows.len() < usize::from(self.screen_rows)
            || self.cursor.position[0] >= self.cols
            || self.cursor.position[1] >= self.screen_rows
            || self.cursor.shape > 2
            || self.anchors.len() > MAX_ANCHORS
        {
            return Err("invalid snapshot grid dimensions or cursor".into());
        }
        let history = self.rows.len() - usize::from(self.screen_rows);
        let mut buffer =
            Buffer::with_scrollback(Dimensions::new(self.cols, self.screen_rows), history);
        let mut restored = Vec::with_capacity(self.rows.len());
        for saved in &self.rows {
            if saved.cells.len() > usize::from(self.cols) || usize::from(saved.anchor) > MAX_ANCHORS
            {
                return Err("snapshot row exceeds grid bounds".into());
            }
            let mut row = Row::new(self.cols);
            for (col, saved) in saved.cells.iter().enumerate() {
                *budget = budget
                    .checked_sub(32 + saved.grapheme.len())
                    .ok_or("snapshot exceeds byte budget")?;
                let mut chars = saved.grapheme.chars();
                let first = chars.next().ok_or("empty snapshot grapheme")?;
                let extra: Vec<_> = chars.collect();
                if extra.len() > MAX_COMBINING_PER_CELL {
                    return Err("snapshot grapheme exceeds bound".into());
                }
                let mut flags = CellFlags::from_bits(saved.flags);
                flags.unset(CellFlags::HAS_COMBINING);
                let mut cell = Cell::with_attrs(
                    first,
                    decode_color(saved.fg)?,
                    decode_color(saved.bg)?,
                    flags,
                );
                cell.set_underline_color(decode_color(saved.underline)?);
                cell.set_hyperlink_id(saved.hyperlink);
                row[col as u16] = cell;
                row.set_grapheme_extra(col as u16, &extra);
            }
            row.set_wrapped(saved.wrapped);
            row.set_marks(RowMarks {
                input_start: saved.marks[0],
                output_start: saved.marks[1],
                command_end: saved.marks[2],
            });
            row.set_anchor(AnchorTag::new(saved.anchor, saved.anchor_first));
            row.check_integrity(MAX_COMBINING_PER_CELL)?;
            restored.push(row);
        }
        let screen = restored.split_off(history);
        buffer.push_to_history(restored);
        for (index, row) in screen.iter().enumerate() {
            buffer
                .grid_mut()
                .row_mut(index as u16)
                .ok_or("missing viewer row")?
                .assign(row);
        }
        let cursor = buffer.grid_mut().cursor_mut();
        cursor.position.col = self.cursor.position[0];
        cursor.position.row = self.cursor.position[1];
        cursor.visible = self.cursor.visible;
        cursor.blinking = self.cursor.blinking;
        cursor.wrap_pending = self.cursor.wrap_pending;
        cursor.shape = match self.cursor.shape {
            1 => CursorShape::Underline,
            2 => CursorShape::Bar,
            _ => CursorShape::Block,
        };
        let mut seen = std::collections::HashSet::new();
        let mut anchors = Vec::new();
        for saved in &self.anchors {
            if saved.tag == 0
                || usize::from(saved.tag) > MAX_ANCHORS
                || !seen.insert(saved.tag)
                || saved.rows == 0
                || saved.line.saturating_add(usize::from(saved.rows)) > self.rows.len()
            {
                return Err("invalid snapshot anchor".into());
            }
            for offset in 0..usize::from(saved.rows) {
                let row = &self.rows[saved.line + offset];
                if row.anchor != saved.tag || row.anchor_first != (offset == 0) {
                    return Err("snapshot anchor rows disagree".into());
                }
            }
            anchors.push((
                saved.tag,
                saved.serial,
                Anchor {
                    session_id: saved.session,
                    surface_id: saved.surface,
                    rows: saved.rows,
                },
                saved.line,
            ));
        }
        buffer.restore_view_anchors(&anchors);
        buffer.scroll_up(self.scroll_offset.min(history));
        buffer.check_integrity()?;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::AnsiParser;
    use crate::grid::{ImageId, TerminalHandler};

    fn feed(terminal: &mut Terminal, parser: &mut AnsiParser, bytes: &[u8]) {
        let mut handler = TerminalHandler::new(terminal);
        parser.feed(bytes, |action| handler.handle(action));
    }

    #[test]
    fn test_view_snapshot_preserves_both_screens_styles_history_and_links() {
        let mut terminal = Terminal::new(Dimensions::new(12, 3));
        let mut parser = AnsiParser::new();
        feed(&mut terminal, &mut parser, "first\r\nsecond\r\nthird\r\n\x1b[31m中e\u{301}\x1b[0m\x1b]8;;https://example.test\x1b\\link\x1b]8;;\x1b\\".as_bytes());
        feed(
            &mut terminal,
            &mut parser,
            b"\x1b]4;1;rgb:12/34/56\x1b\\\x1b[?1049hALT\x1b[?2004h",
        );
        terminal.images.add(TerminalImage {
            id: ImageId(9),
            width: 1,
            height: 1,
            data: vec![1, 2, 3, 255],
            start_col: 2,
            start_row: 1,
            cell_width: 1,
            cell_height: 1,
        });
        let snapshot = TerminalSnapshot::capture(&terminal).unwrap();
        let mut view = snapshot.to_view().unwrap();
        assert!(view.is_alternate_screen());
        assert!(view.modes().bracketed_paste);
        assert_eq!(
            view.grid().row(0).unwrap().text(),
            terminal.grid().row(0).unwrap().text()
        );
        assert_eq!(view.main.total_lines(), terminal.main.total_lines());
        for index in 0..terminal.main.total_lines() {
            let original = terminal.main.row_at(index).unwrap().to_row();
            assert!(view.main.row_at(index).unwrap().eq_row(&original));
        }
        assert_eq!(view.palette().color(1), [0x12, 0x34, 0x56]);
        assert_eq!(view.hyperlinks, terminal.hyperlinks);
        assert_eq!(
            view.images.images().next().unwrap().data,
            vec![1, 2, 3, 255]
        );
        assert!(view.take_responses().is_empty());
    }

    #[test]
    fn test_view_snapshot_does_not_transfer_partial_parser_input() {
        let mut terminal = Terminal::new(Dimensions::new(10, 2));
        let mut parser = AnsiParser::new();
        feed(&mut terminal, &mut parser, b"old\x1b[31");
        let first = TerminalSnapshot::capture(&terminal)
            .unwrap()
            .to_view()
            .unwrap();
        assert_eq!(first.grid().row(0).unwrap().text_trimmed(), "old");
        feed(&mut terminal, &mut parser, b"mnew");
        let second = TerminalSnapshot::capture(&terminal)
            .unwrap()
            .to_view()
            .unwrap();
        assert_eq!(second.grid().row(0).unwrap().text_trimmed(), "oldnew");
        assert_eq!(
            second.grid().row(0).unwrap().cell(3).unwrap().fg(),
            Color::Named(1)
        );
    }

    #[test]
    fn test_view_snapshot_rejects_unbounded_or_inconsistent_input_before_hydration() {
        let terminal = Terminal::new(Dimensions::new(10, 2));
        let mut snapshot = TerminalSnapshot::capture(&terminal).unwrap();
        snapshot.main.cols = u16::MAX;
        snapshot.main.screen_rows = u16::MAX;
        assert!(snapshot.to_view().is_err());
        let mut snapshot = TerminalSnapshot::capture(&terminal).unwrap();
        snapshot.images.push(TerminalImage {
            id: ImageId(9),
            width: u32::MAX,
            height: u32::MAX,
            data: vec![],
            start_col: 0,
            start_row: 0,
            cell_width: 0,
            cell_height: 0,
        });
        assert!(snapshot.to_view().is_err());
        let mut snapshot = TerminalSnapshot::capture(&terminal).unwrap();
        snapshot.main.anchors.push(AnchorSnapshot {
            tag: 1,
            serial: 1,
            session: 1,
            surface: 1,
            rows: 1,
            line: 0,
        });
        assert!(snapshot.to_view().is_err());
        snapshot.version += 1;
        assert!(snapshot.to_view().is_err());
    }

    #[test]
    fn test_view_snapshot_rejects_image_dimensions_and_duplicate_resource_identities() {
        let terminal = Terminal::new(Dimensions::new(10, 2));
        let baseline = TerminalSnapshot::capture(&terminal).unwrap();
        let image = TerminalImage {
            id: ImageId(9),
            width: MAX_SIXEL_DIM,
            height: 1,
            data: vec![255; MAX_SIXEL_DIM as usize * 4],
            start_col: 0,
            start_row: 0,
            cell_width: 10,
            cell_height: 1,
        };
        let mut valid = baseline.clone();
        valid.images.push(image.clone());
        assert!(
            valid.to_view().is_ok(),
            "the documented dimension cap is inclusive"
        );
        let mut oversized = baseline.clone();
        let mut wide = image.clone();
        wide.width += 1;
        wide.data.extend_from_slice(&[255; 4]);
        oversized.images.push(wide);
        assert!(
            oversized.to_view().is_err(),
            "valid RGBA lengths alone must not admit excessive dimensions"
        );
        let mut tall = image.clone();
        tall.width = 1;
        tall.height = MAX_SIXEL_DIM + 1;
        tall.data.extend_from_slice(&[255; 4]);
        let mut oversized = baseline.clone();
        oversized.images.push(tall);
        assert!(oversized.to_view().is_err());
        let mut duplicate_images = baseline.clone();
        duplicate_images.images = vec![image.clone(), image];
        assert!(
            duplicate_images.to_view().is_err(),
            "image replacement must not silently discard snapshot data"
        );
        let mut duplicate_palette = baseline;
        duplicate_palette.palette = vec![(1, [10, 20, 30]), (1, [40, 50, 60])];
        assert!(duplicate_palette.to_view().is_err());
    }
}
