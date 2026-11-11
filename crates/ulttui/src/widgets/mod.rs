//! Built-in widgets.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

mod badge;
mod binary_toggle;
mod button;
mod checkbox;
mod collapsible;
mod container;
mod diff;
mod divider;
mod dock;
mod image;
mod input_history;
mod list;
mod markdown;
mod modal;
mod primitives;
mod progress;
mod scroll;
mod scroll_search;
mod select;
mod semantic;
mod spinner;
mod state;
mod table;
mod tabs;
mod text;
mod toggle;
mod tree;
mod typography;

pub use badge::{Badge, BadgeVariant};
pub use button::Button;
pub use checkbox::Checkbox;
pub use collapsible::Collapsible;
pub use container::{Column, Row, Stack};
pub use diff::{parse_unified_diff, DiffKind, DiffLine, DiffMode, DiffView};
pub use divider::Divider;
pub use dock::DockLayout;
pub use image::Image;
pub use list::List;
pub use markdown::{wrapped_line_count, MarkdownView};
pub use modal::Modal;
pub use primitives::{Card, Inset, Section, Spacer};
pub use progress::ProgressBar;
pub use scroll::ScrollView;
pub use select::Select;
pub use semantic::Semantic;
pub(crate) use semantic::{send_semantics, Accessible};
pub use spinner::Spinner;
pub use state::{EmptyState, ErrorState, LoadingState};
pub use table::Table;
pub use tabs::Tabs;
pub use text::{RichText, Text, TextInput};
pub use toggle::Toggle;
pub use tree::{Tree, TreeNode};
pub use typography::{Accent, Body, Muted, Title};

use crate::event::{Event, KeyEvent};
use crate::focus::FocusManager;
use ratatui::layout::Rect;
use ratatui::Frame;
use ultty_protocol::{AppMessage, Constraints, Content, RegionId};

/// Expand tabs to eight-column stops, starting each line at column zero.
fn expand_tabs(text: &str) -> Cow<'_, str> {
    if !text.contains('\t') {
        return Cow::Borrowed(text);
    }
    expand_tabs_from_column(text, &mut 0)
}

fn expand_tabs_from_column<'a>(text: &'a str, column: &mut usize) -> Cow<'a, str> {
    let mut expanded = text
        .contains('\t')
        .then(|| String::with_capacity(text.len()));
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = 8 - *column % 8;
            if let Some(output) = &mut expanded {
                output.extend(std::iter::repeat_n(' ', spaces));
            }
            *column += spaces;
        } else {
            if let Some(output) = &mut expanded {
                output.push(ch);
            }
            if ch == '\n' {
                *column = 0;
            } else {
                *column += ch.width().unwrap_or(0);
            }
        }
    }
    expanded.map_or(Cow::Borrowed(text), Cow::Owned)
}

/// Read-only focus query for widgets that render differently when focused.
///
/// Registration is not done here: the runner registers every focusable
/// widget once per frame via [`FocusManager::register_view_tree`], so
/// rendering code stays free of side effects.
pub(crate) fn is_widget_focused(widget_id: Option<&str>) -> bool {
    match widget_id {
        Some(id) => FocusManager::with_current(|fm| fm.is_focused(id)),
        None => false,
    }
}

/// Render a view tree into native protocol messages, rooted at `RegionId::ROOT`.
///
/// This is the same walk the native backend performs for child views. It is
/// exposed so tests and the mock backend can inspect the exact message tree a
/// view produces without a terminal connection.
pub fn render_view_messages(view: &dyn View) -> crate::Result<Vec<AppMessage>> {
    let mut messages = Vec::new();
    let mut next_id = 1u32;
    let mut sink = |msg: &AppMessage| -> crate::Result<()> {
        messages.push(msg.clone());
        Ok(())
    };
    render_view_to_native(view, &mut sink, RegionId::ROOT, &mut next_id, None)?;
    Ok(messages)
}

