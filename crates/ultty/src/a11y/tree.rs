//! The AccessKit tree Ultty exposes, built without any platform types.
//!
//! ```text
//! Window (the window title)
//! ├── TabList
//! │   └── Tab, one per tab, the active one selected
//! └── TabPanel (the active tab's title)
//!     ├── Terminal (the focused pane's title): one TextRun per screen row,
//!     │   the caret at the cursor; focus rests here
//!     └── Group (each other pane's title), no children
//! ```
//!
//! The grid is the live screen: scrollback, per-command nodes (OSC 133),
//! the selection and native scenes are later slices of ADR 0021.
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

use crate::grid::{CellFlags, Grid, Row};

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
    /// The screen's text, for the focused pane only.
    pub grid: Option<GridSnapshot>,
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

/// One screen row as a text run: its characters (a cell's whole grapheme
/// each, a wide character once), the columns they start at and how many
/// they span, and the line break unless the row soft-wraps.
#[derive(Debug, Clone, PartialEq)]
struct RowText {
    text: String,
    /// UTF-8 length of each character, the line break included.
    lengths: Vec<u8>,
    /// First column of each character; the line break sits after the last.
    columns: Vec<u16>,
    /// Columns each character spans; 0 for the line break.
    widths: Vec<u8>,
    line_break: bool,
}

