//! Semantic text primitives that use theme design tokens.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style as RStyle};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{expand_tabs, View};
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{Constraints, Content, Dimension, Style as ProtoStyle};

/// Large section heading. Uses `text_title` color, bold.
///
/// For assistive technology it is a heading, of level 1 unless
/// [`Title::level`] says otherwise.
pub struct Title {
    id: Option<String>,
    text: String,
    level: u8,
}

impl Title {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: None,
            text: expand_tabs(&text.into()).into_owned(),
            level: 1,
        }
    }

    /// The heading's depth: 1 for the top, 2 for a section, and so on.
    pub fn level(mut self, level: u8) -> Self {
        self.level = level.max(1);
        self
    }
}

impl View for Title {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn semantics(&self) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics {
            role: ultty_protocol::Role::Heading,
            level: self.level,
            ..Default::default()
        }
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            width: Dimension::Cells(crate::overrides::text_or(&self.text).chars().count() as f32),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        let theme = Theme::current();
        let style = ProtoStyle::new().fg(theme.tokens.text_title).bold();
        Content::RichText(
            ultty_protocol::RichText::new().span(crate::overrides::text_or(&self.text), style),
        )
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let style = RStyle::default()
            .fg(to_ratatui_color(theme.tokens.text_title))
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::styled(crate::overrides::text_or(&self.text), style)),
            area,
        );
    }
}

/// Standard body text. Uses `text_body` color.
pub struct Body {
    id: Option<String>,
    text: String,
}

impl Body {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: None,
            text: expand_tabs(&text.into()).into_owned(),
        }
    }
}

impl View for Body {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            width: Dimension::Cells(crate::overrides::text_or(&self.text).chars().count() as f32),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        let theme = Theme::current();
        let style = ProtoStyle::new().fg(theme.tokens.text_body);
        Content::RichText(
            ultty_protocol::RichText::new().span(crate::overrides::text_or(&self.text), style),
        )
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let style = RStyle::default().fg(to_ratatui_color(theme.tokens.text_body));
        frame.render_widget(
            Paragraph::new(Line::styled(crate::overrides::text_or(&self.text), style)),
            area,
        );
    }
}

/// De-emphasized text. Uses `text_muted` color.
pub struct Muted {
    id: Option<String>,
    text: String,
}

impl Muted {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: None,
            text: expand_tabs(&text.into()).into_owned(),
        }
    }
}

impl View for Muted {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            width: Dimension::Cells(crate::overrides::text_or(&self.text).chars().count() as f32),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        let theme = Theme::current();
        let style = ProtoStyle::new().fg(theme.tokens.text_muted);
        Content::RichText(
            ultty_protocol::RichText::new().span(crate::overrides::text_or(&self.text), style),
        )
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let style = RStyle::default().fg(to_ratatui_color(theme.tokens.text_muted));
        frame.render_widget(
            Paragraph::new(Line::styled(crate::overrides::text_or(&self.text), style)),
            area,
        );
    }
}

/// Emphasized text. Uses `text_accent` color, bold.
pub struct Accent {
    id: Option<String>,
    text: String,
}

impl Accent {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: None,
            text: expand_tabs(&text.into()).into_owned(),
        }
    }
}

impl View for Accent {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn constraints(&self) -> Constraints {
        Constraints {
            width: Dimension::Cells(crate::overrides::text_or(&self.text).chars().count() as f32),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        let theme = Theme::current();
        let style = ProtoStyle::new().fg(theme.tokens.text_accent).bold();
        Content::RichText(
            ultty_protocol::RichText::new().span(crate::overrides::text_or(&self.text), style),
        )
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let theme = Theme::current();
        let style = RStyle::default()
            .fg(to_ratatui_color(theme.tokens.text_accent))
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::styled(crate::overrides::text_or(&self.text), style)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typography_expands_tabs_at_construction() {
        assert_eq!(Title::new("ab\tc").text, "ab      c");
        assert_eq!(Body::new("ab\tc").text, "ab      c");
        assert_eq!(Muted::new("ab\tc").text, "ab      c");
        assert_eq!(Accent::new("ab\tc").text, "ab      c");
    }

    #[test]
    fn title_content_is_rich_text() {
        let t = Title::new("Hello");
        assert!(matches!(t.content(), Content::RichText(_)));
    }

    #[test]
    fn title_not_focusable() {
        let t = Title::new("Hello");
        assert!(!t.is_focusable());
    }

    #[test]
    fn title_uses_text_title_color() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let t = Title::new("Hello");
        if let Content::RichText(rt) = t.content() {
            assert_eq!(rt.spans[0].style.fg, Some(theme.tokens.text_title));
            assert!(rt.spans[0].style.bold);
        } else {
            panic!("Expected RichText");
        }
    }

    #[test]
    fn body_content_is_rich_text() {
        let b = Body::new("Hello");
        assert!(matches!(b.content(), Content::RichText(_)));
    }

    #[test]
    fn body_not_focusable() {
        let b = Body::new("Hello");
        assert!(!b.is_focusable());
    }

    #[test]
    fn body_uses_text_body_color() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let b = Body::new("Hello");
        if let Content::RichText(rt) = b.content() {
            assert_eq!(rt.spans[0].style.fg, Some(theme.tokens.text_body));
        } else {
            panic!("Expected RichText");
        }
    }

    #[test]
    fn muted_content_is_rich_text() {
        let m = Muted::new("Hello");
        assert!(matches!(m.content(), Content::RichText(_)));
    }

    #[test]
    fn muted_not_focusable() {
        let m = Muted::new("Hello");
        assert!(!m.is_focusable());
    }

    #[test]
    fn muted_uses_text_muted_color() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let m = Muted::new("Hello");
        if let Content::RichText(rt) = m.content() {
            assert_eq!(rt.spans[0].style.fg, Some(theme.tokens.text_muted));
        } else {
            panic!("Expected RichText");
        }
    }

    #[test]
    fn accent_content_is_rich_text() {
        let a = Accent::new("Hello");
        assert!(matches!(a.content(), Content::RichText(_)));
    }

    #[test]
    fn accent_not_focusable() {
        let a = Accent::new("Hello");
        assert!(!a.is_focusable());
    }

    #[test]
    fn accent_uses_text_accent_color() {
        let theme = Theme::dark();
        Theme::set_current(theme.clone());
        let a = Accent::new("Hello");
        if let Content::RichText(rt) = a.content() {
            assert_eq!(rt.spans[0].style.fg, Some(theme.tokens.text_accent));
            assert!(rt.spans[0].style.bold);
        } else {
            panic!("Expected RichText");
        }
    }
}
