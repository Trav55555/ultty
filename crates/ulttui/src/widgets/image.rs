//! Image widget for displaying inline images.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
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
        let scale_label = match self.scale {
            ImageScale::Fit => "fit",
            ImageScale::Fill => "fill",
            ImageScale::Stretch => "stretch",
            ImageScale::None => "none",
        };
        let title = format!("Image {}x{} ({})", self.width, self.height, scale_label);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));

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
                .style(Style::default().fg(Color::DarkGray));
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
