//! Tabs widget for switching between views.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Constraints, Dimension, Display, FlexDirection, RegionId, Role, Semantics,
    SetPosition, Style as ProtoStyle, TextContent,
};

/// A tab bar widget.
#[derive(Default)]
pub struct Tabs {
    id: Option<String>,
    labels: Vec<String>,
    active: usize,
}

impl std::fmt::Debug for Tabs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tabs")
            .field("labels", &self.labels)
            .field("active", &self.active)
            .finish()
    }
}

impl Tabs {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn labels(mut self, labels: Vec<impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    pub fn active(mut self, index: usize) -> Self {
        self.active = index;
        self
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_label(&self) -> Option<&str> {
        self.labels.get(self.active).map(|s| s.as_str())
    }

    pub fn tab_count(&self) -> usize {
        self.labels.len()
    }

    pub fn select_next(&mut self) {
        if !self.labels.is_empty() {
            self.active = (self.active + 1) % self.labels.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.labels.is_empty() {
            if self.active == 0 {
                self.active = self.labels.len() - 1;
            } else {
                self.active -= 1;
            }
        }
    }

    pub fn select(&mut self, index: usize) {
        if index < self.labels.len() {
            self.active = index;
        }
    }
}

impl View for Tabs {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            gap: ultty_protocol::Size {
                width: Dimension::Cells(1.0),
                height: Dimension::Auto,
            },
            ..Default::default()
        }
    }

    fn semantics(&self) -> Semantics {
        Semantics {
            role: Role::TabList,
            ..Semantics::default()
        }
    }

    fn children_count(&self) -> usize {
        self.labels.len()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let label = match self.labels.get(index) {
            Some(l) => l,
            None => return Ok(None),
        };

        let region_id = RegionId::new(*next_id);
        *next_id += 1;

        let theme = Theme::current();
        let is_active = index == self.active;
        let style = if is_active {
            ProtoStyle::new()
                .fg(theme.selection.fg)
                .bg(theme.selection.bg)
                .bold()
        } else {
            ProtoStyle::new().fg(theme.foreground)
        };

        send_message(&AppMessage::CreateRegion {
            id: region_id,
            parent,
            insert_index: -1,
            constraints: Constraints::default(),
            style,
        })?;

        let display_text = format!(" {} ", label);
        let content = ultty_protocol::Content::Text(TextContent::new(display_text));
        let tab = Semantics {
            role: Role::Tab,
            label: Some(label.clone()),
            position: Some(SetPosition {
                index: index as u32 + 1,
                size: self.labels.len() as u32,
            }),
            selected: is_active,
            ..Semantics::default()
        };
        super::send_semantics(send_message, region_id, tab, &content)?;
        send_message(&AppMessage::SetContent {
            id: region_id,
            content,
        })?;

        Ok(Some(region_id))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let spans: Vec<Span> = self
            .labels
            .iter()
            .enumerate()
            .flat_map(|(i, label)| {
                let is_active = i == self.active;
                let style = if is_active {
                    Style::default()
                        .fg(to_ratatui_color(theme.selection.fg))
                        .bg(to_ratatui_color(theme.selection.bg))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(to_ratatui_color(theme.muted))
                };

                let mut spans = vec![Span::styled(format!(" {} ", label), style)];
                if i < self.labels.len() - 1 {
                    spans.push(Span::raw(" "));
                }
                spans
            })
            .collect();

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &crate::event::KeyEvent) -> Option<super::FocusAction> {
        use crate::event::Key;
        match key.key {
            Key::Left | Key::Char('h') => {
                self.select_prev();
                Some(super::FocusAction::Consumed)
            }
            Key::Right | Key::Char('l') => {
                self.select_next();
                Some(super::FocusAction::Consumed)
            }
            Key::Char(c) if c.is_ascii_digit() => {
                let idx = c as usize - '0' as usize;
                if idx > 0 && idx <= self.labels.len() {
                    self.select(idx - 1);
                }
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
    use crate::event::{Key, KeyEvent, Modifiers};
    use crate::widgets::FocusAction;

    #[test]
    fn tabs_navigation() {
        let mut tabs = Tabs::new().labels(vec!["One", "Two", "Three"]);
        assert_eq!(tabs.active_index(), 0);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 1);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 2);

        tabs.select_next();
        assert_eq!(tabs.active_index(), 0); // wraps

        tabs.select_prev();
        assert_eq!(tabs.active_index(), 2); // wraps back
    }

    #[test]
    fn tabs_select() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        tabs.select(2);
        assert_eq!(tabs.active_index(), 2);
        assert_eq!(tabs.active_label(), Some("C"));

        tabs.select(99); // out of bounds, no change
        assert_eq!(tabs.active_index(), 2);
    }

    #[test]
    fn tabs_empty() {
        let mut tabs = Tabs::new();
        tabs.select_next(); // should not panic
        tabs.select_prev();
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active_label(), None);
    }

    // --- New tests below ---

