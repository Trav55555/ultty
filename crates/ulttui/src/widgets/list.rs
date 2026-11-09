//! List widget with vim navigation.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List as RatatuiList, ListItem};
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use crate::vim::{KeyParser, VimConfig, VimHandler, VimResult};
use crate::vim::{VimAction, VimCommand, VimMode, VimMotion, VimState};
use crate::{impl_vim_ext, impl_vim_handler_accessors};
use ultty_protocol::{
    AppMessage, BorderStyle, Constraints, Dimension, Display, FlexDirection, RegionId,
    Style as ProtoStyle, TextContent,
};

/// A selectable list with vim navigation.
#[derive(Debug)]
pub struct List<T> {
    items: Vec<T>,
    selected: usize,
    vim_state: VimState,
    vim_config: VimConfig,
    key_parser: KeyParser,
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
            key_parser: KeyParser::new(),
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

    /// Select the visible item at `index` (clamped to the last one).
    pub fn selected(mut self, index: usize) -> Self {
        self.select_index(index);
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
    pub fn append(&mut self, item: T) {
        let insert_index = self.items.len();
        let visible = self
            .filter_query
            .as_ref()
            .map(|query| item.as_ref().to_lowercase().contains(query))
            .unwrap_or(true);

        self.items.push(item);
        if visible {
            self.filtered_indices.push(insert_index);
        }

        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.filtered_indices.len() - 1);
        }
    }

    pub fn with_appended(mut self, item: T) -> Self {
        self.append(item);
        self
    }

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
    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            padding: super::BORDER_PADDING,
            ..Default::default()
        }
    }

    fn style(&self) -> ProtoStyle {
        ProtoStyle::new().border(BorderStyle::thin())
    }

    fn content(&self) -> ultty_protocol::Content {
        // Content::None - children provide content
        ultty_protocol::Content::None
    }

    fn children_count(&self) -> usize {
        self.filtered_indices.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let actual_idx = match self.filtered_indices.get(index) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        let item = &self.items[actual_idx];
        let text = format!("{}", item);

        let theme = Theme::current();
        let style = if index == self.selected {
            ProtoStyle::new()
                .fg(theme.selection.fg)
                .bg(theme.selection.bg)
                .bold()
        } else {
            ProtoStyle::default()
        };

        let constraints = Constraints {
            display: Display::Flex,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            ..Default::default()
        };

        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints,
            style,
        })?;

        send_message(&AppMessage::SetContent {
            id: region_id,
            content: ultty_protocol::Content::Text(TextContent::new(text)),
        })?;

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let items: Vec<ListItem> = self
            .filtered_indices
            .iter()
            .enumerate()
            .map(|(display_idx, &actual_idx)| {
                let item = &self.items[actual_idx];
                let content = format!("{}", item);
                let style = if display_idx == self.selected {
                    Style::default()
                        .fg(to_ratatui_color(theme.selection.fg))
                        .bg(to_ratatui_color(theme.selection.bg))
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
                    .fg(to_ratatui_color(theme.selection.fg))
                    .bg(to_ratatui_color(theme.selection.bg))
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_widget(list, area);
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
            Key::Tab if key.modifiers.shift => return Some(super::FocusAction::Previous),
            Key::Tab => return Some(super::FocusAction::Next),
            _ => {}
        }

        // Everything else goes through the persistent vim parser so `gg`,
        // `G`, `3j` and page motions complete across key events.
        let vim_key = crate::vim::to_vim_key(&key.key)?;
        match self.handle_vim_key(&vim_key, crate::vim::to_vim_modifiers(key.modifiers)) {
            VimResult::Ignored => None,
            _ => Some(super::FocusAction::Consumed),
        }
    }
}

impl<T> VimHandler for List<T> {
    impl_vim_handler_accessors!(List<T>);

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

impl_vim_ext!(List<T>, generics: T);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, KeyEvent, Modifiers};
    use crate::widgets::FocusAction;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers::default(),
            focused_id: None,
        }
    }

    fn ten_items() -> List<String> {
        List::new().items((0..10).map(|i| format!("item {i}")).collect())
    }

    #[test]
    fn list_selected_builder_sets_and_clamps_selection() {
        assert_eq!(ten_items().selected(4).selected_index(), 4);
        assert_eq!(ten_items().selected(42).selected_index(), 9);
        assert_eq!(List::<String>::new().selected(3).selected_index(), 0);
    }

    #[test]
    fn list_on_key_j_k_and_arrows_move_selection() {
        let mut list = ten_items();
        assert!(matches!(
            list.on_key(&key(Key::Char('j'))),
            Some(FocusAction::Consumed)
        ));
        list.on_key(&key(Key::Down));
        assert_eq!(list.selected_index(), 2);
        list.on_key(&key(Key::Char('k')));
        list.on_key(&key(Key::Up));
        assert_eq!(list.selected_index(), 0);
    }

    #[test]
    fn list_on_key_gg_and_shift_g_jump_to_ends() {
        let mut list = ten_items();
        list.on_key(&key(Key::Char('G')));
        assert_eq!(list.selected_index(), 9);

        list.on_key(&key(Key::Char('g')));
        assert_eq!(list.selected_index(), 9, "lone g must not jump");
        list.on_key(&key(Key::Char('g')));
        assert_eq!(list.selected_index(), 0);
    }

    #[test]
    fn list_on_key_count_prefix_moves_multiple() {
        let mut list = ten_items();
        list.on_key(&key(Key::Char('3')));
        assert_eq!(list.selected_index(), 0);
        list.on_key(&key(Key::Char('j')));
        assert_eq!(list.selected_index(), 3);
    }

    #[test]
    fn list_on_key_tab_and_unhandled() {
        let mut list = ten_items();
        assert!(matches!(
            list.on_key(&key(Key::Tab)),
            Some(FocusAction::Next)
        ));
        assert!(list.on_key(&key(Key::Char('q'))).is_none());
    }

    fn render_ansi_text(list: &List<&str>, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                list.render_ansi(frame, frame.area());
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

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

    #[test]
    fn test_append_without_filter_updates_visible_count() {
        let mut list = List::new().items(vec!["a", "b"]);
        list.append("c");

        assert_eq!(list.total_count(), 3);
        assert_eq!(list.visible_count(), 3);
        assert_eq!(list.selected_index(), 0);
    }

    #[test]
    fn test_append_with_filter_respects_query() {
        let mut list = List::new().items(vec!["apple", "banana"]);
        list.filter("ap");
        assert_eq!(list.visible_count(), 1);

        list.append("apricot");
        assert_eq!(list.visible_count(), 2);

        list.append("berry");
        assert_eq!(list.visible_count(), 2);
        assert_eq!(list.total_count(), 4);
    }

    #[test]
    fn test_append_updates_ansi_fallback_render() {
        let mut list = List::new().items(vec!["apple", "banana"]);
        list.append("apricot");

        let ansi = render_ansi_text(&list, 40, 8);
        assert!(ansi.contains("apricot"));
        assert!(ansi.contains("List [3/3]"));
    }
}
