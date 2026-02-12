//! Built-in widgets.

mod badge;
mod button;
mod checkbox;
mod container;
mod divider;
mod image;
mod list;
mod modal;
mod primitives;
mod progress;
mod scroll;
mod select;
mod spinner;
mod state;
mod table;
mod tabs;
mod text;
mod toggle;
mod typography;

pub use badge::{Badge, BadgeVariant};
pub use button::Button;
pub use checkbox::Checkbox;
pub use container::{Column, Row, Stack};
pub use divider::Divider;
pub use image::Image;
pub use list::List;
pub use modal::Modal;
pub use primitives::{Card, Inset, Section, Spacer};
pub use progress::ProgressBar;
pub use scroll::ScrollView;
pub use select::Select;
pub use spinner::Spinner;
pub use state::{EmptyState, ErrorState, LoadingState};
pub use table::Table;
pub use tabs::Tabs;
pub use text::{RichText, Text, TextInput};
pub use toggle::Toggle;
pub use typography::{Accent, Body, Muted, Title};

use crate::event::{Event, KeyEvent};
use crate::focus::FocusManager;
use ratatui::layout::Rect;
use ratatui::Frame;

pub(crate) fn register_and_check_focus(widget_id: Option<&str>) -> bool {
    if let Some(id) = widget_id {
        FocusManager::with_current_mut(|fm| fm.register(id.to_string()));
        FocusManager::with_current(|fm| fm.is_focused(id))
    } else {
        false
    }
}

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

    /// Widget ID for focus management. Override in focusable widgets.
    fn widget_id(&self) -> Option<&str> {
        None
    }

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

    fn widget_id(&self) -> Option<&str> {
        (*self).widget_id()
    }

    fn is_focusable(&self) -> bool {
        (*self).is_focusable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal test widget with all defaults
    struct EmptyView;
    impl View for EmptyView {}

    // A widget that overrides some defaults
    struct CustomView;
    impl View for CustomView {
        fn constraints(&self) -> ultty_protocol::Constraints {
            ultty_protocol::Constraints {
                width: ultty_protocol::Dimension::Percent(50.0),
                ..Default::default()
            }
        }

        fn is_focusable(&self) -> bool {
            true
        }

        fn children_count(&self) -> usize {
            3
        }
    }

    // --- View trait defaults ---

    #[test]
    fn default_constraints() {
        let v = EmptyView;
        let c = v.constraints();
        assert_eq!(c, ultty_protocol::Constraints::default());
    }

    #[test]
    fn default_style() {
        let v = EmptyView;
        let s = v.style();
        assert_eq!(s, ultty_protocol::Style::default());
    }

    #[test]
    fn default_content_is_none() {
        let v = EmptyView;
        assert_eq!(v.content(), ultty_protocol::Content::None);
    }

    #[test]
    fn default_children_count_is_zero() {
        let v = EmptyView;
        assert_eq!(v.children_count(), 0);
    }

    #[test]
    fn default_is_not_focusable() {
        let v = EmptyView;
        assert!(!v.is_focusable());
    }

    #[test]
    fn default_on_key_returns_none() {
        let mut v = EmptyView;
        let key = KeyEvent {
            key: crate::event::Key::Enter,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        };
        assert!(v.on_key(&key).is_none());
    }

    #[test]
    fn default_on_event_returns_false() {
        let mut v = EmptyView;
        let event = Event::FocusGained;
        assert!(!v.on_event(&event));
    }

    #[test]
    fn default_render_child_native_returns_none() {
        let v = EmptyView;
        let mut next_id = 1u32;
        let parent = ultty_protocol::RegionId::new(0);
        let mut send = |_msg: &ultty_protocol::AppMessage| -> anyhow::Result<()> { Ok(()) };
        let result = v.render_child_native(0, &mut send, parent, &mut next_id);
        assert!(result.unwrap().is_none());
    }

    // --- Custom view overrides ---

    #[test]
    fn custom_constraints() {
        let v = CustomView;
        let c = v.constraints();
        assert_eq!(c.width, ultty_protocol::Dimension::Percent(50.0));
    }

    #[test]
    fn custom_is_focusable() {
        let v = CustomView;
        assert!(v.is_focusable());
    }

    #[test]
    fn custom_children_count() {
        let v = CustomView;
        assert_eq!(v.children_count(), 3);
    }

    // --- View impl for &T ---

    #[test]
    fn ref_view_delegates_constraints() {
        let v = CustomView;
        let r: &CustomView = &v;
        let c = <&CustomView as View>::constraints(&r);
        assert_eq!(c.width, ultty_protocol::Dimension::Percent(50.0));
    }

    #[test]
    fn ref_view_delegates_style() {
        let v = EmptyView;
        let r: &EmptyView = &v;
        let s = <&EmptyView as View>::style(&r);
        assert_eq!(s, ultty_protocol::Style::default());
    }

    #[test]
    fn ref_view_delegates_content() {
        let v = EmptyView;
        let r: &EmptyView = &v;
        assert_eq!(
            <&EmptyView as View>::content(&r),
            ultty_protocol::Content::None
        );
    }

    #[test]
    fn ref_view_delegates_children_count() {
        let v = CustomView;
        let r: &CustomView = &v;
        assert_eq!(<&CustomView as View>::children_count(&r), 3);
    }

    #[test]
    fn ref_view_delegates_is_focusable() {
        let v = CustomView;
        let r: &CustomView = &v;
        assert!(<&CustomView as View>::is_focusable(&r));
    }

    #[test]
    fn ref_view_empty_not_focusable() {
        let v = EmptyView;
        let r: &EmptyView = &v;
        assert!(!<&EmptyView as View>::is_focusable(&r));
    }

    // --- FocusAction ---

    #[test]
    fn focus_action_consumed() {
        let a = FocusAction::Consumed;
        assert!(matches!(a, FocusAction::Consumed));
    }

    #[test]
    fn focus_action_next() {
        let a = FocusAction::Next;
        assert!(matches!(a, FocusAction::Next));
    }

    #[test]
    fn focus_action_previous() {
        let a = FocusAction::Previous;
        assert!(matches!(a, FocusAction::Previous));
    }

    // --- Text widget through View ---

    #[test]
    fn text_content_via_view() {
        let t = Text::new("hello");
        let content = <Text as View>::content(&t);
        match content {
            ultty_protocol::Content::Text(tc) => assert_eq!(tc.text, "hello"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn text_not_focusable() {
        let t = Text::new("test");
        assert!(!t.is_focusable());
    }

    #[test]
    fn text_has_intrinsic_height_constraints() {
        let t = Text::new("test");
        let c = <Text as View>::constraints(&t);
        assert_eq!(c.width, ultty_protocol::Dimension::Percent(100.0));
        assert_eq!(c.height, ultty_protocol::Dimension::Cells(1.0));
        assert_eq!(c.flex_shrink, 1.0);
    }

    #[test]
    fn text_default_style() {
        let t = Text::new("test");
        let s = <Text as View>::style(&t);
        assert_eq!(s, ultty_protocol::Style::default());
    }

    #[test]
    fn text_no_children() {
        let t = Text::new("test");
        assert_eq!(<Text as View>::children_count(&t), 0);
    }

    #[test]
    fn text_content_accessor() {
        let t = Text::new("hello world");
        assert_eq!(t.content(), "hello world");
    }

    #[test]
    fn text_from_string() {
        let t = Text::new(String::from("owned"));
        assert_eq!(t.content(), "owned");
    }

    // --- RichText through View ---

    #[test]
    fn rich_text_empty() {
        let rt = RichText::new();
        assert!(rt.spans().is_empty());
    }

    #[test]
    fn rich_text_builder() {
        let rt = RichText::new()
            .text("hello ")
            .span("world", ultty_protocol::Style::new().bold());
        assert_eq!(rt.spans().len(), 2);
        assert_eq!(rt.spans()[0].0, "hello ");
        assert_eq!(rt.spans()[1].0, "world");
        assert!(rt.spans()[1].1.bold);
    }

    #[test]
    fn rich_text_content_via_view() {
        let rt = RichText::new().text("test");
        let content = <RichText as View>::content(&rt);
        assert!(matches!(content, ultty_protocol::Content::RichText(_)));
    }

    #[test]
    fn rich_text_not_focusable() {
        let rt = RichText::new();
        assert!(!rt.is_focusable());
    }

    // --- TextInput through View ---

    #[test]
    fn text_input_default() {
        let input = TextInput::new();
        assert_eq!(input.current_value(), "");
        assert_eq!(input.cursor_position(), 0);
        assert!(input.selection().is_none());
    }

    #[test]
    fn text_input_with_value() {
        let input = TextInput::new().value("hello");
        assert_eq!(input.current_value(), "hello");
        assert_eq!(input.cursor_position(), 5); // cursor at end
    }

    #[test]
    fn text_input_with_placeholder() {
        let input = TextInput::new().placeholder("Enter text...");
        // Placeholder does not affect value
        assert_eq!(input.current_value(), "");
    }

    #[test]
    fn text_input_multiline() {
        let input = TextInput::new().multiline();
        assert!(input.is_focusable());
    }

    #[test]
    fn text_input_is_focusable() {
        let input = TextInput::new();
        assert!(input.is_focusable());
    }

    #[test]
    fn text_input_content_is_widget() {
        let input = TextInput::new().value("test");
        let content = <TextInput as View>::content(&input);
        assert!(matches!(content, ultty_protocol::Content::Widget(_)));
    }
}
