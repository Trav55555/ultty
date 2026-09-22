//! Table widget with column headers and row data.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row as RatatuiRow, Table as RatatuiTable};
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Constraints, Dimension, Display, FlexDirection, RegionId, Style as ProtoStyle,
    TextContent,
};

/// A table with headers and selectable rows.
#[derive(Default)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    selected: usize,
    column_widths: Vec<f32>,
}

impl std::fmt::Debug for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Table")
            .field("headers", &self.headers)
            .field("rows_count", &self.rows.len())
            .field("selected", &self.selected)
            .finish()
    }
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn headers(mut self, headers: Vec<impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(Into::into).collect();
        self
    }

    pub fn rows(mut self, rows: Vec<Vec<impl Into<String>>>) -> Self {
        self.rows = rows
            .into_iter()
            .map(|row| row.into_iter().map(Into::into).collect())
            .collect();
        self
    }

    pub fn column_widths(mut self, widths: Vec<f32>) -> Self {
        self.column_widths = widths;
        self
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_row(&self) -> Option<&Vec<String>> {
        self.rows.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        if delta > 0 {
            self.selected = (self.selected + delta as usize).min(self.rows.len() - 1);
        } else {
            let abs = (-delta) as usize;
            self.selected = self.selected.saturating_sub(abs);
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        if !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
        }
    }

    /// Fixed width for `column`, if one was given via `column_widths`.
    /// Columns without a width share the remaining space equally.
    fn column_width(&self, column: usize) -> Option<f32> {
        self.column_widths.get(column).copied()
    }

    fn column_count(&self) -> usize {
        self.headers
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
    }

    fn row_view<'a>(&self, cells: &'a [String], style: ProtoStyle) -> TableRow<'a> {
        TableRow {
            cells: cells
                .iter()
                .enumerate()
                .map(|(col, text)| TableCell {
                    text,
                    width: self.column_width(col),
                })
                .collect(),
            style,
        }
    }
}

/// Header row style, shared by both render modes.
fn header_style(theme: &Theme) -> ProtoStyle {
    ProtoStyle::new().fg(theme.primary).bold()
}

/// Selected data row style, shared by both render modes.
fn selected_row_style(theme: &Theme) -> ProtoStyle {
    ProtoStyle::new()
        .fg(theme.selection.fg)
        .bg(theme.selection.bg)
        .bold()
}

fn to_ratatui_style(style: &ProtoStyle) -> Style {
    let mut out = Style::default();
    if let Some(fg) = style.fg {
        out = out.fg(to_ratatui_color(fg));
    }
    if let Some(bg) = style.bg {
        out = out.bg(to_ratatui_color(bg));
    }
    if style.bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    out
}

/// One native table row: a flex row of `TableCell`s.
struct TableRow<'a> {
    cells: Vec<TableCell<'a>>,
    style: ProtoStyle,
}

impl View for TableRow<'_> {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            width: Dimension::Percent(100.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        self.style.clone()
    }

    fn children_count(&self) -> usize {
        self.cells.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        match self.cells.get(index) {
            Some(cell) => Ok(Some(super::render_view_to_native(
                cell,
                send_message,
                parent,
                next_id,
                None,
            )?)),
            None => Ok(None),
        }
    }
}

/// One native table cell: fixed width when the column has one, else flex.
struct TableCell<'a> {
    text: &'a str,
    width: Option<f32>,
}

impl View for TableCell<'_> {
    fn constraints(&self) -> Constraints {
        match self.width {
            Some(width) => Constraints {
                display: Display::Flex,
                width: Dimension::Cells(width),
                flex_shrink: 0.0,
                ..Default::default()
            },
            None => Constraints {
                display: Display::Flex,
                flex_grow: 1.0,
                ..Default::default()
            },
        }
    }

    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::Text(TextContent::new(self.text))
    }
}

