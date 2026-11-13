//! Same-fixture dual-mode parity harness.
//!
//! Each test builds one `View`, renders it through the ANSI path (into a
//! ratatui `Buffer`, styles preserved) and through the native path (into the
//! emitted protocol message tree), and asserts on both. Focus registration
//! runs the same backend-independent tree walk the runner uses.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color as RColor, Modifier};
use ratatui::Terminal;

use ulttui::focus::FocusManager;
use ulttui::theme::{to_ratatui_color, Theme};
use ulttui::widgets::{
    render_view_messages, Button, Column, LoadingState, ScrollView, Section, Table, Text,
    TextInput, View,
};
use ultty_protocol::{AppMessage, Constraints, Content, Dimension, RegionId, Role, WidgetContent};

struct Rendered {
    ansi: Buffer,
    native: Vec<AppMessage>,
    focusable_ids: Vec<String>,
}

/// Render `view` in both modes with `focused` (if any) as the focused widget.
fn render_both(view: &dyn View, width: u16, height: u16, focused: Option<&str>) -> Rendered {
    Theme::set_current(Theme::dark());
    let mut focus = FocusManager::new();
    focus.register_view_tree(view);
    if let Some(id) = focused {
        focus.focus(id);
    }
    let focusable_ids = focus.focusable_ids().to_vec();

    let previous = FocusManager::set_current(focus);
    let native = render_view_messages(view).expect("native render");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| view.render_ansi(frame, frame.area()))
        .expect("ansi render");
    let ansi = terminal.backend().buffer().clone();
    FocusManager::set_current(previous);

    Rendered {
        ansi,
        native,
        focusable_ids,
    }
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn created_constraints(messages: &[AppMessage], parent: RegionId) -> Vec<Constraints> {
    messages
        .iter()
        .filter_map(|m| match m {
            AppMessage::CreateRegion {
                parent: p,
                constraints,
                ..
            } if *p == parent => Some(constraints.clone()),
            _ => None,
        })
        .collect()
}

fn scroll_messages(messages: &[AppMessage]) -> Vec<(RegionId, f32, f32)> {
    messages
        .iter()
        .filter_map(|m| match m {
            AppMessage::Scroll { id, x, y, .. } => Some((*id, *x, *y)),
            _ => None,
        })
        .collect()
}

fn twenty_line_scrollview() -> ScrollView {
    let lines: Vec<String> = (0..20).map(|i| format!("Line {i}")).collect();
    let mut scroll = ScrollView::new().child(Text::new(lines.join("\n")));
    scroll.set_content_height(20.0);
    scroll.set_viewport_height(5.0);
    scroll.scroll_to(10.0);
    scroll
}

#[test]
fn scrollview_offset_scrolls_ansi_and_emits_native_scroll() {
    let rendered = render_both(&twenty_line_scrollview(), 20, 7, None);

    // ANSI: rows inside the border show the window starting at line 10.
    assert!(row_text(&rendered.ansi, 1).starts_with("│Line 10"));
    assert!(row_text(&rendered.ansi, 5).starts_with("│Line 14"));

    // Native: the scroll view region (id 1) carries the same offset.
    assert_eq!(
        scroll_messages(&rendered.native),
        vec![(RegionId::new(1), 0.0, 10.0)]
    );
}

#[test]
fn section_wrapping_scrollview_still_emits_native_scroll() {
    let section = Section::new().title("Log").child(twenty_line_scrollview());
    let rendered = render_both(&section, 20, 8, None);

    let scrolls = scroll_messages(&rendered.native);
    assert_eq!(scrolls.len(), 1, "Section must not drop the child's offset");
    assert_eq!((scrolls[0].1, scrolls[0].2), (0.0, 10.0));
    let scroll_region = scrolls[0].0;
    assert!(rendered.native.iter().any(|m| matches!(
        m,
        AppMessage::CreateRegion { id, parent, .. } if *id == scroll_region && *parent == RegionId::new(1)
    )));

    // ANSI: title row, then the scrolled window.
    assert!(row_text(&rendered.ansi, 0).contains("Log"));
    assert!(row_text(&rendered.ansi, 2).starts_with("│Line 10"));
}

