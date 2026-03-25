//! Scroll view widget with vim navigation.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use crate::vim::{VimConfig, VimHandler, VimResult};
use crate::{impl_vim_ext, impl_vim_handler_accessors};
use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimState};
use ultty_protocol::{AppMessage, Constraints, Display, FlexDirection, OverflowAxis, RegionId};

pub struct ScrollView {
    constraints: Constraints,
    child: Option<Box<dyn View>>,
    scroll_x: f32,
    scroll_y: f32,
    content_height: f32,
    viewport_height: f32,
    virtual_scroll: bool,
    item_height: f32,
    vim_state: VimState,
    vim_config: VimConfig,
    search_query: Option<String>,
    search_matches: Vec<usize>,
    current_match: usize,
}

impl std::fmt::Debug for ScrollView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollView")
            .field("scroll_y", &self.scroll_y)
            .field("has_child", &self.child.is_some())
            .field("virtual_scroll", &self.virtual_scroll)
            .finish()
    }
}

impl Default for ScrollView {
    fn default() -> Self {
        let constraints = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: ultty_protocol::Dimension::Cells(0.0),
            width: ultty_protocol::Dimension::Percent(100.0),
            overflow: ultty_protocol::Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Scroll,
            },
            ..Default::default()
        };
        Self {
            constraints,
            child: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            content_height: 0.0,
            viewport_height: 0.0,
            virtual_scroll: false,
            item_height: 1.0,
            vim_state: VimState::new(),
            vim_config: VimConfig::for_scroll_view(),
            search_query: None,
            search_matches: Vec::new(),
            current_match: 0,
        }
    }
}

impl ScrollView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child<V: View + 'static>(mut self, child: V) -> Self {
        self.child = Some(Box::new(child));
        self
    }

    pub fn horizontal(mut self) -> Self {
        self.constraints.overflow.x = OverflowAxis::Scroll;
        self
    }

    pub fn virtual_scroll(mut self, enabled: bool) -> Self {
        self.virtual_scroll = enabled;
        self
    }

    pub fn item_height(mut self, height: f32) -> Self {
        self.item_height = if height > 0.0 { height } else { 1.0 };
        self
    }

    pub fn scroll_position(&self) -> (f32, f32) {
        (self.scroll_x, self.scroll_y)
    }

    pub fn set_content_height(&mut self, height: f32) {
        self.content_height = height;
    }

    pub fn set_viewport_height(&mut self, height: f32) {
        self.viewport_height = height;
    }

    pub fn viewport_height(&self) -> f32 {
        self.viewport_height
    }

    pub fn is_near_bottom(&self, threshold: f32) -> bool {
        let max = (self.content_height - self.viewport_height).max(0.0);
        max - self.scroll_y <= threshold
    }

    pub fn scroll_to(&mut self, y: f32) {
        let max_scroll = (self.content_height - self.viewport_height).max(0.0);
        self.scroll_y = y.clamp(0.0, max_scroll);
    }

    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll_to(self.scroll_y + delta);
    }

    fn visible_range(&self, total_children: usize) -> (usize, usize) {
        if !self.virtual_scroll || total_children == 0 {
            return (0, total_children);
        }

        let item_height = if self.item_height > 0.0 {
            self.item_height
        } else {
            1.0
        };
        let buffer_pages = 1.0;
        let buffer = self.viewport_height * buffer_pages;

        let visible_start = (self.scroll_y - buffer).max(0.0);
        let visible_end = self.scroll_y + self.viewport_height + buffer;

        let start_idx = (visible_start / item_height) as usize;
        let end_idx = ((visible_end / item_height).ceil() as usize).min(total_children);

        (start_idx, end_idx)
    }

    pub fn visible_child_range(&self, total_items: usize) -> Option<(usize, usize)> {
        if !self.virtual_scroll || total_items == 0 {
            return None;
        }

        Some(self.visible_range(total_items))
    }

    fn line_height(&self) -> f32 {
        1.0
    }

    fn page_size(&self) -> f32 {
        self.viewport_height
    }

    fn half_page_size(&self) -> f32 {
        self.viewport_height / 2.0
    }

    fn scroll_lines(&mut self, lines: i32) {
        let delta = lines as f32 * self.line_height();
        self.scroll_by(delta);
    }

    fn scroll_to_top(&mut self) {
        self.scroll_y = 0.0;
    }

    pub fn scroll_to_bottom(&mut self) {
        let max_scroll = (self.content_height - self.viewport_height).max(0.0);
        self.scroll_y = max_scroll;
    }

    fn scroll_to_line(&mut self, line: usize) {
        let y = line as f32 * self.line_height();
        self.scroll_to(y);
    }

    fn center_on_line(&mut self, line: usize) {
        let y = line as f32 * self.line_height();
        let centered = y - self.viewport_height / 2.0;
        self.scroll_to(centered);
    }

    pub fn search(&mut self, query: &str) {
        self.search_query = Some(query.to_string());
        self.search_matches.clear();
        self.current_match = 0;
    }

    pub fn add_search_match(&mut self, line: usize) {
        self.search_matches.push(line);
    }

    fn goto_next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_match = (self.current_match + 1) % self.search_matches.len();
        let line = self.search_matches[self.current_match];
        self.center_on_line(line);
    }

    fn goto_prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.current_match == 0 {
            self.current_match = self.search_matches.len() - 1;
        } else {
            self.current_match -= 1;
        }
        let line = self.search_matches[self.current_match];
        self.center_on_line(line);
    }
}

