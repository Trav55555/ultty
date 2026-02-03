//! Built-in widgets.

mod button;
mod container;
mod image;
mod list;
mod scroll;
mod text;

pub use button::Button;
pub use container::{Column, Row, Stack};
pub use image::Image;
pub use list::List;
pub use scroll::ScrollView;
pub use text::{RichText, Text, TextInput};

use crate::event::{Event, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

/// Trait for renderable views.
pub trait View {
    fn constraints(&self) -> ultty_protocol::Constraints {
        ultty_protocol::Constraints::default()
    }

    fn style(&self) -> ultty_protocol::Style {
        ultty_protocol::Style::default()
    }

    fn content(&self) -> ultty_protocol::Content {
        ultty_protocol::Content::None
    }

    fn children_count(&self) -> usize {
        0
    }

    fn render_child_native(
        &self,
        _index: usize,
        _send_message: &mut dyn FnMut(&ultty_protocol::AppMessage) -> anyhow::Result<()>,
        _parent: ultty_protocol::RegionId,
        _next_id: &mut u32,
    ) -> anyhow::Result<Option<ultty_protocol::RegionId>> {
        Ok(None)
    }

    fn render_ansi(&self, _frame: &mut Frame, _area: Rect) {}

    fn is_focusable(&self) -> bool {
        false
    }

    fn on_key(&mut self, _key: &KeyEvent) -> Option<FocusAction> {
        None
    }

    fn on_event(&mut self, _event: &Event) -> bool {
        false
    }
}

pub enum FocusAction {
    Consumed,
    Next,
    Previous,
}

impl<T: View> View for &T {
    fn constraints(&self) -> ultty_protocol::Constraints {
        (*self).constraints()
    }

    fn style(&self) -> ultty_protocol::Style {
        (*self).style()
    }

    fn content(&self) -> ultty_protocol::Content {
        (*self).content()
    }

    fn children_count(&self) -> usize {
        (*self).children_count()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&ultty_protocol::AppMessage) -> anyhow::Result<()>,
        parent: ultty_protocol::RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<ultty_protocol::RegionId>> {
        (*self).render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        (*self).render_ansi(frame, area)
    }

    fn is_focusable(&self) -> bool {
        (*self).is_focusable()
    }
}
