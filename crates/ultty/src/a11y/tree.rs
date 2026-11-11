//! The AccessKit tree Ultty exposes, built without any platform types.
//!
//! ```text
//! Window (the window title)
//! ├── TabList
//! │   └── Tab, one per tab, the active one selected
//! └── TabPanel (the active tab's title)
//!     ├── Terminal (the focused pane's title): one TextRun per screen row
//!     │   (a row past 255 characters is several, on one line),
//!     │   the caret at the cursor; focus rests here
//!     │   └── an inline surface's root, after its anchor row's runs
//!     │       and described by that row, with its region tree
//!     └── Group (each other pane's title), no children
//! ```
//!
//! While a native app shows a full scene the grid is hidden under it, so
//! the focused pane is a `Group` (its title) holding the app's `ROOT`
//! region tree instead, and focus is on the app's focused region, or on
//! its root when nothing is focused ([`super::scene`]).
//!
//! A live region's announcements are `Label` children after its text and
//! child regions ([`super::live`]).
//!
//! The grid is the live screen: scrollback, per-command nodes (OSC 133)
//! and the selection are later work of ADR 0021.
//!
//! [`GridSnapshot::capture`] copies what the tree needs out of a locked
//! grid; [`build`] turns a [`Snapshot`] into a full [`TreeUpdate`] with no
//! lock held. Node ids are hashes of stable names (`t<tab>`, `p<pane>`,
//! `p<pane>r<row>`), so a rebuild that shows the same thing sends the same
//! nodes and AccessKit, which diffs updates, reports only what changed.

use accesskit::{
    Node, NodeId, Rect, Role, TextDirection, TextPosition, TextSelection, TreeId, TreeInfo,
    TreeUpdate,
};

use super::live::LiveRegions;
use super::scene::{Emitter, SceneSnapshot, MAX_PANE_NODES};
use crate::grid::{CellFlags, Grid, RowView};

/// The window's node, the root of the tree.
pub const WINDOW_ID: NodeId = NodeId(0);

/// What the window shows, gathered by the app for [`build`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Every tab, in tab bar order.
    pub tabs: Vec<TabSnapshot>,
    /// The panes of the selected tab, in layout order.
    pub panes: Vec<PaneSnapshot>,
}

impl Snapshot {
    /// Give the live regions in the snapshot's scenes and surfaces what
    /// `live` has announced for them.
    pub fn set_announcements(&mut self, live: &LiveRegions) {
        for pane in &mut self.panes {
            let surfaces = pane.surfaces.iter_mut().map(|surface| &mut surface.scene);
            for scene in pane.scenes.iter_mut().chain(surfaces) {
                scene.set_announcements(live);
            }
        }
    }
}

/// One tab of the tab bar.
#[derive(Debug, Clone, PartialEq)]
pub struct TabSnapshot {
    pub id: usize,
    pub title: String,
    /// The tab on screen.
    pub selected: bool,
}

/// One pane of the selected tab.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneSnapshot {
    pub id: usize,
    pub title: String,
    /// Where the pane is, in window pixels.
    pub bounds: Rect,
    /// The screen's text, for the focused pane only, unless a native app
    /// hides it under a full scene.
    pub grid: Option<GridSnapshot>,
    /// The `ROOT` trees of the full-scene native apps the focused pane
    /// shows, in drawing order; when there are any, they replace the grid.
    pub scenes: Vec<SceneSnapshot>,
    /// The inline surfaces on the focused pane's screen.
    pub surfaces: Vec<SurfaceSnapshot>,
}

/// An inline surface on the screen (ADR 0020).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceSnapshot {
    /// The screen row its first row (its anchor row) shows on; negative
    /// when it starts above the screen.
    pub row: i64,
    pub scene: SceneSnapshot,
}

/// The text of a pane's screen, copied out of its grid.
#[derive(Debug, Clone, PartialEq)]
pub struct GridSnapshot {
    rows: Vec<RowText>,
    /// The cursor as (row, character index in that row).
    caret: Option<(usize, usize)>,
    cell_width: f64,
    cell_height: f64,
}

/// One screen row: its characters (a cell's whole grapheme each, a wide
/// character once), the columns they start at and how many they span,
/// where words start, and the line break unless the row soft-wraps.
#[derive(Debug, Clone, PartialEq)]
struct RowText {
    text: String,
    /// UTF-8 length of each character, the line break included.
    lengths: Vec<u8>,
    /// First column of each character; the line break sits after the last.
    columns: Vec<u16>,
    /// Columns each character spans; 0 for the line break.
    widths: Vec<u8>,
    /// Whether a word starts at each character.
    word_starts: Vec<bool>,
    line_break: bool,
}

