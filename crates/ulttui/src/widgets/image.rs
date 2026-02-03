//! Image widget for displaying inline images.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use ultty_protocol::{Content, ImageScale, ImageWidget as ProtoImageWidget, WidgetContent};

use super::View;

/// Image widget that displays inline RGBA pixel data.
pub struct Image {
    data: Vec<u8>,
    width: u32,
    height: u32,
    scale: ImageScale,
}

impl Image {
    /// Create a new image widget from RGBA pixel data.
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            data,
            width,
            height,
            scale: ImageScale::Fit,
        }
    }

    /// Set the scaling mode.
    pub fn scale(mut self, scale: ImageScale) -> Self {
        self.scale = scale;
        self
    }
}

impl View for Image {
    fn content(&self) -> Content {
        Content::Widget(WidgetContent::Image(ProtoImageWidget {
            data: self.data.clone(),
            width: self.width,
            height: self.height,
            scale: self.scale,
        }))
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        let text = format!("[Image: {}x{}]", self.width, self.height);
        let paragraph = Paragraph::new(Line::from(text)).style(Style::default());
        frame.render_widget(paragraph, area);
    }
}
