//! Select (dropdown) widget.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{FocusAction, View};
use crate::event::{Key, KeyEvent};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    AppMessage, Constraints, Content, Dimension, Display, Expanded, FlexDirection, RegionId, Role,
    Semantics, SetPosition, TextContent,
};

/// A select widget with a list of options.
///
/// When collapsed, shows `▼ selected_option`.
/// When expanded, shows all options with a cursor and selection marker;
/// natively each option is a region of its own.
#[derive(Debug)]
pub struct Select {
    options: Vec<String>,
    selected: usize,
    expanded: bool,
    cursor: usize,
    id: Option<String>,
    accessible: super::Accessible,
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
            accessible: super::Accessible::default(),
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

    /// The accessible name, such as "Theme".
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.accessible.label = Some(label.into());
        self
    }

    /// Longer help text for assistive technology.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.accessible.description = Some(description.into());
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

    /// The line drawn for option `index` while expanded: the cursor, the
    /// mark of the selected option, the option.
    fn option_line(&self, index: usize, option: &str) -> String {
        let cursor = if index == self.cursor { "> " } else { "  " };
        let mark = if index == self.selected { "* " } else { "  " };
        format!("{cursor}{mark}{option}")
    }
}

impl View for Select {
    /// A combo box whose value is the selected option, expanded or not;
    /// its options are its children while it is expanded.
    fn semantics(&self) -> Semantics {
        self.accessible.apply(Semantics {
            role: Role::ComboBox,
            expanded: if self.expanded {
                Expanded::Expanded
            } else {
                Expanded::Collapsed
            },
            value_text: Some(self.selected_value().to_string()),
            ..Default::default()
        })
    }

    fn widget_id(&self) -> Option<&str> {
        Select::widget_id(self)
    }

    fn style(&self) -> ultty_protocol::Style {
        let theme = Theme::current();
        let is_focused = super::is_widget_focused(self.widget_id());
        let fg = if is_focused {
            theme.control.indicator
        } else {
            theme.control.fg
        };
        ultty_protocol::Style::new().fg(fg)
    }

    /// The selected option while collapsed; nothing while expanded, when
    /// the options are child regions.
    fn content(&self) -> Content {
        if self.expanded {
            return Content::None;
        }
        let is_focused = super::is_widget_focused(self.widget_id());
        let focus_prefix = if is_focused { ">> " } else { "" };
        Content::Text(TextContent {
            text: format!("{}\u{25bc} {}", focus_prefix, self.selected_value()),
            wrap: ultty_protocol::TextWrap::None,
            align: ultty_protocol::TextAlign::Left,
            overflow: ultty_protocol::TextOverflow::Clip,
        })
    }

    fn children_count(&self) -> usize {
        if self.expanded {
            self.options.len()
        } else {
            0
        }
    }

    /// Option `index`, while expanded: an `Option` region named by the
    /// option, with its place among the options and `selected` on the one
    /// under the cursor, which Enter would choose (the combo box's value
    /// stays the chosen one until then). The line drawn in it is hidden
    /// so the option is not read twice.
    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let Some(option) = self.options.get(index).filter(|_| self.expanded) else {
            return Ok(None);
        };
        let wire = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let under_cursor = index == self.cursor;
        // The option under the cursor is highlighted across the row, as
        // a `List`'s selected row is.
        let (row_style, line_style) = if under_cursor {
            let theme = Theme::current();
            let highlight = ultty_protocol::Style::new()
                .fg(theme.selection.fg)
                .bg(theme.selection.bg)
                .bold();
            (highlight.clone(), highlight)
        } else {
            (ultty_protocol::Style::default(), self.style())
        };

