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
}