pub(crate) fn render_view_to_native(
    child: &dyn View,
    send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
    parent: RegionId,
    next_id: &mut u32,
    constraint_overrides: Option<Constraints>,
) -> crate::Result<RegionId> {
    let region_id = RegionId::new(*next_id);
    *next_id += 1;

    // The user's override for this widget (ADR 0023): `None` outside an
    // override scope. Constraints, style, content and semantics are read with
    // the widget's replacement text in scope; its children (rendered below,
    // outside the scope) see their own.
    let user = crate::overrides::for_view(child).flatten();
    let hidden = user.as_ref().is_some_and(|u| u.hidden);
    let text = user.as_ref().and_then(|u| u.text.clone());
    let (constraints, style, shown) = crate::overrides::with_text(text, || {
        let mut constraints = constraint_overrides.unwrap_or_else(|| child.constraints());
        let mut style = child.style();
        if hidden {
            constraints.display = ultty_protocol::Display::None;
            return (constraints, style, None);
        }
        let mut content = child.content();
        if let Some(user) = &user {
            style = user.apply_style(style);
            content = user.apply_content(content);
        }
        (constraints, style, Some((content, child.semantics())))
    });

    send_message(&AppMessage::CreateRegion {
        id: region_id,
        parent,
        insert_index: -1,
        constraints,
        style,
    })?;
    if let Some(key) = child.native_key() {
        crate::backend::native_diff::record_native_key(region_id, key);
    }
    // A hidden widget is an empty `display: none` region.
    let Some((content, semantics)) = shown else {
        return Ok(region_id);
    };

    send_semantics(send_message, region_id, semantics, &content)?;
    if content != Content::None {
        send_message(&AppMessage::SetContent {
            id: region_id,
            content,
        })?;
    }

    if let Some((sx, sy)) = child.scroll_offset() {
        send_message(&AppMessage::Scroll {
            id: region_id,
            x: sx,
            y: sy,
            animate: false,
        })?;
    }

    for i in 0..child.children_count() {
        child.render_child_native(i, send_message, region_id, next_id)?;
    }

    Ok(region_id)
}

/// Render `view` in ANSI mode with the user's overrides for it (ADR 0023).
///
/// The ANSI counterpart of the native walk: the backend draws the root
/// through it and every container draws each child through it, so an
/// override reaches any widget with an id. A hidden widget draws nothing;
/// an overridden colour or bold is painted over the widget's area after it
/// draws; replacement text is in scope while it draws.
pub fn render_view_ansi(view: &dyn View, frame: &mut Frame, area: Rect) {
    let Some(user) = crate::overrides::for_view(view) else {
        view.render_ansi(frame, area);
        return;
    };
    let user = user.unwrap_or_default();
    if user.hidden {
        return;
    }
    crate::overrides::with_text(user.text.clone(), || view.render_ansi(frame, area));
    if let Some(style) = user.ratatui_style() {
        let buffer = frame.buffer_mut();
        let area = area.intersection(buffer.area);
        buffer.set_style(area, style);
    }
}

