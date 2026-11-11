//! User overrides (ADR 0023) applied through the view walks: what each kind
//! of override does to a real widget tree in both modes, and to focus.

use std::rc::Rc;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use ulttui::focus::FocusManager;
use ulttui::overrides::{with_overrides, Overrides};
use ulttui::widgets::{
    render_view_ansi, render_view_messages, Button, Collapsible, Column, Text, View,
};
use ultty_protocol::{AppMessage, Content, Display, WidgetContent};

fn overrides(source: &str) -> Rc<Overrides> {
    Rc::new(Overrides::parse(source).expect("test overrides parse"))
}

fn ansi(view: &dyn View, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| render_view_ansi(view, frame, frame.area()))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn buttons() -> Column {
    Column::new()
        .child(Button::new("One").id("one"))
        .child(Button::new("Two").id("two"))
        .child(Button::new("Three").id("three"))
}

#[test]
fn a_hidden_widget_takes_no_room_and_no_focus() {
    let view = buttons();
    let hide_two = overrides("[widget.two]\nhidden = true");

    let (buffer, native, focusable) = with_overrides(hide_two, || {
        let mut focus = FocusManager::new();
        focus.register_view_tree(&view);
        (
            ansi(&view, 20, 3),
            render_view_messages(&view).unwrap(),
            focus.focusable_ids().to_vec(),
        )
    });

    assert_eq!(rows(&buffer), ["[ One ]", "[ Three ]", ""]);
    assert_eq!(focusable, ["one", "three"]);
    let displays: Vec<Display> = native
        .iter()
        .filter_map(|m| match m {
            AppMessage::CreateRegion { constraints, .. } => Some(constraints.display),
            _ => None,
        })
        .collect();
    assert_eq!(displays[2], Display::None, "{displays:?}");
    let labels: Vec<&str> = native
        .iter()
        .filter_map(|m| match m {
            AppMessage::SetContent {
                content: Content::Widget(WidgetContent::Button(b)),
                ..
            } => Some(b.label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, ["One", "Three"], "a hidden widget sends no content");
    let hidden_regions: Vec<u32> = native
        .iter()
        .filter_map(|m| match m {
            AppMessage::SetSemantics { id, semantics } if semantics.hidden => Some(id.0),
            _ => None,
        })
        .collect();
    assert_eq!(
        hidden_regions,
        [3],
        "the hidden region is out of the accessibility tree"
    );
}

#[test]
fn colour_overrides_leave_a_focused_widget_its_focus_colours() {
    use ulttui::theme::{to_ratatui_color, Theme};
    Theme::set_current(Theme::dark());
    let theme = Theme::current();
    let view = buttons();
    let recolour = overrides(
        "[widget.one]\nfg = \"red\"\nbg = \"blue\"\n[widget.two]\nfg = \"red\"\nbg = \"blue\"\n",
    );

    let mut focus = FocusManager::new();
    focus.register_view_tree(&view);
    focus.focus("one");
    let previous = FocusManager::set_current(focus);
    let (buffer, native) = with_overrides(recolour, || {
        (ansi(&view, 20, 3), render_view_messages(&view).unwrap())
    });
    FocusManager::set_current(previous);

    // ANSI: the focused button keeps the theme's focus colours, the other
    // takes the override.
    let focused = &buffer[(0, 0)];
    assert_eq!(focused.fg, to_ratatui_color(theme.button.focused_fg));
    assert_eq!(focused.bg, to_ratatui_color(theme.button.focused_bg));
    let other = &buffer[(0, 1)];
    assert_eq!(
        (other.fg, other.bg),
        (ratatui::style::Color::Red, ratatui::style::Color::Blue)
    );

    // Native: the same, in the regions' styles.
    let styles: Vec<_> = native
        .iter()
        .filter_map(|m| match m {
            AppMessage::CreateRegion { style, .. } => Some((style.fg, style.bg)),
            _ => None,
        })
        .collect();
    assert_eq!(
        styles[1],
        (Some(theme.button.focused_fg), Some(theme.button.focused_bg))
    );
    use ultty_protocol::{Color, NamedColor};
    assert_eq!(
        styles[2],
        (
            Some(Color::Named(NamedColor::Red)),
            Some(Color::Named(NamedColor::Blue))
        )
    );
}

#[test]
fn without_active_overrides_nothing_changes() {
    let view = buttons();
    let before = (ansi(&view, 20, 3), render_view_messages(&view).unwrap());
    let empty = with_overrides(Rc::new(Overrides::default()), || {
        (ansi(&view, 20, 3), render_view_messages(&view).unwrap())
    });
    assert_eq!(before, empty);
    assert_eq!(rows(&before.0), ["[ One ]", "[ Two ]", "[ Three ]"]);
}

#[test]
fn text_set_on_a_container_does_not_reach_its_children() {
    let view = Collapsible::new(Text::new("Header"), Text::new("Body"))
        .id("section")
        .expanded(true);
    let rename = overrides("[widget.section]\ntext = \"Renamed\"");

    let (buffer, native) = with_overrides(rename, || {
        (ansi(&view, 20, 2), render_view_messages(&view).unwrap())
    });

    let shown = rows(&buffer).join("\n");
    assert!(
        shown.contains("Header") && shown.contains("Body"),
        "{shown}"
    );
    assert!(!format!("{native:?}").contains("Renamed"));
}

#[test]
fn replacement_text_resizes_the_widget() {
    let view = Column::new().child(Button::new("OK").id("ok"));
    let longer = overrides("[widget.ok]\ntext = \"Accept\"");
    let (buffer, native) = with_overrides(longer, || {
        (ansi(&view, 20, 1), render_view_messages(&view).unwrap())
    });
    assert_eq!(rows(&buffer), ["[ Accept ]"]);
    let widths: Vec<_> = native
        .iter()
        .filter_map(|m| match m {
            AppMessage::CreateRegion { constraints, .. } => Some(constraints.width),
            _ => None,
        })
        .collect();
    assert_eq!(widths[1], ultty_protocol::Dimension::Cells(10.0));
}