        let option_id = RegionId::new(*next_id);
        *next_id += 1;
        send_message(&AppMessage::CreateRegion {
            id: option_id,
            parent,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(1.0),
                ..Default::default()
            },
            style: row_style,
        })?;
        let semantics = Semantics {
            role: Role::Option,
            label: Some(option.clone()),
            position: Some(SetPosition {
                index: wire(index + 1),
                size: wire(self.options.len()),
            }),
            selected: under_cursor,
            ..Semantics::default()
        };
        super::send_semantics(send_message, option_id, semantics, &Content::None)?;

        let line_id = RegionId::new(*next_id);
        *next_id += 1;
        send_message(&AppMessage::CreateRegion {
            id: line_id,
            parent: option_id,
            insert_index: -1,
            constraints: Constraints::default(),
            style: line_style,
        })?;
        let content = Content::Text(TextContent::new(self.option_line(index, option)));
        let hidden = Semantics {
            hidden: true,
            ..Semantics::default()
        };
        super::send_semantics(send_message, line_id, hidden, &content)?;
        send_message(&AppMessage::SetContent {
            id: line_id,
            content,
        })?;
        Ok(Some(option_id))
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        if self.expanded {
            match &key.key {
                Key::Up | Key::Char('k') => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    }
                    Some(FocusAction::Consumed)
                }
                Key::Down | Key::Char('j') => {
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
                Key::Tab if key.modifiers.shift => Some(FocusAction::Previous),
                Key::Tab => Some(FocusAction::Next),
                _ => None,
            }
        }
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        use ratatui::text::Span;

        let is_focused = super::is_widget_focused(self.widget_id());

        let theme = Theme::current();
        if self.expanded {
            let mut lines = Vec::new();
            for (i, option) in self.options.iter().enumerate() {
                let style = if i == self.cursor {
                    Style::default()
                        .fg(to_ratatui_color(theme.selection.fg))
                        .bg(to_ratatui_color(theme.selection.bg))
                } else {
                    Style::default().fg(to_ratatui_color(theme.control.fg))
                };
                lines.push(Line::styled(self.option_line(i, option), style));
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
    use crate::focus::FocusManager;

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
    fn shift_tab_moves_focus_previous_when_collapsed() {
        let mut s = Select::new(sample_options());
        let key = KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers {
                shift: true,
                ..crate::event::Modifiers::default()
            },
            focused_id: None,
        };
        let result = s.on_key(&key);
        assert!(matches!(result, Some(FocusAction::Previous)));
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        }
    }

    /// The text drawn by each region and the semantics sent for it, in
    /// the order a native render sends them.
    fn native(s: &Select) -> (Vec<String>, Vec<(u32, Semantics)>) {
        let mut texts = Vec::new();
        let mut semantics = Vec::new();
        for message in crate::widgets::render_view_messages(s).unwrap() {
            match message {
                AppMessage::SetContent {
                    content: Content::Text(text),
                    ..
                } => texts.push(text.text),
                AppMessage::SetSemantics {
                    id,
                    semantics: sent,
                } => semantics.push((id.0, sent)),
                _ => {}
            }
        }
        (texts, semantics)
    }

    #[test]
    fn collapsed_native_is_one_combo_box_valued_by_the_selection() {
        let s = Select::new(sample_options()).selected(1).label("Colour");
        let (texts, semantics) = native(&s);
        assert_eq!(texts, ["\u{25bc} Green"]);
        assert_eq!(semantics.len(), 1, "no option regions while collapsed");
        let (id, combo) = &semantics[0];
        assert_eq!(*id, 1);
        assert_eq!(combo.role, Role::ComboBox);
        assert_eq!(combo.label.as_deref(), Some("Colour"));
        assert_eq!(combo.value_text.as_deref(), Some("Green"));
        assert_eq!(combo.expanded, Expanded::Collapsed);
    }

    #[test]
    fn expanded_native_highlights_the_option_under_the_cursor() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let mut s = Select::new(sample_options());
        s.on_key(&key(Key::Enter));
        s.on_key(&key(Key::Down));
        // Regions 2, 4 and 6 are the option rows (each with its line).
        let backgrounds: Vec<_> = crate::widgets::render_view_messages(&s)
            .unwrap()
            .into_iter()
            .filter_map(|message| match message {
                AppMessage::CreateRegion { id, style, .. } if [2, 4, 6].contains(&id.0) => {
                    Some(style.bg)
                }
                _ => None,
            })
            .collect();
        assert_eq!(backgrounds, [None, Some(theme.selection.bg), None]);
    }

    #[test]
    fn expanded_native_draws_each_option_in_an_option_region() {
        let mut s = Select::new(sample_options()).selected(1);
        s.on_key(&key(Key::Enter));
        s.on_key(&key(Key::Down));
        assert!(s.is_expanded());
        let (texts, semantics) = native(&s);
        assert_eq!(texts, ["    Red", "  * Green", ">   Blue"]);

        let (_, combo) = &semantics[0];
        assert_eq!(combo.role, Role::ComboBox);
        assert_eq!(combo.expanded, Expanded::Expanded);
        assert_eq!(
            combo.value_text.as_deref(),
            Some("Green"),
            "the value is the chosen option until Enter"
        );
        let options: Vec<_> = semantics
            .iter()
            .filter(|(_, semantics)| semantics.role == Role::Option)
            .map(|(_, option)| {
                (
                    option.label.as_deref().unwrap_or_default(),
                    option
                        .position
                        .map(|position| (position.index, position.size)),
                    option.selected,
                )
            })
            .collect();
        assert_eq!(
            options,
            [
                ("Red", Some((1, 3)), false),
                ("Green", Some((2, 3)), false),
                ("Blue", Some((3, 3)), true),
            ],
            "the option under the cursor is the selected one"
        );
        let hidden_lines = semantics
            .iter()
            .filter(|(_, semantics)| semantics.hidden)
            .count();
        assert_eq!(hidden_lines, 3, "the drawn lines are not read again");
    }

    #[test]
    fn expanded_j_k_navigation_matches_arrows() {
        let mut s = Select::new(sample_options());
        let enter = KeyEvent {
            key: Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        s.on_key(&enter);

        let j = KeyEvent {
            key: Key::Char('j'),
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        let k = KeyEvent {
            key: Key::Char('k'),
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };

        s.on_key(&j);
        s.on_key(&enter);
        assert_eq!(s.selected_index(), 1);

        s.on_key(&enter);
        s.on_key(&k);
        s.on_key(&enter);
        assert_eq!(s.selected_index(), 0);
    }

    #[test]
    fn focused_select_collapsed_content_has_focus_prefix() {
        let old = FocusManager::set_current(FocusManager::new());
        FocusManager::with_current_mut(|fm| {
            fm.register("color".to_string());
            fm.focus("color");
        });

        let s = Select::new(sample_options()).id("color");
        match s.content() {
            ultty_protocol::Content::Text(tc) => {
                assert!(tc.text.starts_with(">> "));
            }
            _ => panic!("Expected Text content"),
        }

        FocusManager::set_current(old);
    }
}
