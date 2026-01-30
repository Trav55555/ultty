//! Built-in widgets.

mod button;
mod container;
mod list;
mod scroll;
mod text;

pub use button::Button;
pub use container::{Column, Row, Stack};
pub use list::List;
pub use scroll::ScrollView;
pub use text::{RichText, Text, TextInput};

use ratatui::layout::Rect;
use ratatui::Frame;

/// Trait for renderable views.
pub trait View {
    /// Get the constraints for this view.
    fn constraints(&self) -> ultty_protocol::Constraints {
        ultty_protocol::Constraints::default()
    }

    /// Render this view to an ANSI terminal using ratatui.
    /// Default implementation does nothing (useful for containers that delegate to children).
    fn render_ansi(&self, _frame: &mut Frame, _area: Rect) {}
}

/// Blanket impl for references.
impl<T: View> View for &T {
    fn constraints(&self) -> ultty_protocol::Constraints {
        (*self).constraints()
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        (*self).render_ansi(frame, area)
    }
}
