//! Scroll view widget with vim navigation.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::View;
use crate::vim::{VimConfig, VimExt, VimHandler, VimResult};
use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimState};
use ultty_protocol::{AppMessage, Constraints, OverflowAxis, RegionId};

pub struct ScrollView {
    constraints: Constraints,
    child: Option<Box<dyn View>>,
    scroll_x: f32,
    scroll_y: f32,
    content_height: f32,
    viewport_height: f32,
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
            .finish()
    }
}

impl Default for ScrollView {
    fn default() -> Self {
        let mut constraints = Constraints::default();
        constraints.overflow.y = OverflowAxis::Scroll;
        Self {
            constraints,
            child: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            content_height: 0.0,
            viewport_height: 0.0,
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

    pub fn scroll_position(&self) -> (f32, f32) {
        (self.scroll_x, self.scroll_y)
    }

    pub fn set_content_height(&mut self, height: f32) {
        self.content_height = height;
    }

    pub fn set_viewport_height(&mut self, height: f32) {
        self.viewport_height = height;
    }

    pub fn scroll_to(&mut self, y: f32) {
        let max_scroll = (self.content_height - self.viewport_height).max(0.0);
        self.scroll_y = y.clamp(0.0, max_scroll);
    }

    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll_to(self.scroll_y + delta);
    }

    fn line_height(&self) -> f32 {
        16.0
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

    fn scroll_to_bottom(&mut self) {
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

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(child) = &self.child {
            child.render_ansi(frame, inner);
        } else {
            let scroll_info = format!(
                "Scroll: {:.0}/{:.0}",
                self.scroll_y,
                max_scroll
            );
            let paragraph = Paragraph::new(Line::raw(scroll_info));
            frame.render_widget(paragraph, inner);
        }
    }

    fn is_focusable(&self) -> bool {
        true
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

impl VimExt for ScrollView {
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
}