/// The most characters in one `TextRun`. AccessKit's word starts are a
/// list of `u8` and its consumer keeps character indices in a `u8`, so a
/// longer row is split into runs on the same line.
const MAX_RUN_CHARS: usize = 255;

/// How a row begins: on a new line, or continuing a soft-wrapped one whose
/// last character was blank or not (a word running on across the wrap).
#[derive(Debug, Clone, Copy, PartialEq)]
enum RowStart {
    NewLine,
    After { blank: bool },
}

impl GridSnapshot {
    /// Copy the screen rows and cursor of `grid`, whose cells are
    /// `cell_width` by `cell_height` pixels. One pass over the cells: this
    /// runs with the terminal locked.
    pub fn capture(grid: &Grid, cell_width: f64, cell_height: f64) -> Self {
        let dims = grid.dimensions();
        let cursor = grid.cursor();
        // With a wrap pending the cursor is past the last column, not on it.
        let cursor_col = cursor.position.col + u16::from(cursor.wrap_pending);
        let mut caret = None;
        let mut start = RowStart::NewLine;
        let mut rows = Vec::with_capacity(usize::from(dims.rows));
        for index in 0..dims.rows {
            let Some(row) = grid.row(index) else { break };
            let line_break = index + 1 < dims.rows && !row.is_wrapped();
            let text = if index == cursor.position.row {
                // Keep the blanks before the cursor, so the caret after a
                // prompt's trailing space lands after it.
                let text = RowText::capture(&row, cursor_col, line_break, start);
                caret = Some((usize::from(index), text.char_index_at(cursor_col)));
                text
            } else {
                RowText::capture(&row, 0, line_break, start)
            };
            start = if row.is_wrapped() {
                RowStart::After {
                    blank: text.ends_blank(),
                }
            } else {
                RowStart::NewLine
            };
            rows.push(text);
        }
        Self {
            rows,
            caret,
            cell_width,
            cell_height,
        }
    }
}

impl RowText {
    /// The row's characters through its last non-blank column, or through
    /// `keep_columns` if that is further; all of them when the row
    /// soft-wraps, since its trailing blanks separate words from the next
    /// row's. A hidden (concealed) cell reads as a blank, as it is drawn.
    ///
    /// A wide character that did not fit at the end of a wrapping row
    /// leaves its last column as an ordinary blank cell (the grid does not
    /// mark it), so that blank is read as a space, as copying does.
    fn capture(row: &RowView<'_>, keep_columns: u16, line_break: bool, start: RowStart) -> Self {
        let shows = |col: u16| {
            row.cell(col).is_some_and(|cell| {
                !cell.has_flag(CellFlags::HIDDEN)
                    && (cell.is_wide_spacer() || cell.char() != ' ' || cell.is_wide())
            })
        };
        let end = if row.is_wrapped() {
            row.width()
        } else {
            (0..row.width())
                .rev()
                .find(|&col| shows(col))
                .map_or(0, |col| col + 1)
                .max(keep_columns.min(row.width()))
        };

        let capacity = usize::from(end) + 1;
        let mut text = String::with_capacity(capacity);
        let mut lengths = Vec::with_capacity(capacity);
        let mut columns = Vec::with_capacity(capacity);
        let mut widths = Vec::with_capacity(capacity);
        let mut word_starts = Vec::with_capacity(capacity);
        let mut previous_blank = match start {
            RowStart::NewLine => None,
            RowStart::After { blank } => Some(blank),
        };
        for col in 0..end {
            let Some(cell) = row.cell(col) else { break };
            if cell.is_wide_spacer() {
                continue;
            }
            let start = text.len();
            if cell.has_flag(CellFlags::HIDDEN) {
                text.push(' ');
            } else {
                push_within(&mut text, start, cell.char());
                for &c in row.grapheme_extra(col).unwrap_or(&[]) {
                    push_within(&mut text, start, c);
                }
            }
            let blank = text[start..].chars().all(char::is_whitespace);
            // A line's first character starts a word (its leading blanks
            // are a word of their own); after that, a word starts after
            // blanks. Trailing blanks belong to the word before them.
            word_starts.push(previous_blank.is_none_or(|previous| previous && !blank));
            previous_blank = Some(blank);
            // `push_within` keeps every character under 256 bytes.
            lengths.push(u8::try_from(text.len() - start).unwrap_or(u8::MAX));
            columns.push(col);
            widths.push(if cell.is_wide() { 2 } else { 1 });
        }
        if line_break {
            text.push('\n');
            lengths.push(1);
            columns.push(end);
            widths.push(0);
            word_starts.push(previous_blank.is_none());
        }
        Self {
            text,
            lengths,
            columns,
            widths,
            word_starts,
            line_break,
        }
    }

    /// Characters before the line break.
    fn visible_len(&self) -> usize {
        self.lengths.len() - usize::from(self.line_break)
    }

