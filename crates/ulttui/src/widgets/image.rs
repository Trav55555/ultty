//! Image widget for displaying inline images.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use ultty_protocol::{
    Content, ImageScale, ImageWidget as ProtoImageWidget, Role, Semantics, WidgetContent,
};

use super::View;
use crate::theme::{to_ratatui_color, Theme};

/// Image widget that displays inline RGBA pixel data.
pub struct Image {
    id: Option<String>,
    data: Vec<u8>,
    width: u32,
    height: u32,
    scale: ImageScale,
    alt: Option<String>,
}

impl Image {
    /// Give this widget an id so focus, `ulttyctl regions` and user overrides
    /// (ADR 0023) can reach it.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Create a new image widget from RGBA pixel data.
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            id: None,
            data,
            width,
            height,
            scale: ImageScale::Fit,
            alt: None,
        }
    }

    /// Set the scaling mode.
    pub fn scale(mut self, scale: ImageScale) -> Self {
        self.scale = scale;
        self
    }

    /// The alt text: what the image shows, for assistive technology. An
    /// image without one is unnamed.
    pub fn label(mut self, alt: impl Into<String>) -> Self {
        self.alt = Some(alt.into());
        self
    }
}

impl View for Image {
    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn semantics(&self) -> Semantics {
        Semantics {
            role: Role::Image,
            label: self.alt.clone(),
            ..Semantics::default()
        }
    }

    fn content(&self) -> Content {
        Content::Widget(WidgetContent::Image(ProtoImageWidget {
            data: self.data.clone(),
            width: self.width,
            height: self.height,
            scale: self.scale,
        }))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let scale_label = match self.scale {
            ImageScale::Fit => "fit",
            ImageScale::Fill => "fill",
            ImageScale::Stretch => "stretch",
            ImageScale::None => "none",
        };
        let theme = Theme::current();
        let title = format!("Image {}x{} ({})", self.width, self.height, scale_label);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(to_ratatui_color(theme.muted)));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Show a placeholder pattern in the inner area
        if inner.height > 0 && inner.width > 0 {
            let text = format!(
                "{}x{} RGBA ({} bytes)",
                self.width,
                self.height,
                self.data.len()
            );
            let paragraph = Paragraph::new(Line::from(text))
                .style(Style::default().fg(to_ratatui_color(theme.muted)));
            frame.render_widget(paragraph, inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_content() {
        let data = vec![255u8; 16]; // 2x2 RGBA
        let img = Image::new(data.clone(), 2, 2);
        match <Image as View>::content(&img) {
            Content::Widget(WidgetContent::Image(iw)) => {
                assert_eq!(iw.width, 2);
                assert_eq!(iw.height, 2);
                assert_eq!(iw.data, data);
                assert_eq!(iw.scale, ImageScale::Fit);
            }
            _ => panic!("Expected Image widget content"),
        }
    }

    #[test]
    fn image_scale() {
        let img = Image::new(vec![], 10, 10).scale(ImageScale::Fill);
        match <Image as View>::content(&img) {
            Content::Widget(WidgetContent::Image(iw)) => {
                assert_eq!(iw.scale, ImageScale::Fill);
            }
            _ => panic!("Expected Image widget content"),
        }
    }

    #[test]
    fn image_not_focusable() {
        let img = Image::new(vec![], 1, 1);
        assert!(!img.is_focusable());
    }
}
