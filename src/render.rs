use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

/// Represents the virtual screen state.
/// We use Ratatui's `Buffer` as our primitive 2D grid of cells.
pub struct RenderBuffer {
    pub buffer: Buffer,
    pub area: Rect,
}

impl RenderBuffer {
    /// Create a new empty buffer of the given size.
    pub fn new(width: u16, height: u16) -> Self {
        let area = Rect::new(0, 0, width, height);
        // Ratatui's buffer initialization
        let buffer = Buffer::empty(area);
        Self { buffer, area }
    }

    /// Resize the backing buffer.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.area = Rect::new(0, 0, width, height);
        self.buffer.resize(self.area);
    }
}

/// A "Frame" that abstracts the drawing process.
/// In our architecture, the Layout engine will provide the `Rect`s,
/// and the Render engine will draw into this Frame.
pub struct Frame<'a> {
    pub buffer: &'a mut Buffer,
    pub cursor_position: Option<(u16, u16)>,
}

impl<'a> Frame<'a> {
    pub fn new(buffer: &'a mut Buffer) -> Self {
        Self {
            buffer,
            cursor_position: None,
        }
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }
}
