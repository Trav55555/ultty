//! Table widget with column headers and row data.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row as RatatuiRow, Table as RatatuiTable};
use ratatui::Frame;

use super::View;
use ultty_protocol::{
    AppMessage, Color as ProtoColor, Constraints, Dimension, Display, FlexDirection, NamedColor,
    RegionId, Style as ProtoStyle, TextContent,
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
        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        if index == 0 {
            // Header row
            let style = ProtoStyle::new()
                .fg(ProtoColor::Named(NamedColor::Cyan))
                .bold();
            let constraints = Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                ..Default::default()
            };

            send_message(&AppMessage::CreateRegion {
                id: region_id,
                parent,
                insert_index: -1,
                constraints,
                style,
            })?;

            // Create child cells for each header
            for header in &self.headers {
                let cell_id = RegionId::new(*next_id);
                *next_id += 1;
                send_message(&AppMessage::CreateRegion {
                    id: cell_id,
                    parent: region_id,
                    insert_index: -1,
                    constraints: Constraints {
                        flex_grow: 1.0,
                        ..Default::default()
                    },
                    style: ProtoStyle::default(),
                })?;
                send_message(&AppMessage::SetContent {
                    id: cell_id,
                    content: ultty_protocol::Content::Text(TextContent::new(header)),
                })?;
            }
        } else {
            // Data row
            let row_idx = index - 1;
            let is_selected = row_idx == self.selected;
            let style = if is_selected {
                ProtoStyle::new()
                    .fg(ProtoColor::Named(NamedColor::Black))
                    .bg(ProtoColor::Named(NamedColor::White))
                    .bold()
            } else {
                ProtoStyle::default()
            };

            let constraints = Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                ..Default::default()
            };

            send_message(&AppMessage::CreateRegion {
                id: region_id,
                parent,
                insert_index: -1,
                constraints,
                style,
            })?;

            if let Some(row) = self.rows.get(row_idx) {
                for cell_text in row {
                    let cell_id = RegionId::new(*next_id);
                    *next_id += 1;
                    send_message(&AppMessage::CreateRegion {
                        id: cell_id,
                        parent: region_id,
                        insert_index: -1,
                        constraints: Constraints {
                            flex_grow: 1.0,
                            ..Default::default()
                        },
                        style: ProtoStyle::default(),
                    })?;
                    send_message(&AppMessage::SetContent {
                        id: cell_id,
                        content: ultty_protocol::Content::Text(TextContent::new(cell_text)),
                    })?;
                }
            }
        }

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::layout::Constraint;

        let col_count = self.headers.len().max(
            self.rows
                .first()
                .map(|r| r.len())
                .unwrap_or(0),
        );

        let widths: Vec<Constraint> = if self.column_widths.is_empty() {
            vec![Constraint::Ratio(1, col_count.max(1) as u32); col_count]
        } else {
            self.column_widths
                .iter()
                .map(|&w| Constraint::Min(w as u16))
                .collect()
        };

        let header_cells: Vec<Cell> = self
            .headers
            .iter()
            .map(|h| {
                Cell::from(h.as_str()).style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect();
        let header = RatatuiRow::new(header_cells).bottom_margin(1);

        let rows: Vec<RatatuiRow> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let cells: Vec<Cell> = row.iter().map(|c| Cell::from(c.as_str())).collect();
                let style = if i == self.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
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
    use crate::event::{
        Event, Key, KeyEvent, Modifiers, MouseEvent, MouseEventKind,
    };
    use crate::widgets::FocusAction;

    #[test]
    fn table_navigation() {
        let mut table = Table::new()
            .headers(vec!["Name", "Age"])
            .rows(vec![
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

        assert_eq!(
            table.selected_row(),
            Some(&vec!["Alice".to_string()])
        );
    }

    #[test]
    fn table_empty() {
        let mut table = Table::new().headers(vec!["X"]).rows(Vec::<Vec<String>>::new());
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

    #[test]
    fn table_column_widths_builder() {
        let table = Table::new()
            .headers(vec!["A", "B"])
            .column_widths(vec![10.0, 20.0]);
        // Verify it doesn't panic and the table is constructed
        assert_eq!(table.selected_index(), 0);
    }

    #[test]
    fn table_selected_row_after_move() {
        let mut table = Table::new()
            .headers(vec!["Name"])
            .rows(vec![vec!["Alice"], vec!["Bob"], vec!["Carol"]]);

        table.move_selection(2);
        assert_eq!(
            table.selected_row(),
            Some(&vec!["Carol".to_string()])
        );
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
    fn table_debug_format() {
        let table = Table::new()
            .headers(vec!["Name", "Age"])
            .rows(vec![vec!["Alice", "30"]]);
        let debug = format!("{:?}", table);
        assert!(debug.contains("Table"));
        assert!(debug.contains("rows_count"));
        assert!(debug.contains("selected"));
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
        }
    }

    fn key_with_shift(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
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
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"]]);
        let result = table.on_key(&key(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn table_on_key_shift_tab_focus_previous() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"]]);
        let result = table.on_key(&key_with_shift(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn table_on_key_unhandled_returns_none() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"]]);
        let result = table.on_key(&key(Key::Char('z')));
        assert!(result.is_none());
    }

    #[test]
    fn table_on_event_scroll_up() {
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"], vec!["d"], vec!["e"]]);
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
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"], vec!["b"], vec!["c"], vec!["d"], vec!["e"]]);

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
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"]]);

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
        let mut table = Table::new()
            .headers(vec!["X"])
            .rows(vec![vec!["a"]]);

        let handled = table.on_event(&Event::FocusGained);
        assert!(!handled);
    }
}