impl View for Table {
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::new().border(ultty_protocol::BorderStyle::thin())
    }

    fn children_count(&self) -> usize {
        // header row + data rows
        1 + self.rows.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let theme = Theme::current();
        let row = if index == 0 {
            self.row_view(&self.headers, header_style(&theme))
        } else {
            let row_idx = index - 1;
            let Some(cells) = self.rows.get(row_idx) else {
                return Ok(None);
            };
            let style = if row_idx == self.selected {
                selected_row_style(&theme)
            } else {
                ProtoStyle::default()
            };
            self.row_view(cells, style)
        };
        Ok(Some(super::render_view_to_native(
            &row,
            send_message,
            parent,
            next_id,
            None,
        )?))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let widths: Vec<Constraint> = (0..self.column_count())
            .map(|col| match self.column_width(col) {
                Some(width) => Constraint::Length(width as u16),
                None => Constraint::Fill(1),
            })
            .collect();

        let header_cells: Vec<Cell> = self
            .headers
            .iter()
            .map(|h| Cell::from(h.as_str()).style(to_ratatui_style(&header_style(&theme))))
            .collect();
        let header = RatatuiRow::new(header_cells);

        let rows: Vec<RatatuiRow> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let cells: Vec<Cell> = row.iter().map(|c| Cell::from(c.as_str())).collect();
                let style = if i == self.selected {
                    to_ratatui_style(&selected_row_style(&theme))
                } else {
                    Style::default()
                };
                RatatuiRow::new(cells).style(style)
            })
            .collect();

        let table = RatatuiTable::new(rows, widths)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title("Table"));

        frame.render_widget(table, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_event(&mut self, event: &crate::event::Event) -> bool {
        if let crate::event::Event::Mouse(mouse) = event {
            match mouse.kind {
                crate::event::MouseEventKind::ScrollUp => {
                    self.move_selection(-3);
                    return true;
                }
                crate::event::MouseEventKind::ScrollDown => {
                    self.move_selection(3);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;
        match key.key {
            Key::Char('j') | Key::Down => {
                self.move_selection(1);
                Some(super::FocusAction::Consumed)
            }
            Key::Char('k') | Key::Up => {
                self.move_selection(-1);
                Some(super::FocusAction::Consumed)
            }
            Key::Char('g') => {
                self.select_first();
                Some(super::FocusAction::Consumed)
            }
            Key::Char('G') => {
                self.select_last();
                Some(super::FocusAction::Consumed)
            }
            Key::Tab if key.modifiers.shift => Some(super::FocusAction::Previous),
            Key::Tab => Some(super::FocusAction::Next),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Key, KeyEvent, Modifiers, MouseEvent, MouseEventKind};
    use crate::widgets::FocusAction;

    #[test]
    fn table_navigation() {
        let mut table = Table::new().headers(vec!["Name", "Age"]).rows(vec![
            vec!["Alice", "30"],
            vec!["Bob", "25"],
            vec!["Carol", "35"],
        ]);

        assert_eq!(table.selected_index(), 0);
        table.move_selection(1);
        assert_eq!(table.selected_index(), 1);
        table.select_last();
        assert_eq!(table.selected_index(), 2);
        table.select_first();
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_bounds() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);

        table.move_selection(100);
        assert_eq!(table.selected_index(), 1);
        table.move_selection(-100);
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_selected_row() {
        let table = Table::new()
            .headers(vec!["Name"])
            .rows(vec![vec!["Alice"], vec!["Bob"]]);

        assert_eq!(table.selected_row(), Some(&vec!["Alice".to_string()]));
    }

    #[test]
    fn table_empty() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(Vec::<Vec<String>>::new());
        table.move_selection(1); // should not panic
        assert_eq!(table.selected_index(), 0);
        assert_eq!(table.selected_row(), None);
    }

    // --- New tests below ---

    #[test]
    fn table_default_values() {
        let table = Table::default();
        assert_eq!(table.selected_index(), 0);
        assert_eq!(table.selected_row(), None);
    }

    #[test]
    fn table_new_is_default() {
        let table = Table::new();
        assert_eq!(table.selected_index(), 0);
        assert_eq!(table.selected_row(), None);
    }

    fn native_messages(table: &Table) -> Vec<AppMessage> {
        crate::widgets::render_view_messages(table).unwrap()
    }

    fn created_constraints(messages: &[AppMessage], parent: RegionId) -> Vec<Constraints> {
        messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::CreateRegion {
                    parent: p,
                    constraints,
                    ..
                } if *p == parent => Some(constraints.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn native_cells_use_column_widths_and_flex_for_the_rest() {
        let table = Table::new()
            .headers(vec!["A", "B", "C"])
            .column_widths(vec![10.0, 5.0])
            .rows(vec![vec!["1", "2", "3"]]);
        let messages = native_messages(&table);

        // Region 1 is the table, region 2 the header row.
        let header_cells = created_constraints(&messages, RegionId::new(2));
        assert_eq!(header_cells.len(), 3);
        assert_eq!(header_cells[0].width, Dimension::Cells(10.0));
        assert_eq!(header_cells[0].flex_grow, 0.0);
        assert_eq!(header_cells[1].width, Dimension::Cells(5.0));
        assert_eq!(header_cells[2].width, Dimension::Auto);
        assert_eq!(header_cells[2].flex_grow, 1.0);

        let data_row_id = messages
            .iter()
            .filter_map(|m| match m {
                AppMessage::CreateRegion { id, parent, .. } if *parent == RegionId::new(1) => {
                    Some(*id)
                }
                _ => None,
            })
            .nth(1)
            .expect("data row region");
        let data_cells = created_constraints(&messages, data_row_id);
        assert_eq!(data_cells.len(), 3);
        assert_eq!(data_cells[0].width, Dimension::Cells(10.0));
        assert_eq!(data_cells[2].flex_grow, 1.0);
    }

    #[test]
    fn native_header_row_uses_theme_primary() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let table = Table::new().headers(vec!["A"]).rows(vec![vec!["1"]]);
        let messages = native_messages(&table);

        let header_style = messages
            .iter()
            .find_map(|m| match m {
                AppMessage::CreateRegion { id, style, .. } if *id == RegionId::new(2) => {
                    Some(style.clone())
                }
                _ => None,
            })
            .expect("header row region");
        assert_eq!(header_style.fg, Some(theme.primary));
        assert!(header_style.bold);
    }

    #[test]
    fn ansi_columns_use_fixed_widths_and_rows_follow_header_directly() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let table = Table::new()
            .headers(vec!["AA", "BB", "CC"])
            .column_widths(vec![4.0, 3.0])
            .rows(vec![vec!["x", "y", "z"]]);
        let mut terminal = Terminal::new(TestBackend::new(24, 5)).unwrap();
        terminal
            .draw(|frame| table.render_ansi(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();

        // Border takes column 0; columns are 4 and 3 wide with 1 cell spacing.
        assert_eq!(buffer[(1, 1)].symbol(), "A");
        assert_eq!(buffer[(6, 1)].symbol(), "B");
        assert_eq!(buffer[(10, 1)].symbol(), "C");
        assert_eq!(
            buffer[(1, 1)].fg,
            to_ratatui_color(theme.primary),
            "header uses theme.primary"
        );
        // No blank separator row: the data row is directly below the header.
        assert_eq!(buffer[(1, 2)].symbol(), "x");
        assert_eq!(buffer[(6, 2)].symbol(), "y");
        assert_eq!(buffer[(10, 2)].symbol(), "z");
    }

    #[test]
    fn table_selected_row_after_move() {
        let mut table = Table::new().headers(vec!["Name"]).rows(vec![
            vec!["Alice"],
            vec!["Bob"],
            vec!["Carol"],
        ]);

        table.move_selection(2);
        assert_eq!(table.selected_row(), Some(&vec!["Carol".to_string()]));
    }

    #[test]
    fn table_select_first_after_move() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"]]);
        table.move_selection(2);
        assert_eq!(table.selected_index(), 2);
        table.select_first();
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_select_last_on_empty() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(Vec::<Vec<String>>::new());
        table.select_last(); // should not panic
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_move_selection_negative_from_zero() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);
        table.move_selection(-1);
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_move_selection_clamps_to_last() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"]]);
        table.move_selection(10);
        assert_eq!(table.selected_index(), 2);
    }

    #[test]
    fn table_constraints() {
        let table = Table::new();
        let c = <Table as View>::constraints(&table);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Column);
        assert_eq!(c.width, Dimension::Percent(100.0));
    }

    #[test]
    fn table_children_count_header_plus_rows() {
        let table = Table::new()
            .headers(vec!["A"])
            .rows(vec![vec!["1"], vec!["2"], vec!["3"]]);
        // 1 header + 3 data rows = 4
        assert_eq!(<Table as View>::children_count(&table), 4);
    }

    #[test]
    fn table_children_count_no_rows() {
        let table = Table::new()
            .headers(vec!["A"])
            .rows(Vec::<Vec<String>>::new());
        // 1 header + 0 data rows = 1
        assert_eq!(<Table as View>::children_count(&table), 1);
    }

    #[test]
    fn table_is_focusable() {
        let table = Table::new();
        assert!(table.is_focusable());
    }

    #[test]
    fn table_style_has_border() {
        let table = Table::new();
        let style = <Table as View>::style(&table);
        assert_ne!(style.border.style, ultty_protocol::BorderType::None);
    }

    fn key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers::default(),
            focused_id: None,
        }
    }

    fn key_with_shift(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            focused_id: None,
        }
    }

    #[test]
    fn table_on_key_j_moves_down() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);
        let result = table.on_key(&key(Key::Char('j')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 1);
    }

    #[test]
    fn table_on_key_down_moves_down() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);
        let result = table.on_key(&key(Key::Down));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 1);
    }

    #[test]
    fn table_on_key_k_moves_up() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);
        table.move_selection(1);
        let result = table.on_key(&key(Key::Char('k')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_on_key_up_moves_up() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"]]);
        table.move_selection(1);
        let result = table.on_key(&key(Key::Up));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_on_key_g_selects_first() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"]]);
        table.move_selection(2);
        let result = table.on_key(&key(Key::Char('g')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_on_key_shift_g_selects_last() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"]]);
        let result = table.on_key(&key(Key::Char('G')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(table.selected_index(), 2);
    }

    #[test]
    fn table_on_key_tab_focus_next() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![vec!["a"]]);
        let result = table.on_key(&key(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn table_on_key_shift_tab_focus_previous() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![vec!["a"]]);
        let result = table.on_key(&key_with_shift(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn table_on_key_unhandled_returns_none() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![vec!["a"]]);
        let result = table.on_key(&key(Key::Char('z')));
        assert!(result.is_none());
    }

    #[test]
    fn table_on_event_scroll_up() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![
            vec!["a"],
            vec!["b"],
            vec!["c"],
            vec!["d"],
            vec!["e"],
        ]);
        table.move_selection(4);
        assert_eq!(table.selected_index(), 4);

        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = table.on_event(&event);
        assert!(handled);
        assert_eq!(table.selected_index(), 1); // moved up by 3
    }

    #[test]
    fn table_on_event_scroll_down() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![
            vec!["a"],
            vec!["b"],
            vec!["c"],
            vec!["d"],
            vec!["e"],
        ]);

        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = table.on_event(&event);
        assert!(handled);
        assert_eq!(table.selected_index(), 3); // moved down by 3
    }

    #[test]
    fn table_on_event_non_scroll_ignored() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![vec!["a"]]);

        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = table.on_event(&event);
        assert!(!handled);
    }

    #[test]
    fn table_on_event_non_mouse_ignored() {
        let mut table = Table::new().headers(vec!["X"]).rows(vec![vec!["a"]]);

        let handled = table.on_event(&Event::FocusGained);
        assert!(!handled);
    }
}