    /// Whether the row's last character is blank, or it has none.
    fn ends_blank(&self) -> bool {
        let count = self.visible_len();
        count == 0
            || self.text[self.byte_range(&(count - 1..count))]
                .chars()
                .all(char::is_whitespace)
    }

    /// The index of the character at column `col`: the one that covers it,
    /// or the line break (the end, without one) past the last character.
    fn char_index_at(&self, col: u16) -> usize {
        let count = self.visible_len();
        let past_end =
            count == 0 || col >= self.columns[count - 1] + u16::from(self.widths[count - 1]);
        if past_end {
            count
        } else {
            self.columns[..count]
                .partition_point(|&start| start <= col)
                .saturating_sub(1)
        }
    }

    /// The row's text runs: ranges of at most [`MAX_RUN_CHARS`] characters,
    /// and always one, empty for an empty last row.
    fn runs(&self) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        let len = self.lengths.len();
        let count = len.div_ceil(MAX_RUN_CHARS).max(1);
        (0..count).map(move |run| run * MAX_RUN_CHARS..((run + 1) * MAX_RUN_CHARS).min(len))
    }

    /// The run and the index within it of character `index`; past the end,
    /// the end of the last run.
    fn run_position(&self, index: usize) -> (usize, usize) {
        let len = self.lengths.len();
        if index >= len {
            let last = len.div_ceil(MAX_RUN_CHARS).max(1) - 1;
            (last, len - last * MAX_RUN_CHARS)
        } else {
            (index / MAX_RUN_CHARS, index % MAX_RUN_CHARS)
        }
    }

    /// The byte range of characters `chars` in the text.
    fn byte_range(&self, chars: &std::ops::Range<usize>) -> std::ops::Range<usize> {
        let start: usize = self.lengths[..chars.start]
            .iter()
            .map(|&l| usize::from(l))
            .sum();
        let len: usize = self.lengths[chars.clone()]
            .iter()
            .map(|&l| usize::from(l))
            .sum();
        start..start + len
    }
}

/// Append `c` to the character that began at `start` in `text`, unless
/// that would make it longer than AccessKit's per-character limit of 255
/// bytes. Cells hold at most 17 codepoints, so this never drops one today.
fn push_within(text: &mut String, start: usize, c: char) {
    if text.len() - start + c.len_utf8() <= usize::from(u8::MAX) {
        text.push(c);
    }
}