impl View for ScrollView {
    fn constraints(&self) -> Constraints {
        self.constraints.clone()
    }

    fn style(&self) -> ultty_protocol::Style {
        ultty_protocol::Style::new().border(ultty_protocol::BorderStyle::thin())
    }

    fn content(&self) -> ultty_protocol::Content {
        if self.child.is_some() {
            ultty_protocol::Content::None
        } else {
            let scroll_info = format!(
                "Scroll: {:.0}/{:.0}",
                self.scroll_y,
                self.content_height.max(1.0) - self.viewport_height
            );
            ultty_protocol::Content::Text(ultty_protocol::TextContent::new(scroll_info))
        }
    }

    fn children_count(&self) -> usize {
        if self.child.is_some() {
            1
        } else {
            0
        }
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        if index == 0 {
            if let Some(child) = &self.child {
                let region_id = RegionId::new(*next_id);
                *next_id += 1;

                send_message(&AppMessage::CreateRegion {
                    id: region_id,
                    parent,
                    insert_index: -1,
                    constraints: child.constraints(),
                    style: child.style(),
                })?;

                let content = child.content();
                if content != ultty_protocol::Content::None {
                    send_message(&AppMessage::SetContent {
                        id: region_id,
                        content,
                    })?;
                }

                for i in 0..child.children_count() {
                    child.render_child_native(i, send_message, region_id, next_id)?;
                }

                return Ok(Some(region_id));
            }
        }
        Ok(None)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let max_scroll = (self.content_height - self.viewport_height).max(0.0);
        let pct = if max_scroll > 0.0 {
            (self.scroll_y / max_scroll * 100.0) as u32
        } else {
            0
        };
        let title = if self.content_height > 0.0 {
            format!("ScrollView [{}%]", pct)
        } else {
            "ScrollView".to_string()
        };

        let theme = Theme::current();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(to_ratatui_color(theme.muted)));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(child) = &self.child {
            child.render_ansi(frame, inner);
        } else {
            let scroll_info = format!("Scroll: {:.0}/{:.0}", self.scroll_y, max_scroll);
            let paragraph = Paragraph::new(Line::raw(scroll_info));
            frame.render_widget(paragraph, inner);
        }
    }

    fn scroll_offset(&self) -> Option<(f32, f32)> {
        if self.scroll_x != 0.0 || self.scroll_y != 0.0 {
            Some((self.scroll_x, self.scroll_y))
        } else {
            None
        }
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_event(&mut self, event: &crate::event::Event) -> bool {
        if let crate::event::Event::Mouse(mouse) = event {
            match mouse.kind {
                crate::event::MouseEventKind::ScrollUp => {
                    self.scroll_lines(-3);
                    return true;
                }
                crate::event::MouseEventKind::ScrollDown => {
                    self.scroll_lines(3);
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
                self.scroll_lines(1);
                Some(super::FocusAction::Consumed)
            }
            Key::Char('k') | Key::Up => {
                self.scroll_lines(-1);
                Some(super::FocusAction::Consumed)
            }
            Key::Char('d') if key.modifiers.ctrl => {
                self.scroll_by(self.half_page_size());
                Some(super::FocusAction::Consumed)
            }
            Key::Char('u') if key.modifiers.ctrl => {
                self.scroll_by(-self.half_page_size());
                Some(super::FocusAction::Consumed)
            }
            Key::Char('g') => {
                self.scroll_to_top();
                Some(super::FocusAction::Consumed)
            }
            Key::Char('G') => {
                self.scroll_to_bottom();
                Some(super::FocusAction::Consumed)
            }
            Key::Tab if key.modifiers.shift => Some(super::FocusAction::Previous),
            Key::Tab => Some(super::FocusAction::Next),
            _ => None,
        }
    }
}

impl VimHandler for ScrollView {
    impl_vim_handler_accessors!(ScrollView);

    fn handle_vim_action(&mut self, action: &VimAction) -> VimResult {
        match action {
            VimAction::SearchForward => {
                self.vim_state.mode = VimMode::Search;
                self.vim_state.search_forward = true;
                self.vim_state.command_buffer.clear();
                VimResult::ModeChanged(VimMode::Search)
            }
            VimAction::SearchBackward => {
                self.vim_state.mode = VimMode::Search;
                self.vim_state.search_forward = false;
                self.vim_state.command_buffer.clear();
                VimResult::ModeChanged(VimMode::Search)
            }
            VimAction::CenterScreen => {
                let current_line = (self.scroll_y / self.line_height()) as usize;
                let visible_lines = (self.viewport_height / self.line_height()) as usize;
                self.center_on_line(current_line + visible_lines / 2);
                VimResult::Handled
            }
            VimAction::ScrollCursorTop => VimResult::Handled,
            VimAction::ScrollCursorBottom => VimResult::Handled,
            VimAction::ExitToNormal => {
                if self.vim_state.mode == VimMode::Search
                    && !self.vim_state.command_buffer.is_empty()
                {
                    let query = self.vim_state.command_buffer.clone();
                    self.vim_state.last_search = Some(query.clone());
                    self.search(&query);
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
                self.scroll_lines(count);
                VimResult::Handled
            }
            VimMotion::Up => {
                self.scroll_lines(-count);
                VimResult::Handled
            }
            VimMotion::HalfPageDown => {
                for _ in 0..count {
                    self.scroll_by(self.half_page_size());
                }
                VimResult::Handled
            }
            VimMotion::HalfPageUp => {
                for _ in 0..count {
                    self.scroll_by(-self.half_page_size());
                }
                VimResult::Handled
            }
            VimMotion::PageDown => {
                for _ in 0..count {
                    self.scroll_by(self.page_size());
                }
                VimResult::Handled
            }
            VimMotion::PageUp => {
                for _ in 0..count {
                    self.scroll_by(-self.page_size());
                }
                VimResult::Handled
            }
            VimMotion::DocumentStart => {
                self.scroll_to_top();
                VimResult::Handled
            }
            VimMotion::DocumentEnd => {
                self.scroll_to_bottom();
                VimResult::Handled
            }
            VimMotion::Line(n) => {
                self.scroll_to_line(*n);
                VimResult::Handled
            }
            VimMotion::ScreenTop => VimResult::Handled,
            VimMotion::ScreenMiddle => VimResult::Handled,
            VimMotion::ScreenBottom => VimResult::Handled,
            VimMotion::SearchNext => {
                for _ in 0..count {
                    if self.vim_state.search_forward {
                        self.goto_next_match();
                    } else {
                        self.goto_prev_match();
                    }
                }
                VimResult::Handled
            }
            VimMotion::SearchPrev => {
                for _ in 0..count {
                    if self.vim_state.search_forward {
                        self.goto_prev_match();
                    } else {
                        self.goto_next_match();
                    }
                }
                VimResult::Handled
            }
            _ => VimResult::Ignored,
        }
    }
}

impl_vim_ext!(ScrollView);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Key, KeyEvent, Modifiers, MouseEvent, MouseEventKind};
    use crate::widgets::FocusAction;

    #[test]
    fn test_scroll_navigation() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let cmd = VimCommand::motion(VimMotion::Down);
        scroll.handle_vim_command(&cmd);
        assert!(scroll.scroll_y > 0.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::DocumentEnd));
        assert_eq!(scroll.scroll_y, 900.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::DocumentStart));
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn test_page_navigation() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::HalfPageDown));
        assert_eq!(scroll.scroll_y, 50.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::PageDown));
        assert_eq!(scroll.scroll_y, 150.0);
    }

    #[test]
    fn test_search() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.search("test");
        scroll.add_search_match(10);
        scroll.add_search_match(50);
        scroll.add_search_match(90);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::SearchNext));
        assert!(scroll.scroll_y > 0.0);
    }

    // --- New tests below ---

    #[test]
    fn scroll_default_values() {
        let scroll = ScrollView::default();
        assert_eq!(scroll.scroll_position(), (0.0, 0.0));
        assert_eq!(scroll.content_height, 0.0);
        assert_eq!(scroll.viewport_height, 0.0);
    }

    #[test]
    fn scroll_virtual_scroll_default_off() {
        let scroll = ScrollView::new();
        assert!(!scroll.virtual_scroll);
    }

    #[test]
    fn scroll_virtual_scroll_builder() {
        let scroll = ScrollView::new().virtual_scroll(true);
        assert!(scroll.virtual_scroll);
    }

    #[test]
    fn scroll_item_height_default() {
        let scroll = ScrollView::new();
        assert_eq!(scroll.item_height, 1.0);
    }

    #[test]
    fn scroll_item_height_builder() {
        let scroll = ScrollView::new().item_height(3.0);
        assert_eq!(scroll.item_height, 3.0);
    }

    #[test]
    fn scroll_visible_range_disabled() {
        let scroll = ScrollView::new();
        assert_eq!(scroll.visible_range(100), (0, 100));
    }

    #[test]
    fn scroll_visible_range_enabled_at_top() {
        let mut scroll = ScrollView::new().virtual_scroll(true).item_height(1.0);
        scroll.set_viewport_height(50.0);
        scroll.set_content_height(1000.0);

        let (start, end) = scroll.visible_range(1000);
        assert_eq!(start, 0);
        assert!(end <= 101);
        assert!(end >= 50);
    }

    #[test]
    fn scroll_visible_range_enabled_scrolled() {
        let mut scroll = ScrollView::new().virtual_scroll(true).item_height(1.0);
        scroll.set_viewport_height(50.0);
        scroll.set_content_height(1000.0);
        scroll.scroll_to(500.0);

        let (start, end) = scroll.visible_range(1000);
        assert!(start > 0);
        assert!(start <= 450);
        assert!(end >= 550);
        assert!(end <= 601);
    }

    #[test]
    fn scroll_visible_range_clamps_to_total() {
        let mut scroll = ScrollView::new().virtual_scroll(true).item_height(1.0);
        scroll.set_viewport_height(50.0);
        scroll.set_content_height(30.0);

        let (start, end) = scroll.visible_range(30);
        assert_eq!(start, 0);
        assert_eq!(end, 30);
    }

    #[test]
    fn scroll_visible_child_range_none_when_disabled() {
        let scroll = ScrollView::new();
        assert!(scroll.visible_child_range(100).is_none());
    }

    #[test]
    fn scroll_visible_child_range_some_when_enabled() {
        let mut scroll = ScrollView::new().virtual_scroll(true);
        scroll.set_viewport_height(50.0);
        scroll.set_content_height(1000.0);

        assert!(scroll.visible_child_range(1000).is_some());
    }

    #[test]
    fn scroll_visible_child_range_empty_total() {
        let scroll = ScrollView::new().virtual_scroll(true);
        assert!(scroll.visible_child_range(0).is_none());
    }

    #[test]
    fn scroll_new_is_default() {
        let scroll = ScrollView::new();
        assert_eq!(scroll.scroll_position(), (0.0, 0.0));
    }

    #[test]
    fn scroll_set_content_height() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        assert_eq!(scroll.content_height, 500.0);
    }

    #[test]
    fn scroll_set_viewport_height() {
        let mut scroll = ScrollView::new();
        scroll.set_viewport_height(200.0);
        assert_eq!(scroll.viewport_height, 200.0);
    }

    #[test]
    fn scroll_to_clamps_to_max() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(1000.0);
        assert_eq!(scroll.scroll_y, 400.0); // max is content - viewport = 400
    }

    #[test]
    fn scroll_to_clamps_to_zero() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(-50.0);
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_to_when_content_smaller_than_viewport() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(50.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(100.0);
        assert_eq!(scroll.scroll_y, 0.0); // max is 0 since content < viewport
    }

    #[test]
    fn scroll_by_positive() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_by(50.0);
        assert_eq!(scroll.scroll_y, 50.0);
        scroll.scroll_by(25.0);
        assert_eq!(scroll.scroll_y, 75.0);
    }

    #[test]
    fn scroll_by_negative() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(200.0);
        scroll.scroll_by(-50.0);
        assert_eq!(scroll.scroll_y, 150.0);
    }

    #[test]
    fn scroll_by_clamps() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_by(-100.0);
        assert_eq!(scroll.scroll_y, 0.0);

        scroll.scroll_by(10000.0);
        assert_eq!(scroll.scroll_y, 400.0);
    }

    #[test]
    fn scroll_position_returns_xy() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(123.0);
        let (x, y) = scroll.scroll_position();
        assert_eq!(x, 0.0);
        assert_eq!(y, 123.0);
    }

    #[test]
    fn scroll_to_top() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        scroll.scroll_to_top();
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_to_bottom() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(500.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to_bottom();
        assert_eq!(scroll.scroll_y, 400.0);
    }

    #[test]
    fn scroll_to_bottom_when_content_fits() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(50.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to_bottom();
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_lines_down() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_lines(2);
        assert_eq!(scroll.scroll_y, 2.0); // 2 * 1.0
    }

    #[test]
    fn scroll_lines_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to(100.0);
        scroll.scroll_lines(-3);
        assert_eq!(scroll.scroll_y, 97.0); // 100 - 3*1.0 = 97
    }

    #[test]
    fn scroll_to_line() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.scroll_to_line(5);
        assert_eq!(scroll.scroll_y, 5.0); // 5 * 1.0
    }

    #[test]
    fn scroll_center_on_line() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.center_on_line(10);
        // 10 * 1.0 - 100.0/2 = 10 - 50 = -40, clamped to 0
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_center_on_line_clamps_to_zero() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.center_on_line(0);
        // 0 - 50 = -50, clamped to 0
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_search_clears_previous() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.search("first");
        scroll.add_search_match(10);
        assert_eq!(scroll.search_matches.len(), 1);

        scroll.search("second");
        assert_eq!(scroll.search_matches.len(), 0);
        assert_eq!(scroll.current_match, 0);
    }

    #[test]
    fn scroll_search_next_wraps() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.search("test");
        scroll.add_search_match(5);
        scroll.add_search_match(20);

        scroll.goto_next_match();
        assert_eq!(scroll.current_match, 1);

        scroll.goto_next_match();
        assert_eq!(scroll.current_match, 0); // wraps
    }

    #[test]
    fn scroll_search_prev_wraps() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.search("test");
        scroll.add_search_match(5);
        scroll.add_search_match(20);

        assert_eq!(scroll.current_match, 0);
        scroll.goto_prev_match();
        assert_eq!(scroll.current_match, 1); // wraps back

        scroll.goto_prev_match();
        assert_eq!(scroll.current_match, 0);
    }

    #[test]
    fn scroll_search_next_empty_noop() {
        let mut scroll = ScrollView::new();
        scroll.search("nothing");
        scroll.goto_next_match(); // should not panic
        assert_eq!(scroll.current_match, 0);
    }

    #[test]
    fn scroll_search_prev_empty_noop() {
        let mut scroll = ScrollView::new();
        scroll.search("nothing");
        scroll.goto_prev_match(); // should not panic
        assert_eq!(scroll.current_match, 0);
    }

    #[test]
    fn scroll_search_next_centers_on_match() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(2000.0);
        scroll.set_viewport_height(100.0);

        scroll.search("test");
        scroll.add_search_match(50);

        scroll.goto_next_match();
        // centers on line 50: 50*1.0 - 50 = 0, clamped to 0
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_horizontal_sets_overflow() {
        let scroll = ScrollView::new().horizontal();
        let c = scroll.constraints();
        assert_eq!(c.overflow.x, OverflowAxis::Scroll);
    }

    #[test]
    fn scroll_is_focusable() {
        let scroll = ScrollView::new();
        assert!(scroll.is_focusable());
    }

    #[test]
    fn scroll_children_count_no_child() {
        let scroll = ScrollView::new();
        assert_eq!(<ScrollView as View>::children_count(&scroll), 0);
    }

    #[test]
    fn scroll_children_count_with_child() {
        let scroll = ScrollView::new().child(crate::widgets::Text::new("hello"));
        assert_eq!(<ScrollView as View>::children_count(&scroll), 1);
    }

    #[test]
    fn scroll_content_without_child() {
        let scroll = ScrollView::new();
        let content = <ScrollView as View>::content(&scroll);
        match content {
            ultty_protocol::Content::Text(_) => {} // expected
            _ => panic!("Expected Text content when no child"),
        }
    }

    #[test]
    fn scroll_content_with_child() {
        let scroll = ScrollView::new().child(crate::widgets::Text::new("hello"));
        let content = <ScrollView as View>::content(&scroll);
        assert_eq!(content, ultty_protocol::Content::None);
    }

    #[test]
    fn scroll_debug_format() {
        let scroll = ScrollView::new();
        let debug = format!("{:?}", scroll);
        assert!(debug.contains("ScrollView"));
        assert!(debug.contains("scroll_y"));
    }

    #[test]
    fn scroll_style_has_border() {
        let scroll = ScrollView::new();
        let style = <ScrollView as View>::style(&scroll);
        assert_ne!(style.border.style, ultty_protocol::BorderType::None);
    }

    fn key(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers::default(),
            focused_id: None,
        }
    }

    fn key_with_ctrl(k: Key) -> KeyEvent {
        KeyEvent {
            key: k,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
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
    fn scroll_on_key_j_scrolls_down() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let result = scroll.on_key(&key(Key::Char('j')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(scroll.scroll_y > 0.0);
    }

    #[test]
    fn scroll_on_key_down_scrolls_down() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let result = scroll.on_key(&key(Key::Down));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(scroll.scroll_y > 0.0);
    }

    #[test]
    fn scroll_on_key_k_scrolls_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        let result = scroll.on_key(&key(Key::Char('k')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(scroll.scroll_y < 200.0);
    }

    #[test]
    fn scroll_on_key_up_scrolls_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        let result = scroll.on_key(&key(Key::Up));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(scroll.scroll_y < 200.0);
    }

    #[test]
    fn scroll_on_key_ctrl_d_half_page_down() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let result = scroll.on_key(&key_with_ctrl(Key::Char('d')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(scroll.scroll_y, 50.0); // half of viewport
    }

    #[test]
    fn scroll_on_key_ctrl_u_half_page_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        let result = scroll.on_key(&key_with_ctrl(Key::Char('u')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(scroll.scroll_y, 150.0);
    }

    #[test]
    fn scroll_on_key_g_goes_to_top() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(500.0);

        let result = scroll.on_key(&key(Key::Char('g')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(scroll.scroll_y, 0.0);
    }

    #[test]
    fn scroll_on_key_shift_g_goes_to_bottom() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let result = scroll.on_key(&key(Key::Char('G')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(scroll.scroll_y, 900.0);
    }

    #[test]
    fn scroll_on_key_tab_focus_next() {
        let mut scroll = ScrollView::new();
        let result = scroll.on_key(&key(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn scroll_on_key_shift_tab_focus_previous() {
        let mut scroll = ScrollView::new();
        let result = scroll.on_key(&key_with_shift(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn scroll_on_key_unhandled_returns_none() {
        let mut scroll = ScrollView::new();
        let result = scroll.on_key(&key(Key::Char('z')));
        assert!(result.is_none());
    }

    #[test]
    fn scroll_on_event_scroll_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = scroll.on_event(&event);
        assert!(handled);
        assert!(scroll.scroll_y < 200.0); // scrolled up by 3 lines
    }

    #[test]
    fn scroll_on_event_scroll_down() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = scroll.on_event(&event);
        assert!(handled);
        assert!(scroll.scroll_y > 0.0);
    }

    #[test]
    fn scroll_on_event_non_scroll_ignored() {
        let mut scroll = ScrollView::new();
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            x: 0,
            y: 0,
            modifiers: Modifiers::default(),
        });
        let handled = scroll.on_event(&event);
        assert!(!handled);
    }

    #[test]
    fn scroll_on_event_non_mouse_ignored() {
        let mut scroll = ScrollView::new();
        let handled = scroll.on_event(&Event::FocusGained);
        assert!(!handled);
    }

    #[test]
    fn scroll_vim_page_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(500.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::PageUp));
        assert_eq!(scroll.scroll_y, 400.0);
    }

    #[test]
    fn scroll_vim_half_page_up() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(500.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::HalfPageUp));
        assert_eq!(scroll.scroll_y, 450.0);
    }

    #[test]
    fn scroll_vim_up_count() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);
        scroll.scroll_to(200.0);

        scroll.handle_vim_command(&VimCommand::motion_with_count(3, VimMotion::Up));
        // 3 lines up: 200 - 3*1.0 = 197
        assert_eq!(scroll.scroll_y, 197.0);
    }

    #[test]
    fn scroll_vim_line_motion() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(1000.0);
        scroll.set_viewport_height(100.0);

        scroll.handle_vim_command(&VimCommand::motion(VimMotion::Line(10)));
        // scroll_to_line(10): 10 * 1.0 = 10, clamped to max 900
        assert_eq!(scroll.scroll_y, 10.0);
    }

    #[test]
    fn scroll_vim_search_prev() {
        let mut scroll = ScrollView::new();
        scroll.set_content_height(2000.0);
        scroll.set_viewport_height(100.0);
        scroll.vim_state.search_forward = true;

        scroll.search("test");
        scroll.add_search_match(10);
        scroll.add_search_match(50);

        // SearchPrev with forward=true calls goto_prev_match
        scroll.handle_vim_command(&VimCommand::motion(VimMotion::SearchPrev));
        // goes to last match (wraps): line 50
        assert_eq!(scroll.current_match, 1);
    }

    #[test]
    fn scroll_vim_unhandled_returns_ignored() {
        let mut scroll = ScrollView::new();
        let result = scroll.handle_vim_command(&VimCommand::motion(VimMotion::Word));
        assert!(matches!(result, VimResult::Ignored));
    }
}
