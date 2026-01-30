//! List widget with vim navigation.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List as RatatuiList, ListItem};
use ratatui::Frame;

use super::View;
use crate::vim::{VimConfig, VimExt, VimHandler, VimResult};
use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimState};

/// A selectable list with vim navigation.
#[derive(Debug)]
pub struct List<T> {
    items: Vec<T>,
    selected: usize,
    vim_state: VimState,
    vim_config: VimConfig,
    filter_query: Option<String>,
    filtered_indices: Vec<usize>,
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            vim_state: VimState::new(),
            vim_config: VimConfig::for_list(),
            filter_query: None,
            filtered_indices: Vec::new(),
        }
    }
}

impl<T> List<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(mut self, items: Vec<T>) -> Self {
        let len = items.len();
        self.items = items;
        self.filtered_indices = (0..len).collect();
        self
    }

    pub fn selected_index(&self) -> usize {
        if self.filtered_indices.is_empty() {
            0
        } else {
            self.filtered_indices[self.selected.min(self.filtered_indices.len() - 1)]
        }
    }

    pub fn selected_item(&self) -> Option<&T> {
        if self.filtered_indices.is_empty() {
            None
        } else {
            let idx = self.selected.min(self.filtered_indices.len() - 1);
            self.items.get(self.filtered_indices[idx])
        }
    }

    pub fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }

    pub fn total_count(&self) -> usize {
        self.items.len()
    }

    fn move_selection(&mut self, delta: i32) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let len = self.filtered_indices.len();
        if delta > 0 {
            self.selected = (self.selected + delta as usize).min(len - 1);
        } else {
            let abs_delta = (-delta) as usize;
            self.selected = self.selected.saturating_sub(abs_delta);
        }
    }

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    fn select_index(&mut self, index: usize) {
        if !self.filtered_indices.is_empty() {
            self.selected = index.min(self.filtered_indices.len() - 1);
        }
    }
}

impl<T: AsRef<str>> List<T> {
    pub fn filter(&mut self, query: &str) {
        if query.is_empty() {
            self.filter_query = None;
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filter_query = Some(query.to_lowercase());
            let query_lower = query.to_lowercase();
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.as_ref().to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter_query = None;
        self.filtered_indices = (0..self.items.len()).collect();
    }
}

impl<T: std::fmt::Display> View for List<T> {
    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .filtered_indices
            .iter()
            .enumerate()
            .map(|(display_idx, &actual_idx)| {
                let item = &self.items[actual_idx];
                let content = format!("{}", item);
                let style = if display_idx == self.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::styled(content, style))
            })
            .collect();

        let title = if let Some(query) = &self.filter_query {
            format!(
                "List (filter: '{}') [{}/{}]",
                query,
                self.visible_count(),
                self.total_count()
            )
        } else {
            format!("List [{}/{}]", self.visible_count(), self.total_count())
        };

        let list = RatatuiList::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_widget(list, area);
    }
}

impl<T> VimHandler for List<T> {
    fn vim_state(&self) -> &VimState {
        &self.vim_state
    }

    fn vim_state_mut(&mut self) -> &mut VimState {
        &mut self.vim_state
    }

    fn handle_vim_action(&mut self, action: &VimAction) -> VimResult {
        match action {
            VimAction::SearchForward => {
                self.vim_state.mode = VimMode::Search;
                self.vim_state.command_buffer.clear();
                VimResult::ModeChanged(VimMode::Search)
            }
            VimAction::ExitToNormal => {
                if self.vim_state.mode == VimMode::Search {
                    if let Some(query) = self.filter_query.clone() {
                        self.vim_state.last_search = Some(query);
                    }
                }
                self.vim_state.reset_to_normal();
                VimResult::ModeChanged(VimMode::Normal)
            }
            _ => VimResult::Ignored,
        }
    }

    fn handle_vim_command(&mut self, command: &VimCommand) -> VimResult {
        let count = command.effective_count() as i32;

        match &command.motion {
            VimMotion::Down => {
                self.move_selection(count);
                VimResult::Handled
            }
            VimMotion::Up => {
                self.move_selection(-count);
                VimResult::Handled
            }
            VimMotion::DocumentStart => {
                self.select_first();
                VimResult::Handled
            }
            VimMotion::DocumentEnd => {
                self.select_last();
                VimResult::Handled
            }
            VimMotion::Line(n) => {
                self.select_index(*n);
                VimResult::Handled
            }
            VimMotion::HalfPageDown => {
                let page = (self.visible_count() / 2).max(1) as i32;
                self.move_selection(page * count);
                VimResult::Handled
            }
            VimMotion::HalfPageUp => {
                let page = (self.visible_count() / 2).max(1) as i32;
                self.move_selection(-page * count);
                VimResult::Handled
            }
            VimMotion::PageDown => {
                let page = self.visible_count().max(1) as i32;
                self.move_selection(page * count);
                VimResult::Handled
            }
            VimMotion::PageUp => {
                let page = self.visible_count().max(1) as i32;
                self.move_selection(-page * count);
                VimResult::Handled
            }
            _ => VimResult::Ignored,
        }
    }
}

impl<T> VimExt for List<T> {
    fn vim(self) -> Self {
        self
    }

    fn vim_with_config(mut self, config: VimConfig) -> Self {
        self.vim_config = config;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_navigation() {
        let mut list = List::new().items(vec!["a", "b", "c", "d", "e"]);

        assert_eq!(list.selected_index(), 0);

        list.handle_vim_command(&VimCommand::motion(VimMotion::Down));
        assert_eq!(list.selected_index(), 1);

        list.handle_vim_command(&VimCommand::motion_with_count(2, VimMotion::Down));
        assert_eq!(list.selected_index(), 3);

        list.handle_vim_command(&VimCommand::motion(VimMotion::DocumentEnd));
        assert_eq!(list.selected_index(), 4);

        list.handle_vim_command(&VimCommand::motion(VimMotion::DocumentStart));
        assert_eq!(list.selected_index(), 0);
    }

    #[test]
    fn test_list_filter() {
        let mut list = List::new().items(vec!["apple", "banana", "apricot", "cherry"]);

        list.filter("ap");
        assert_eq!(list.visible_count(), 2);
        assert_eq!(list.selected_item(), Some(&"apple"));

        list.handle_vim_command(&VimCommand::motion(VimMotion::Down));
        assert_eq!(list.selected_item(), Some(&"apricot"));

        list.clear_filter();
        assert_eq!(list.visible_count(), 4);
    }

    #[test]
    fn test_list_bounds() {
        let mut list = List::new().items(vec!["a", "b", "c"]);

        list.handle_vim_command(&VimCommand::motion_with_count(100, VimMotion::Down));
        assert_eq!(list.selected_index(), 2);

        list.handle_vim_command(&VimCommand::motion_with_count(100, VimMotion::Up));
        assert_eq!(list.selected_index(), 0);
    }
}