/// Padding that keeps a bordered widget's children off its border. A native
/// border takes no room in the layout (spec section 10.6), so without it
/// children sit on the border line; one cell on each side matches the inner
/// area of the ANSI renderer's bordered `Block`.
pub(crate) const BORDER_PADDING: ultty_protocol::Edges<ultty_protocol::Dimension> =
    ultty_protocol::Edges {
        top: ultty_protocol::Dimension::Cells(1.0),
        right: ultty_protocol::Dimension::Cells(1.0),
        bottom: ultty_protocol::Dimension::Cells(1.0),
        left: ultty_protocol::Dimension::Cells(1.0),
    };

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

    /// What this view is, for assistive technology (ADR 0021 §4).
    ///
    /// The native backend sends it to the terminal when it adds to what
    /// the terminal derives from [`View::content`], and again when it
    /// changes. The default, role `Auto` and nothing else, leaves it all to
    /// the terminal. ANSI mode ignores it.
    fn semantics(&self) -> ultty_protocol::Semantics {
        ultty_protocol::Semantics::default()
    }

    fn children_count(&self) -> usize {
        0
    }

    fn render_child_native(
        &self,
        _index: usize,
        _send_message: &mut dyn FnMut(&ultty_protocol::AppMessage) -> crate::Result<()>,
        _parent: ultty_protocol::RegionId,
        _next_id: &mut u32,
    ) -> crate::Result<Option<ultty_protocol::RegionId>> {
        Ok(None)
    }

    fn render_ansi(&self, _frame: &mut Frame, _area: Rect) {}

    /// Visit every child view this view will render this frame.
    ///
    /// This is the structural walk used by mode-independent passes such as
    /// focus registration, so it must be implemented by every container that
    /// owns child views and must visit them in render order. Children that a
    /// widget synthesizes on the fly (table cells, tab labels) are not views
    /// and need not be visited.
    fn for_each_child(&self, _visit: &mut dyn FnMut(&dyn View)) {}

    /// Widget ID for focus management. Override in focusable widgets.
    ///
    /// IDs should be stable across renders. Immediate-mode view values are
    /// rebuilt every frame, so app code must not rely on Rust object identity
    /// for focus or interaction continuity.
    fn widget_id(&self) -> Option<&str> {
        None
    }

    /// Stable semantic key for native rendering identity.
    ///
    /// This currently defaults to `widget_id()` and is reserved for native
    /// backends that need to map immediate-mode output onto retained protocol
    /// regions. If overridden, keep the key stable for the same logical widget
    /// across frames; traversal-order identity is not stable across sibling
    /// insertions or reorders.
    fn native_key(&self) -> Option<&str> {
        self.widget_id()
    }

    fn is_focusable(&self) -> bool {
        false
    }

    /// Apply a key to app-owned widget state.
    ///
    /// The Ulttui runner does not dispatch this through the rendered view tree:
    /// views are output generated from `Application::view()`. Use this helper
    /// from application state or specialized screens that own a widget value.
    /// App-level event handling receives the focused widget ID on `KeyEvent`.
    fn on_key(&mut self, _key: &KeyEvent) -> Option<FocusAction> {
        None
    }

    /// Apply an event to app-owned widget state.
    ///
    /// Like `on_key`, this is a helper for owners of persistent widget values,
    /// not an automatic runner dispatch hook.
    fn on_event(&mut self, _event: &Event) -> bool {
        false
    }

    /// Return scroll offset (x, y) if this view is a scroll container with non-zero offset.
    fn scroll_offset(&self) -> Option<(f32, f32)> {
        None
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

    fn semantics(&self) -> ultty_protocol::Semantics {
        (*self).semantics()
    }

    fn children_count(&self) -> usize {
        (*self).children_count()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&ultty_protocol::AppMessage) -> crate::Result<()>,
        parent: ultty_protocol::RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<ultty_protocol::RegionId>> {
        (*self).render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        (*self).render_ansi(frame, area)
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        (*self).for_each_child(visit)
    }

    fn widget_id(&self) -> Option<&str> {
        (*self).widget_id()
    }

    fn native_key(&self) -> Option<&str> {
        (*self).native_key()
    }

    fn is_focusable(&self) -> bool {
        (*self).is_focusable()
    }

    fn scroll_offset(&self) -> Option<(f32, f32)> {
        (*self).scroll_offset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tabs_no_tabs_borrowed() {
        assert!(matches!(expand_tabs("hello\n世界"), Cow::Borrowed(_)));
    }

    #[test]
    fn test_expand_tabs_stops() {
        for (input, expected) in [
            ("\t", "        "),
            ("ab\tc", "ab      c"),
            ("\t\t", "                "),
            ("12345678\t", "12345678        "),
            ("abcdefg\t", "abcdefg "),
            ("ab\n\tc", "ab\n        c"),
            ("界\tx", "界      x"),
        ] {
            assert_eq!(expand_tabs(input), expected, "{input:?}");
        }
    }

    #[test]
    fn test_rich_text_tabs_across_spans() {
        let text = RichText::new()
            .text("界")
            .span("\tc", ultty_protocol::Style::new().bold())
            .text("\nxy")
            .text("\tz");
        assert_eq!(text.spans()[1].0, "      c");
        assert!(text.spans()[1].1.bold);
        assert_eq!(text.spans()[3].0, "      z");
        let Content::RichText(native) = View::content(&text) else {
            panic!("Expected rich text");
        };
        assert_eq!(native.spans[1].text, "      c");
        assert_eq!(native.spans[3].text, "      z");
    }

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
        let mut send = |_msg: &ultty_protocol::AppMessage| -> crate::Result<()> { Ok(()) };
        let result = v.render_child_native(0, &mut send, parent, &mut next_id);
        assert!(result.unwrap().is_none());
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
        assert_eq!(c.display, ultty_protocol::Display::Flex);
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
