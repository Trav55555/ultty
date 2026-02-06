//! Select (dropdown) widget.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};

/// A select widget with a list of options.
///
/// When collapsed, shows `▼ selected_option`.
/// When expanded, shows all options with a cursor and selection marker.
#[derive(Debug)]
pub struct Select {
    options: Vec<String>,
    selected: usize,
    expanded: bool,
    cursor: usize,
    id: Option<String>,
}

impl Select {
    /// Create a new select widget with the given options.
    pub fn new(options: Vec<String>) -> Self {
        Self {
            cursor: 0,
            selected: 0,
            expanded: false,
            options,
            id: None,
        }
    }

    /// Set the initially selected index.
    pub fn selected(mut self, index: usize) -> Self {
        if index < self.options.len() {
            self.selected = index;
            self.cursor = index;
        }
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Returns the currently selected index.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Returns the currently selected value (or empty string if no options).
    pub fn selected_value(&self) -> &str {
        self.options
            .get(self.selected)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Returns whether the select is currently expanded.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }
}

impl View for Select {
    fn widget_id(&self) -> Option<&str> {
        Select::widget_id(self)
    }

    fn content(&self) -> ultty_protocol::Content {
        let text = if self.expanded {
            let mut lines = Vec::new();
            for (i, option) in self.options.iter().enumerate() {
                let cursor = if i == self.cursor { "> " } else { "  " };
                let mark = if i == self.selected { "* " } else { "  " };
                lines.push(format!("{}{}{}", cursor, mark, option));
            }
            lines.join("\n")
        } else {
            let selected_text = self.selected_value();
            format!("\u{25bc} {}", selected_text)
        };

        ultty_protocol::Content::Text(ultty_protocol::TextContent {
            text,
            wrap: ultty_protocol::TextWrap::None,
            align: ultty_protocol::TextAlign::Left,
            overflow: ultty_protocol::TextOverflow::Clip,
        })
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        if self.expanded {
            match &key.key {
                Key::Up => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    }
                    Some(FocusAction::Consumed)
                }
                Key::Down => {
                    if self.cursor + 1 < self.options.len() {
                        self.cursor += 1;
                    }
                    Some(FocusAction::Consumed)
                }
                Key::Enter | Key::Char(' ') => {
                    self.selected = self.cursor;
                    self.expanded = false;
                    Some(FocusAction::Consumed)
                }
                Key::Escape => {
                    self.expanded = false;
                    Some(FocusAction::Consumed)
                }
                _ => None,
            }
        } else {
            match &key.key {
                Key::Enter | Key::Char(' ') => {
                    self.expanded = true;
                    self.cursor = self.selected;
                    Some(FocusAction::Consumed)
                }
                Key::Tab => Some(FocusAction::Next),
                _ => None,
            }
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::text::Span;

        // Register with focus manager if we have an ID
        if let Some(id) = self.widget_id() {
            crate::focus::FocusManager::with_current_mut(|fm| fm.register(id.to_string()));
        }
        let is_focused = self
            .widget_id()
            .map(|id| crate::focus::FocusManager::with_current(|fm| fm.is_focused(id)))
            .unwrap_or(false);

        let theme = Theme::current();
        if self.expanded {
            let mut lines = Vec::new();
            for (i, option) in self.options.iter().enumerate() {
                let cursor = if i == self.cursor { "> " } else { "  " };
                let mark = if i == self.selected { "* " } else { "  " };
                let style = if i == self.cursor {
                    Style::default()
                        .fg(to_ratatui_color(theme.selection.fg))
                        .bg(to_ratatui_color(theme.selection.bg))
                } else {
                    Style::default().fg(to_ratatui_color(theme.control.fg))
                };
                lines.push(Line::styled(format!("{}{}{}", cursor, mark, option), style));
            }
            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        } else {
            let selected_text = self.selected_value();
            let arrow = "\u{25bc} ";
            let arrow_style = if is_focused {
                Style::default().fg(to_ratatui_color(theme.control.indicator))
            } else {
                Style::default().fg(to_ratatui_color(theme.control.fg))
            };
            let text_style = Style::default().fg(to_ratatui_color(theme.control.fg));
            let line = Line::from(vec![
                Span::styled(arrow, arrow_style),
                Span::styled(selected_text, text_style),
            ]);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_options() -> Vec<String> {
        vec!["Red".to_string(), "Green".to_string(), "Blue".to_string()]
    }

    #[test]
    fn new_select_defaults() {
        let s = Select::new(sample_options());
        assert_eq!(s.selected_index(), 0);
        assert_eq!(s.selected_value(), "Red");
        assert!(!s.is_expanded());
    }

    #[test]
    fn selected_builder() {
        let s = Select::new(sample_options()).selected(2);
        assert_eq!(s.selected_index(), 2);
        assert_eq!(s.selected_value(), "Blue");
    }

    #[test]
    fn select_with_id() {
        let s = Select::new(sample_options()).id("color");
        assert_eq!(s.widget_id(), Some("color"));
    }

    #[test]
    fn selected_out_of_bounds_ignored() {
        let s = Select::new(sample_options()).selected(99);
        assert_eq!(s.selected_index(), 0);
    }

    #[test]
    fn collapsed_content() {
        let s = Select::new(sample_options());
        match s.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.contains("Red"));
                assert!(tc.text.starts_with('\u{25bc}'));
            }
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn expand_and_collapse() {
        let mut s = Select::new(sample_options());
        assert!(!s.is_expanded());

        // Enter expands
        let key = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        let result = s.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(s.is_expanded());

        // Escape collapses
        let key = KeyEvent {
            key: Key::Escape,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        let result = s.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert!(!s.is_expanded());
    }

    #[test]
    fn navigate_and_select() {
        let mut s = Select::new(sample_options());

        // Expand
        let enter = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&enter);
        assert!(s.is_expanded());

        // Move down twice
        let down = KeyEvent {
            key: Key::Down,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&down);
        s.on_key(&down);

        // Select with Enter
        s.on_key(&enter);
        assert!(!s.is_expanded());
        assert_eq!(s.selected_index(), 2);
        assert_eq!(s.selected_value(), "Blue");
    }

    #[test]
    fn cursor_bounds_respected() {
        let mut s = Select::new(sample_options());

        // Expand
        let enter = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&enter);

        // Move up at top (should stay at 0)
        let up = KeyEvent {
            key: Key::Up,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&up);
        // cursor should still be 0

        // Move down 5 times (only 3 options)
        let down = KeyEvent {
            key: Key::Down,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        for _ in 0..5 {
            s.on_key(&down);
        }
        // Select current
        s.on_key(&enter);
        assert_eq!(s.selected_index(), 2); // max index
    }

    #[test]
    fn is_focusable() {
        let s = Select::new(sample_options());
        assert!(s.is_focusable());
    }

    #[test]
    fn tab_moves_focus_when_collapsed() {
        let mut s = Select::new(sample_options());
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        let result = s.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn expanded_content_shows_all_options() {
        let mut s = Select::new(sample_options());
        let enter = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&enter);
        assert!(s.is_expanded());

        match s.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.contains("Red"));
                assert!(tc.text.contains("Green"));
                assert!(tc.text.contains("Blue"));
            }
            _ => panic!("Expected Text content"),
        }
    }
}