#[test]
fn table_column_widths_apply_in_both_modes() {
    let theme = Theme::dark();
    let table = Table::new()
        .headers(vec!["AA", "BB", "CC"])
        .column_widths(vec![4.0, 3.0])
        .rows(vec![vec!["x", "y", "z"]]);
    let rendered = render_both(&table, 24, 5, None);

    // Native: header cells are children of the header row (region 2).
    let header_cells = created_constraints(&rendered.native, RegionId::new(2));
    assert_eq!(header_cells.len(), 3);
    assert_eq!(header_cells[0].width, Dimension::Cells(4.0));
    assert_eq!(header_cells[1].width, Dimension::Cells(3.0));
    assert_eq!(header_cells[2].flex_grow, 1.0);
    let header_style = rendered
        .native
        .iter()
        .find_map(|m| match m {
            AppMessage::CreateRegion { id, style, .. } if *id == RegionId::new(2) => {
                Some(style.clone())
            }
            _ => None,
        })
        .expect("header row");
    assert_eq!(header_style.fg, Some(theme.primary));

    // ANSI: columns start at x = 1, 6, 10 (border, 4 + spacing, 3 + spacing),
    // the header is styled with theme.primary and the data row follows it.
    assert_eq!(rendered.ansi[(1, 1)].symbol(), "A");
    assert_eq!(rendered.ansi[(6, 1)].symbol(), "B");
    assert_eq!(rendered.ansi[(10, 1)].symbol(), "C");
    assert_eq!(rendered.ansi[(1, 1)].fg, to_ratatui_color(theme.primary));
    assert!(rendered.ansi[(1, 1)].modifier.contains(Modifier::BOLD));
    assert_eq!(rendered.ansi[(1, 2)].symbol(), "x");
    assert_eq!(rendered.ansi[(10, 2)].symbol(), "z");
}

#[test]
fn focus_registration_and_focused_button_style_match_in_both_modes() {
    let theme = Theme::dark();
    let form = Column::new()
        .child(TextInput::new().id("name"))
        .child(Button::new("Save").id("save"))
        .child(TextInput::new().id("email"));
    let rendered = render_both(&form, 20, 3, Some("save"));

    assert_eq!(rendered.focusable_ids, ["name", "save", "email"]);

    // Native: the button region (third region: column=1, name=2, save=3)
    // uses the focused colors.
    let button_style = rendered
        .native
        .iter()
        .find_map(|m| match m {
            AppMessage::CreateRegion { id, style, .. } if *id == RegionId::new(3) => {
                Some(style.clone())
            }
            _ => None,
        })
        .expect("button region");
    assert_eq!(button_style.fg, Some(theme.button.focused_fg));
    assert_eq!(button_style.bg, Some(theme.button.focused_bg));

    // ANSI: the button's cells carry the same focused colors.
    let cell = &rendered.ansi[(0, 1)];
    assert_eq!(cell.symbol(), "[");
    assert_eq!(cell.fg, to_ratatui_color(theme.button.focused_fg));
    assert_eq!(cell.bg, to_ratatui_color(theme.button.focused_bg));
    assert!(cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn loading_state_pinned_frame_matches_in_both_modes() {
    let theme = Theme::dark();
    let loading = LoadingState::new("Loading").frame(3);
    let rendered = render_both(&loading, 20, 1, None);

    // Native: a spinner widget with the label; the terminal animates it.
    assert!(matches!(
        &rendered.native[..],
        [AppMessage::CreateRegion { style, .. }, AppMessage::SetContent {
            content: Content::Widget(WidgetContent::Spinner(spinner)),
            ..
        }] if spinner.label.as_deref() == Some("Loading") && style.fg == Some(theme.primary)
    ));

    // ANSI: frame 3 of the braille cycle plus the label, in theme.primary.
    assert_eq!(row_text(&rendered.ansi, 0), "⠸ Loading");
    assert_eq!(rendered.ansi[(0, 0)].fg, to_ratatui_color(theme.primary));
    assert_ne!(rendered.ansi[(0, 0)].fg, RColor::Reset);
}

#[test]
fn a_focused_input_sends_its_typed_text_and_a_name_natively() {
    let field = TextInput::new().id("msg").value("hello").label("Message");
    let rendered = render_both(&field, 30, 3, Some("msg"));

    // The left focus border leaves the first row available for text.
    assert!(row_text(&rendered.ansi, 0).contains("[N] hello"));

    // Native: the content is the typed text alone (focus and mode are the
    // region's style and the semantics description); the semantics carry
    // the name, which the terminal would not derive, so they are sent.
    let Some(AppMessage::SetContent {
        content: Content::Widget(WidgetContent::Input(input)),
        ..
    }) = rendered
        .native
        .iter()
        .find(|m| matches!(m, AppMessage::SetContent { .. }))
    else {
        panic!("an input region: {:?}", rendered.native);
    };
    assert_eq!(input.value, "hello");
    let semantics = rendered
        .native
        .iter()
        .find_map(|m| match m {
            AppMessage::SetSemantics { semantics, .. } => Some(semantics),
            _ => None,
        })
        .expect("semantics sent");
    assert_eq!(semantics.role, Role::Input);
    assert_eq!(semantics.label.as_deref(), Some("Message"));
    assert_eq!(semantics.value_text.as_deref(), Some("hello"));
}
