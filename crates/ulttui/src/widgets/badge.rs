//! Badge/tag widget for status indicators.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::View;
use crate::theme::{to_ratatui_color, Theme};
use ultty_protocol::{
    Constraints, Content, Dimension, RichText as ProtoRichText, Style as ProtoStyle,
};

/// Badge variants for semantic color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BadgeVariant {
    Primary,
    Success,
    Error,
    Warning,
}

/// Small badge for status indicators.
#[derive(Debug, Clone)]
pub struct Badge {
    text: String,
    variant: BadgeVariant,
}

impl Badge {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            variant: BadgeVariant::Primary,
        }
    }

    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            variant: BadgeVariant::Success,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            variant: BadgeVariant::Error,
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            variant: BadgeVariant::Warning,
        }
    }
}

impl View for Badge {
    fn constraints(&self) -> Constraints {
        let width = self.text.chars().count() + 2; // +2 for padding spaces
        Constraints {
            display: ultty_protocol::Display::Flex,
            width: Dimension::Cells(width as f32),
            height: Dimension::Cells(1.0),
            ..Default::default()
        }
    }

    fn content(&self) -> Content {
        let theme = Theme::current();
        let color = match self.variant {
            BadgeVariant::Primary => theme.primary,
            BadgeVariant::Success => theme.success,
            BadgeVariant::Error => theme.error,
            BadgeVariant::Warning => theme.warning,
        };
        let style = ProtoStyle::new().fg(color);
        Content::RichText(ProtoRichText::new().span(format!(" {} ", self.text), style))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let theme = Theme::current();
        let color = match self.variant {
            BadgeVariant::Primary => theme.primary,
            BadgeVariant::Success => theme.success,
            BadgeVariant::Error => theme.error,
            BadgeVariant::Warning => theme.warning,
        };
        let style = Style::default().fg(to_ratatui_color(color));
        let text = format!(" {} ", self.text);
        let paragraph = Paragraph::new(text).style(style);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_variant_is_primary() {
        let badge = Badge::new("Info");
        assert_eq!(badge.variant, BadgeVariant::Primary);
    }

    #[test]
    fn success_constructor() {
        let badge = Badge::success("OK");
        assert_eq!(badge.variant, BadgeVariant::Success);
    }

    #[test]
    fn error_constructor() {
        let badge = Badge::error("No");
        assert_eq!(badge.variant, BadgeVariant::Error);
    }

    #[test]
    fn warning_constructor() {
        let badge = Badge::warning("Careful");
        assert_eq!(badge.variant, BadgeVariant::Warning);
    }

    #[test]
    fn variant_builder() {
        let badge = Badge::new("X").variant(BadgeVariant::Success);
        assert_eq!(badge.variant, BadgeVariant::Success);
    }

    #[test]
    fn not_focusable() {
        let badge = Badge::new("Info");
        assert!(!badge.is_focusable());
    }

    #[test]
    fn content_returns_richtext() {
        let badge = Badge::new("Info");
        match badge.content() {
            Content::RichText(rt) => {
                assert_eq!(rt.spans.len(), 1);
                assert!(rt.spans[0].text.contains("Info"));
            }
            _ => panic!("Expected RichText content"),
        }
    }

    #[test]
    fn success_variant_uses_success_color() {
        let badge = Badge::success("OK");
        let theme = Theme::current();
        match badge.content() {
            Content::RichText(rt) => {
                assert_eq!(rt.spans[0].style.fg, Some(theme.success));
            }
            _ => panic!("Expected RichText content"),
        }
    }
}