/// The full tree for a window titled `window_title` showing `snapshot`.
///
/// Focus is on the focused pane's `Terminal` node; on a full-scene app's
/// focused region, or its root with none; or on the window when no pane
/// has a grid or a scene.
pub fn build(window_title: &str, snapshot: &Snapshot) -> TreeUpdate {
    let tab_list_id = node_id("tabs");
    let panel_id = node_id("panel");
    let mut out = Emitter::default();
    let mut focus = WINDOW_ID;

    let mut window = Node::new(Role::Window);
    window.set_label(window_title);
    window.set_children(vec![tab_list_id, panel_id]);
    out.push_fixed(WINDOW_ID, window);

    let mut tab_list = Node::new(Role::TabList);
    tab_list.set_children(
        snapshot
            .tabs
            .iter()
            .map(|tab| tab_node_id(tab.id))
            .collect::<Vec<_>>(),
    );
    out.push_fixed(tab_list_id, tab_list);
    let mut panel = Node::new(Role::TabPanel);
    for tab in &snapshot.tabs {
        let mut node = Node::new(Role::Tab);
        node.set_label(tab.title.as_str());
        node.set_selected(tab.selected);
        if tab.selected {
            node.set_controls(vec![panel_id]);
            panel.set_label(tab.title.as_str());
        }
        out.push_fixed(tab_node_id(tab.id), node);
    }

    panel.set_children(
        snapshot
            .panes
            .iter()
            .map(|pane| pane_node_id(pane.id))
            .collect::<Vec<_>>(),
    );
    out.push_fixed(panel_id, panel);
    // Every pane's node, before any app's node can take its id.
    for pane in &snapshot.panes {
        out.reserve(pane_node_id(pane.id));
    }

    for pane in &snapshot.panes {
        let id = pane_node_id(pane.id);
        let mut budget = MAX_PANE_NODES;
        out.truncated = false;
        let scenes: Vec<&SceneSnapshot> = pane.scenes.iter().filter(|s| !s.is_empty()).collect();
        if !scenes.is_empty() {
            let mut children: Vec<NodeId> = scenes
                .iter()
                .filter_map(|scene| scene.emit(&mut out, &mut budget, None))
                .collect();
            push_truncation_note(&mut out, pane.id, &mut children);
            // The app drawn last is on top; with several, the last one
            // with a focused region has the keyboard. Focus must be on a
            // node that was built.
            focus = scenes
                .iter()
                .rev()
                .find_map(|scene| scene.focus_node_id())
                .or_else(|| scenes.last().map(|scene| scene.root_node_id()))
                .filter(|&focus| out.contains(focus))
                .unwrap_or(id);
            let mut group = Node::new(Role::Group);
            group.set_label(pane.title.as_str());
            group.set_bounds(pane.bounds);
            group.set_children(children);
            out.push_fixed(id, group);
            continue;
        }
        let Some(grid) = &pane.grid else {
            let mut group = Node::new(Role::Group);
            group.set_label(pane.title.as_str());
            group.set_bounds(pane.bounds);
            out.push_fixed(id, group);
            continue;
        };
        let mut terminal = Node::new(Role::Terminal);
        terminal.set_label(pane.title.as_str());
        terminal.set_bounds(pane.bounds);
        let caret = grid.caret.and_then(|(row, index)| {
            let (run, character_index) = grid.rows.get(row)?.run_position(index);
            Some(TextPosition {
                node: run_node_id(pane.id, row, run),
                character_index,
            })
        });
        if let Some(caret) = caret {
            terminal.set_text_selection(TextSelection {
                anchor: caret,
                focus: caret,
            });
        }
        // The rows first, so no surface's node can take a run's id.
        let mut row_runs = Vec::with_capacity(grid.rows.len());
        for (index, row) in grid.rows.iter().enumerate() {
            let top = pane.bounds.y0 + index as f64 * grid.cell_height;
            let ranges: Vec<_> = row.runs().collect();
            let last = ranges.len() - 1;
            let mut runs = Vec::with_capacity(ranges.len());
            for (run, chars) in ranges.into_iter().enumerate() {
                let first_col = match run {
                    0 => 0,
                    _ => row.columns.get(chars.start).copied().unwrap_or(0),
                };
                let right = match row.columns.get(chars.end) {
                    Some(&col) if run < last => pane.bounds.x0 + f64::from(col) * grid.cell_width,
                    _ => pane.bounds.x1,
                };
                let bounds = Rect {
                    x0: pane.bounds.x0 + f64::from(first_col) * grid.cell_width,
                    y0: top,
                    x1: right,
                    y1: top + grid.cell_height,
                };
                let mut node = text_run(row, chars, bounds, first_col, grid.cell_width);
                if run > 0 {
                    node.set_previous_on_line(run_node_id(pane.id, index, run - 1));
                }
                if run < last {
                    node.set_next_on_line(run_node_id(pane.id, index, run + 1));
                }
                let run_id = run_node_id(pane.id, index, run);
                out.push_fixed(run_id, node);
                runs.push(run_id);
            }
            row_runs.push(runs);
        }
        // Then the surfaces, in reading order: each after its anchor row
        // and described by it (the fallback text), one that starts above
        // the screen first, one below the last row last.
        let mut surfaces: Vec<&SurfaceSnapshot> = pane
            .surfaces
            .iter()
            .filter(|s| !s.scene.is_empty())
            .collect();
        surfaces.sort_by_key(|surface| surface.row);
        let mut placed: Vec<(i64, NodeId)> = Vec::with_capacity(surfaces.len());
        for surface in surfaces {
            let anchor = usize::try_from(surface.row)
                .ok()
                .filter(|&row| row < grid.rows.len())
                .map(|row| row_node_id(pane.id, row));
            if let Some(root) = surface.scene.emit(&mut out, &mut budget, anchor) {
                placed.push((surface.row, root));
            }
        }
        let mut placed = placed.into_iter().peekable();
        let mut children = Vec::with_capacity(grid.rows.len() + placed.len());
        while let Some((_, root)) = placed.next_if(|&(row, _)| row < 0) {
            children.push(root);
        }
        for (index, runs) in row_runs.into_iter().enumerate() {
            children.extend(runs);
            while let Some((_, root)) = placed.next_if(|&(row, _)| row == index as i64) {
                children.push(root);
            }
        }
        children.extend(placed.map(|(_, root)| root));
        push_truncation_note(&mut out, pane.id, &mut children);
        terminal.set_children(children);
        out.push_fixed(id, terminal);
        focus = id;
    }

    TreeUpdate {
        nodes: out.finish(),
        tree: Some(TreeInfo::new(WINDOW_ID)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

/// When the pane's native nodes were cut to the budget, end its children
/// with a label that says so.
fn push_truncation_note(out: &mut Emitter, pane: usize, children: &mut Vec<NodeId>) {
    let id = node_id(&format!("p{pane}truncated"));
    if out.truncated && !out.contains(id) {
        let mut note = Node::new(Role::Label);
        note.set_value("… truncated");
        out.push_fixed(id, note);
        children.push(id);
    }
}

/// A text run of characters `chars` of `row`, at `bounds`, whose first
/// character is in column `first_col` of cells `cell_width` wide.
fn text_run(
    row: &RowText,
    chars: std::ops::Range<usize>,
    bounds: Rect,
    first_col: u16,
    cell_width: f64,
) -> Node {
    let mut node = Node::new(Role::TextRun);
    node.set_value(&row.text[row.byte_range(&chars)]);
    node.set_text_direction(TextDirection::LeftToRight);
    node.set_bounds(bounds);
    node.set_character_lengths(&row.lengths[chars.clone()]);
    node.set_character_positions(
        row.columns[chars.clone()]
            .iter()
            .map(|&col| (f64::from(col - first_col) * cell_width) as f32)
            .collect::<Vec<_>>(),
    );
    node.set_character_widths(
        row.widths[chars.clone()]
            .iter()
            .map(|&width| (f64::from(width) * cell_width) as f32)
            .collect::<Vec<_>>(),
    );
    node.set_word_starts(
        row.word_starts[chars]
            .iter()
            .enumerate()
            // A run holds at most 255 characters, so its indices fit.
            .filter_map(|(at, &start)| start.then(|| u8::try_from(at).ok()).flatten())
            .collect::<Vec<_>>(),
    );
    node
}

/// A tab's node, `t<id>`.
pub fn tab_node_id(tab: usize) -> NodeId {
    node_id(&format!("t{tab}"))
}

/// A pane's node, `p<id>`.
pub fn pane_node_id(pane: usize) -> NodeId {
    node_id(&format!("p{pane}"))
}

/// Screen row `row` of a pane, `p<id>r<row>`: the row's first text run.
pub fn row_node_id(pane: usize, row: usize) -> NodeId {
    node_id(&format!("p{pane}r{row}"))
}

/// Text run `run` of a screen row: the row's own id for the first, and
/// `p<id>r<row>s<run>` for the rest of a row past 255 characters.
pub fn run_node_id(pane: usize, row: usize, run: usize) -> NodeId {
    if run == 0 {
        row_node_id(pane, row)
    } else {
        node_id(&format!("p{pane}r{row}s{run}"))
    }
}

/// Region `region` of native session `session`, `s<session>:<region>`.
pub fn region_node_id(session: u64, region: u32) -> NodeId {
    node_id(&format!("s{session}:{region}"))
}

/// Line `line` of region `region`'s text, `s<session>:<region>l<line>`.
pub fn region_line_node_id(session: u64, region: u32, line: usize) -> NodeId {
    node_id(&format!("s{session}:{region}l{line}"))
}

/// The `TextRun` under region `region`'s text input, which holds its text
/// for the caret to sit in, `s<session>:<region>i`.
pub fn region_input_run_node_id(session: u64, region: u32) -> NodeId {
    node_id(&format!("s{session}:{region}i"))
}

/// Announcement `serial` of live region `region`,
/// `s<session>:<region>a<serial>` ([`super::live`]).
pub fn region_announcement_node_id(session: u64, region: u32, serial: u64) -> NodeId {
    node_id(&format!("s{session}:{region}a{serial}"))
}

/// The id of the node named `name`: its 64-bit FNV-1a hash, which is the
/// same in every run, kept off the window's id.
fn node_id(name: &str) -> NodeId {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = name.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    });
    NodeId(hash.max(WINDOW_ID.0 + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Dimensions, Terminal, TerminalHandler};
    use crate::protocol::ansi::AnsiParser;

    const CELL_W: f64 = 10.0;
    const CELL_H: f64 = 20.0;

    /// The screen of a `cols` by `rows` terminal after `bytes`.
    fn screen(cols: u16, rows: u16, bytes: &str) -> GridSnapshot {
        let mut terminal = Terminal::new(Dimensions::new(cols, rows));
        let mut parser = AnsiParser::new();
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(bytes.as_bytes(), |action| handler.handle(action));
        GridSnapshot::capture(terminal.grid(), CELL_W, CELL_H)
    }

    fn pane(id: usize, title: &str, grid: Option<GridSnapshot>) -> PaneSnapshot {
        PaneSnapshot {
            id,
            title: title.to_owned(),
            bounds: Rect {
                x0: 0.0,
                y0: 40.0,
                x1: 200.0,
                y1: 140.0,
            },
            grid,
            scenes: Vec::new(),
            surfaces: Vec::new(),
        }
    }

    fn tab(id: usize, title: &str, selected: bool) -> TabSnapshot {
        TabSnapshot {
            id,
            title: title.to_owned(),
            selected,
        }
    }

    /// One tab with the focused pane 1 showing `grid`.
    fn one_pane(grid: GridSnapshot) -> Snapshot {
        Snapshot {
            tabs: vec![tab(1, "zsh", true)],
            panes: vec![pane(1, "zsh", Some(grid))],
        }
    }

    fn node(update: &TreeUpdate, id: NodeId) -> &Node {
        &update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .unwrap_or_else(|| panic!("no node {id:?}"))
            .1
    }

    fn caret(update: &TreeUpdate, pane: usize) -> TextPosition {
        let selection = node(update, pane_node_id(pane))
            .text_selection()
            .expect("the terminal has a caret");
        assert_eq!(selection.anchor, selection.focus, "a caret, not a range");
        selection.focus
    }

    #[test]
    fn ascii_rows_are_text_runs_one_column_per_character() {
        let update = build("Ultty", &one_pane(screen(10, 2, "ab c\r\nxy")));
        let first = node(&update, row_node_id(1, 0));

        assert_eq!(first.role(), Role::TextRun);
        assert_eq!(first.value(), Some("ab c\n"));
        assert_eq!(first.character_lengths(), &[1, 1, 1, 1, 1]);
        assert_eq!(
            first.character_positions(),
            Some([0.0, 10.0, 20.0, 30.0, 40.0].as_slice())
        );
        assert_eq!(
            first.character_widths(),
            Some([10.0, 10.0, 10.0, 10.0, 0.0].as_slice())
        );
        assert_eq!(first.word_starts(), &[0, 3]);
        assert_eq!(
            first.bounds(),
            Some(Rect {
                x0: 0.0,
                y0: 40.0,
                x1: 200.0,
                y1: 60.0
            })
        );
        // The last row has no line break to end it.
        let last = node(&update, row_node_id(1, 1));
        assert_eq!(last.value(), Some("xy"));
        assert_eq!(last.character_lengths(), &[1, 1]);
        assert_eq!(last.bounds().map(|bounds| bounds.y0), Some(60.0));
    }

    #[test]
    fn a_wide_character_is_one_character_spanning_two_columns() {
        let update = build("Ultty", &one_pane(screen(10, 2, "a中b")));
        let row = node(&update, row_node_id(1, 0));

        assert_eq!(row.value(), Some("a中b\n"));
        assert_eq!(row.character_lengths(), &[1, 3, 1, 1]);
        assert_eq!(
            row.character_positions(),
            Some([0.0, 10.0, 30.0, 40.0].as_slice())
        );
        assert_eq!(
            row.character_widths(),
            Some([10.0, 20.0, 10.0, 0.0].as_slice())
        );
    }

    #[test]
    fn combining_marks_join_their_base_character() {
        let update = build("Ultty", &one_pane(screen(10, 2, "e\u{301}x")));
        let row = node(&update, row_node_id(1, 0));

        assert_eq!(row.value(), Some("e\u{301}x\n"));
        assert_eq!(row.character_lengths(), &[3, 1, 1]);
        assert_eq!(
            row.character_positions(),
            Some([0.0, 10.0, 20.0].as_slice())
        );
        assert_eq!(row.character_widths(), Some([10.0, 10.0, 0.0].as_slice()));
    }

    #[test]
    fn concealed_text_reads_as_blanks() {
        let update = build("Ultty", &one_pane(screen(10, 2, "pw:\x1b[8mabc\x1b[0m!")));
        assert_eq!(node(&update, row_node_id(1, 0)).value(), Some("pw:   !\n"));
    }

    #[test]
    fn a_soft_wrapped_row_continues_its_line() {
        let update = build("Ultty", &one_pane(screen(4, 3, "abcdef")));
        assert_eq!(node(&update, row_node_id(1, 0)).value(), Some("abcd"));
        assert_eq!(node(&update, row_node_id(1, 1)).value(), Some("ef\n"));
    }

    #[test]
    fn a_blank_at_the_wrap_column_keeps_the_words_apart() {
        let update = build("Ultty", &one_pane(screen(4, 3, "abc def")));
        let first = node(&update, row_node_id(1, 0));
        let second = node(&update, row_node_id(1, 1));

        assert_eq!(first.value(), Some("abc "));
        assert_eq!(first.word_starts(), &[0]);
        assert_eq!(second.value(), Some("def\n"));
        // "def" starts a word: the row before ended in a blank.
        assert_eq!(second.word_starts(), &[0]);
    }

    #[test]
    fn a_word_running_on_across_a_wrap_starts_no_word_on_the_next_row() {
        let update = build("Ultty", &one_pane(screen(4, 3, "ab cdef gh")));
        assert_eq!(node(&update, row_node_id(1, 0)).word_starts(), &[0, 3]);
        let middle = node(&update, row_node_id(1, 1));
        assert_eq!(middle.value(), Some("def "));
        assert!(
            middle.word_starts().is_empty(),
            "{:?}",
            middle.word_starts()
        );
        // After the blank that ended the row before, "gh" starts a word.
        assert_eq!(node(&update, row_node_id(1, 2)).word_starts(), &[0]);
    }

    #[test]
    fn a_wide_character_that_did_not_fit_leaves_a_blank_before_the_wrap() {
        // The grid leaves the last column an ordinary blank cell, so it is
        // read as a space, as copying the text gives it.
        let update = build("Ultty", &one_pane(screen(4, 3, "abc中")));
        assert_eq!(node(&update, row_node_id(1, 0)).value(), Some("abc "));
        assert_eq!(node(&update, row_node_id(1, 1)).value(), Some("中\n"));
    }

    #[test]
    fn a_caret_with_a_wrap_pending_is_past_the_last_column() {
        let update = build("Ultty", &one_pane(screen(4, 2, "abcd")));
        assert_eq!(
            caret(&update, 1),
            TextPosition {
                node: row_node_id(1, 0),
                character_index: 4
            }
        );
    }

    #[test]
    fn a_row_past_255_characters_is_split_into_runs_on_one_line() {
        let text = "word ".repeat(60);
        let update = build("Ultty", &one_pane(screen(300, 2, &text)));
        let first = node(&update, row_node_id(1, 0));
        let second = node(&update, run_node_id(1, 0, 1));

        assert_eq!(
            node(&update, pane_node_id(1)).children(),
            &[row_node_id(1, 0), run_node_id(1, 0, 1), row_node_id(1, 1)]
        );
        assert_eq!(first.value(), Some(&text[..255]));
        assert_eq!(first.character_lengths().len(), 255);
        assert_eq!(first.next_on_line(), Some(run_node_id(1, 0, 1)));
        assert_eq!(first.previous_on_line(), None);
        assert_eq!(first.word_starts().last(), Some(&250));

        // The rest of the row and its line break, positioned from the run's
        // own left edge at column 255.
        assert_eq!(second.value(), Some(format!("{}\n", &text[255..]).as_str()));
        assert_eq!(second.previous_on_line(), Some(row_node_id(1, 0)));
        assert_eq!(second.next_on_line(), None);
        assert_eq!(
            second.bounds().map(|bounds| bounds.x0),
            Some(255.0 * CELL_W)
        );
        assert_eq!(second.character_positions().map(|p| p[0]), Some(0.0));
        assert_eq!(second.word_starts(), &[0, 5, 10, 15, 20, 25, 30, 35, 40]);
        // The cursor, wrap pending after character 300, sits on the break.
        assert_eq!(
            caret(&update, 1),
            TextPosition {
                node: run_node_id(1, 0, 1),
                character_index: 45
            }
        );

        let tree = accesskit_consumer::Tree::new(update, true);
        let state = tree.state();
        let terminal = state.focus().expect("a focused node");
        assert_eq!(terminal.document_range().text(), format!("{text}\n"));
        let caret = terminal.text_selection_focus().expect("a caret");
        assert_eq!(caret.to_global_usv_index(), 300);
        assert_eq!(caret.to_line_index(), 0);
    }

    #[test]
    fn the_caret_follows_the_cursor() {
        let update = build("Ultty", &one_pane(screen(10, 3, "one\r\ntwo\x1b[2D")));
        assert_eq!(
            caret(&update, 1),
            TextPosition {
                node: row_node_id(1, 1),
                character_index: 1
            }
        );
    }

    #[test]
    fn a_caret_after_trailing_blanks_keeps_them_and_sits_on_the_line_break() {
        let update = build("Ultty", &one_pane(screen(10, 3, "$ ")));
        let row = node(&update, row_node_id(1, 0));

        assert_eq!(row.value(), Some("$ \n"));
        assert_eq!(
            caret(&update, 1),
            TextPosition {
                node: row_node_id(1, 0),
                character_index: 2
            }
        );
    }

    #[test]
    fn a_caret_at_the_end_of_the_last_row_is_past_its_last_character() {
        let update = build("Ultty", &one_pane(screen(10, 2, "\r\nab")));
        assert_eq!(
            caret(&update, 1),
            TextPosition {
                node: row_node_id(1, 1),
                character_index: 2
            }
        );
    }

    #[test]
    fn a_caret_on_either_half_of_a_wide_character_is_on_that_character() {
        // Cursor on the second column of 中, on its first, then after b
        // (the line break, index 3).
        for (bytes, expected) in [("a中b\x1b[3G", 1), ("a中b\x1b[2G", 1), ("a中b", 3)] {
            let update = build("Ultty", &one_pane(screen(10, 2, bytes)));
            assert_eq!(
                caret(&update, 1).character_index,
                expected,
                "cursor after {bytes:?}"
            );
        }
    }

    #[test]
    fn tabs_and_panes_form_a_tab_list_and_the_selected_tabs_panel() {
        let snapshot = Snapshot {
            tabs: vec![tab(1, "zsh", false), tab(2, "vim notes", true)],
            panes: vec![
                pane(4, "vim notes", Some(screen(10, 2, "hi"))),
                pane(5, "htop", None),
            ],
        };
        let update = build("vim notes", &snapshot);

        let window = node(&update, WINDOW_ID);
        assert_eq!(window.role(), Role::Window);
        assert_eq!(window.label(), Some("vim notes"));
        let [tab_list, panel] = window.children() else {
            panic!("window children: {:?}", window.children());
        };
        let tab_list = node(&update, *tab_list);
        assert_eq!(tab_list.role(), Role::TabList);
        assert_eq!(tab_list.children(), &[tab_node_id(1), tab_node_id(2)]);
        let first = node(&update, tab_node_id(1));
        assert_eq!(
            (first.role(), first.label(), first.is_selected()),
            (Role::Tab, Some("zsh"), Some(false))
        );
        let second = node(&update, tab_node_id(2));
        assert_eq!(
            (second.label(), second.is_selected()),
            (Some("vim notes"), Some(true))
        );
        assert_eq!(second.controls(), &[*panel]);

        let panel = node(&update, *panel);
        assert_eq!(panel.role(), Role::TabPanel);
        assert_eq!(panel.label(), Some("vim notes"));
        assert_eq!(panel.children(), &[pane_node_id(4), pane_node_id(5)]);
        let focused = node(&update, pane_node_id(4));
        assert_eq!(focused.role(), Role::Terminal);
        assert_eq!(focused.label(), Some("vim notes"));
        assert_eq!(focused.children(), &[row_node_id(4, 0), row_node_id(4, 1)]);
        assert_eq!(update.focus, pane_node_id(4));
    }

    #[test]
    fn an_unfocused_pane_is_a_labelled_group_with_no_children() {
        let snapshot = Snapshot {
            tabs: vec![tab(1, "zsh", true)],
            panes: vec![
                pane(1, "zsh", Some(screen(10, 2, ""))),
                pane(2, "htop", None),
            ],
        };
        let update = build("zsh", &snapshot);
        let group = node(&update, pane_node_id(2));

        assert_eq!(group.role(), Role::Group);
        assert_eq!(group.label(), Some("htop"));
        assert!(group.children().is_empty());
        assert!(update.nodes.iter().all(|(id, _)| *id != row_node_id(2, 0)));
    }

    #[test]
    fn with_no_focused_grid_focus_rests_on_the_window() {
        let update = build("Ultty", &Snapshot::default());
        assert_eq!(update.focus, WINDOW_ID);
        assert_eq!(update.tree.as_ref().map(|tree| tree.root), Some(WINDOW_ID));
        assert_eq!(update.tree_id, TreeId::ROOT);
    }

    #[test]
    fn rebuilding_the_same_screen_gives_the_same_nodes() {
        let snapshot = one_pane(screen(10, 3, "same\r\n"));
        let first = build("Ultty", &snapshot);
        let again = build("Ultty", &snapshot);

        assert_eq!(first.nodes, again.nodes);
        assert_eq!(first.focus, again.focus);
    }

    #[test]
    fn node_ids_are_fixed_and_distinct() {
        // The same name gives the same id in every run.
        assert_eq!(tab_node_id(1), node_id("t1"));
        assert_eq!(node_id("t1"), NodeId(0x08c7_ff07_b56a_5e16));
        let snapshot = Snapshot {
            tabs: (0..20).map(|id| tab(id, "t", id == 0)).collect(),
            panes: vec![pane(0, "p", Some(screen(10, 50, "")))],
        };
        let update = build("Ultty", &snapshot);
        let mut ids: Vec<NodeId> = update.nodes.iter().map(|(id, _)| *id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn a_full_screen_tree_is_accepted_by_accesskit() {
        // The consumer panics on a malformed tree (an orphan or a duplicate
        // child); feed it a split screen with wide and combining text.
        let snapshot = Snapshot {
            tabs: vec![tab(1, "a", true), tab(2, "b", false)],
            panes: vec![
                pane(1, "a", Some(screen(8, 4, "a中\u{301}b\r\n  x"))),
                pane(2, "b", None),
            ],
        };
        let tree = accesskit_consumer::Tree::new(build("Ultty", &snapshot), true);
        let state = tree.state();
        let focus = state.focus().expect("a focused node");
        assert_eq!(focus.role(), Role::Terminal);
        assert!(focus.supports_text_ranges());
        // What a screen reader reads: the screen, line by line.
        assert_eq!(focus.document_range().text(), "a中\u{301}b\n  x\n\n");
        // The caret after "x", on the second row's line break.
        let caret = focus.text_selection_focus().expect("a caret");
        assert_eq!(
            caret.to_global_usv_index(),
            "a中\u{301}b\n  x".chars().count()
        );
    }
}