    #[test]
    fn tabs_default_values() {
        let tabs = Tabs::default();
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.tab_count(), 0);
        assert_eq!(tabs.active_label(), None);
    }

    #[test]
    fn tabs_new_is_default() {
        let tabs = Tabs::new();
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.tab_count(), 0);
    }

    #[test]
    fn tabs_builder_active() {
        let tabs = Tabs::new().labels(vec!["A", "B", "C"]).active(2);
        assert_eq!(tabs.active_index(), 2);
        assert_eq!(tabs.active_label(), Some("C"));
    }

    #[test]
    fn tabs_tab_count() {
        let tabs = Tabs::new().labels(vec!["X", "Y", "Z"]);
        assert_eq!(tabs.tab_count(), 3);
    }

    #[test]
    fn tabs_tab_count_empty() {
        let tabs = Tabs::new();
        assert_eq!(tabs.tab_count(), 0);
    }

    #[test]
    fn tabs_single_label_wrap() {
        let mut tabs = Tabs::new().labels(vec!["Only"]);
        tabs.select_next();
        assert_eq!(tabs.active_index(), 0);
        tabs.select_prev();
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn tabs_select_zero() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        tabs.select(2);
        tabs.select(0);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active_label(), Some("A"));
    }

    #[test]
    fn tabs_select_boundary() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]);
        tabs.select(1);
        assert_eq!(tabs.active_index(), 1);
        tabs.select(2); // out of bounds, no change
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn tabs_prev_wraps_from_zero() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        assert_eq!(tabs.active_index(), 0);
        tabs.select_prev();
        assert_eq!(tabs.active_index(), 2);
    }

    #[test]
    fn tabs_debug_format() {
        let tabs = Tabs::new().labels(vec!["Home", "Settings"]);
        let debug = format!("{:?}", tabs);
        assert!(debug.contains("Tabs"));
        assert!(debug.contains("Home"));
        assert!(debug.contains("active"));
    }

    #[test]
    fn tabs_constraints() {
        let tabs = Tabs::new().labels(vec!["A"]);
        let c = <Tabs as View>::constraints(&tabs);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Row);
        assert_eq!(c.width, Dimension::Percent(100.0));
        assert_eq!(c.height, Dimension::Cells(1.0));
    }

    #[test]
    fn tabs_children_count_matches_labels() {
        let tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        assert_eq!(<Tabs as View>::children_count(&tabs), 3);
    }

    #[test]
    fn tabs_children_count_empty() {
        let tabs = Tabs::new();
        assert_eq!(<Tabs as View>::children_count(&tabs), 0);
    }

    #[test]
    fn tabs_is_focusable() {
        let tabs = Tabs::new();
        assert!(tabs.is_focusable());
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
    fn tabs_on_key_right_selects_next() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]);
        let result = tabs.on_key(&key(Key::Right));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn tabs_on_key_left_selects_prev() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]).active(1);
        let result = tabs.on_key(&key(Key::Left));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn tabs_on_key_h_selects_prev() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]).active(1);
        let result = tabs.on_key(&key(Key::Char('h')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn tabs_on_key_l_selects_next() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]);
        let result = tabs.on_key(&key(Key::Char('l')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn tabs_on_key_digit_selects_tab() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]);
        let result = tabs.on_key(&key(Key::Char('2')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 1); // digit '2' -> index 1
    }

    #[test]
    fn tabs_on_key_digit_1_selects_first() {
        let mut tabs = Tabs::new().labels(vec!["A", "B", "C"]).active(2);
        let result = tabs.on_key(&key(Key::Char('1')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn tabs_on_key_digit_0_no_change() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]).active(1);
        let result = tabs.on_key(&key(Key::Char('0')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        // '0' maps to index -1 which fails the > 0 check, so no change
        assert_eq!(tabs.active_index(), 1);
    }

    #[test]
    fn tabs_on_key_digit_out_of_range() {
        let mut tabs = Tabs::new().labels(vec!["A", "B"]);
        let result = tabs.on_key(&key(Key::Char('5')));
        assert!(matches!(result, Some(FocusAction::Consumed)));
        // digit 5 -> index 4 but only 2 labels, so no change
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn tabs_on_key_tab_focus_next() {
        let mut tabs = Tabs::new().labels(vec!["A"]);
        let result = tabs.on_key(&key(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Next)));
    }

    #[test]
    fn tabs_on_key_shift_tab_focus_previous() {
        let mut tabs = Tabs::new().labels(vec!["A"]);
        let result = tabs.on_key(&key_with_shift(Key::Tab));
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    #[test]
    fn tabs_on_key_unhandled_returns_none() {
        let mut tabs = Tabs::new().labels(vec!["A"]);
        let result = tabs.on_key(&key(Key::Char('z')));
        assert!(result.is_none());
    }

    #[test]
    fn tabs_active_label_returns_correct_label() {
        let tabs = Tabs::new()
            .labels(vec!["Home", "About", "Contact"])
            .active(1);
        assert_eq!(tabs.active_label(), Some("About"));
    }
}