impl GridSnapshot {
    /// Copy the screen rows and cursor of `grid`, whose cells are
    /// `cell_width` by `cell_height` pixels. One pass over the cells: this
    /// runs with the terminal locked.
    pub fn capture(grid: &Grid, cell_width: f64, cell_height: f64) -> Self {
        let dims = grid.dimensions();
        let cursor = grid.cursor().position;
        let mut caret = None;
        let rows = (0..dims.rows)
            .filter_map(|index| {
                let row = grid.row(index)?;
                let line_break = index + 1 < dims.rows && !row.is_wrapped();
                if index == cursor.row {
                    // Keep the blanks before the cursor, so the caret after
                    // a prompt's trailing space lands after it.
                    let text = RowText::capture(row, cursor.col, line_break);
                    caret = Some((usize::from(index), text.char_index_at(cursor.col)));
                    Some(text)
                } else {
                    Some(RowText::capture(row, 0, line_break))
                }
            })
            .collect();
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
    /// `keep_columns` if that is further. A hidden (concealed) cell reads as
    /// a blank, as it is drawn.
    fn capture(row: &Row, keep_columns: u16, line_break: bool) -> Self {
        let shows = |col: u16| {
            row.get(col).is_some_and(|cell| {
                !cell.has_flag(CellFlags::HIDDEN)
                    && (cell.is_wide_spacer() || cell.char() != ' ' || cell.is_wide())
            })
        };
        let end = (0..row.width())
            .rev()
            .find(|&col| shows(col))
            .map_or(0, |col| col + 1)
            .max(keep_columns.min(row.width()));

        let mut text = String::with_capacity(usize::from(end) + 1);
        let mut lengths = Vec::with_capacity(usize::from(end) + 1);
        let mut columns = Vec::with_capacity(usize::from(end) + 1);
        let mut widths = Vec::with_capacity(usize::from(end) + 1);
        for col in 0..end {
            let Some(cell) = row.get(col) else { break };
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
        }
        Self {
            text,
            lengths,
            columns,
            widths,
            line_break,
        }
    }

    /// Characters before the line break.
    fn visible_len(&self) -> usize {
        self.lengths.len() - usize::from(self.line_break)
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

    /// Where words start: after each run of blanks, and at 0 (a line's
    /// leading blanks are a word of their own). Trailing blanks and the line
    /// break belong to the word before them.
    fn word_starts(&self) -> Vec<u8> {
        let mut starts = Vec::new();
        let mut previous_blank = true;
        for (index, (offset, &len)) in self.char_offsets().zip(&self.lengths).enumerate() {
            let blank = self.text[offset..offset + usize::from(len)]
                .chars()
                .all(char::is_whitespace);
            if index == 0 || (previous_blank && !blank) {
                // `word_starts` is a list of u8; a row past 255 characters
                // keeps the word boundaries that fit.
                match u8::try_from(index) {
                    Ok(index) => starts.push(index),
                    Err(_) => break,
                }
            }
            previous_blank = blank;
        }
        starts
    }

    fn char_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.lengths.iter().scan(0, |offset, &len| {
            let start = *offset;
            *offset += usize::from(len);
            Some(start)
        })
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
/// Focus is on the focused pane's `Terminal` node, or on the window when no
/// pane has a grid.
pub fn build(window_title: &str, snapshot: &Snapshot) -> TreeUpdate {
    let tab_list_id = node_id("tabs");
    let panel_id = node_id("panel");
    let mut nodes = Vec::new();
    let mut focus = WINDOW_ID;

    let mut window = Node::new(Role::Window);
    window.set_label(window_title);
    window.set_children(vec![tab_list_id, panel_id]);
    nodes.push((WINDOW_ID, window));

    let mut tab_list = Node::new(Role::TabList);
    tab_list.set_children(
        snapshot
            .tabs
            .iter()
            .map(|tab| tab_node_id(tab.id))
            .collect::<Vec<_>>(),
    );
    nodes.push((tab_list_id, tab_list));
    let mut panel = Node::new(Role::TabPanel);
    for tab in &snapshot.tabs {
        let mut node = Node::new(Role::Tab);
        node.set_label(tab.title.as_str());
        node.set_selected(tab.selected);
        if tab.selected {
            node.set_controls(vec![panel_id]);
            panel.set_label(tab.title.as_str());
        }
        nodes.push((tab_node_id(tab.id), node));
    }

    panel.set_children(
        snapshot
            .panes
            .iter()
            .map(|pane| pane_node_id(pane.id))
            .collect::<Vec<_>>(),
    );
    nodes.push((panel_id, panel));

    for pane in &snapshot.panes {
        let id = pane_node_id(pane.id);
        let Some(grid) = &pane.grid else {
            let mut group = Node::new(Role::Group);
            group.set_label(pane.title.as_str());
            group.set_bounds(pane.bounds);
            nodes.push((id, group));
            continue;
        };
        let mut terminal = Node::new(Role::Terminal);
        terminal.set_label(pane.title.as_str());
        terminal.set_bounds(pane.bounds);
        let row_ids: Vec<NodeId> = (0..grid.rows.len())
            .map(|row| row_node_id(pane.id, row))
            .collect();
        if let Some((row, index)) = grid.caret {
            let caret = TextPosition {
                node: row_ids[row],
                character_index: index,
            };
            terminal.set_text_selection(TextSelection {
                anchor: caret,
                focus: caret,
            });
        }
        terminal.set_children(row_ids.clone());
        nodes.push((id, terminal));
        focus = id;

        for (index, (row, row_id)) in grid.rows.iter().zip(row_ids).enumerate() {
            let top = pane.bounds.y0 + index as f64 * grid.cell_height;
            let mut run = Node::new(Role::TextRun);
            run.set_value(row.text.as_str());
            run.set_text_direction(TextDirection::LeftToRight);
            run.set_bounds(Rect {
                x0: pane.bounds.x0,
                y0: top,
                x1: pane.bounds.x1,
                y1: top + grid.cell_height,
            });
            run.set_character_lengths(row.lengths.as_slice());
            run.set_character_positions(
                row.columns
                    .iter()
                    .map(|&col| (f64::from(col) * grid.cell_width) as f32)
                    .collect::<Vec<_>>(),
            );
            run.set_character_widths(
                row.widths
                    .iter()
                    .map(|&width| (f64::from(width) * grid.cell_width) as f32)
                    .collect::<Vec<_>>(),
            );
            run.set_word_starts(row.word_starts());
            nodes.push((row_id, run));
        }
    }

    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(WINDOW_ID)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

/// A tab's node, `t<id>`.
pub fn tab_node_id(tab: usize) -> NodeId {
    node_id(&format!("t{tab}"))
}

/// A pane's node, `p<id>`.
pub fn pane_node_id(pane: usize) -> NodeId {
    node_id(&format!("p{pane}"))
}

/// Screen row `row` of a pane, `p<id>r<row>`.
pub fn row_node_id(pane: usize, row: usize) -> NodeId {
    node_id(&format!("p{pane}r{row}"))
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
