//! Snapshot tests for widget rendering.
//!
//! Uses insta to capture widget output and detect regressions.
//! Run `cargo insta review` to review and approve snapshot changes.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ulttui::widgets::{
    Badge, Button, Column, Divider, List, ProgressBar, Row, Spinner, Stack, Text, View,
};

/// Render a widget and return its buffer as a string for snapshot comparison.
fn render_to_string(widget: &impl View, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            widget.render_ansi(frame, frame.area());
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    buffer_to_string(buffer)
}

/// Convert a ratatui buffer to a human-readable string.
fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
    let mut lines = Vec::new();

    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            line.push_str(cell.symbol());
        }
        // Trim trailing spaces but keep the line
        lines.push(line.trim_end().to_string());
    }

    // Remove trailing empty lines
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    lines.join("\n")
}

#[test]
fn snapshot_text_simple() {
    let text = Text::new("Hello, World!");
    let output = render_to_string(&text, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_text_multiline() {
    let text = Text::new("Line 1\nLine 2\nLine 3");
    let output = render_to_string(&text, 20, 5);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_text_unicode() {
    let text = Text::new("Hello 世界 🌍 emoji");
    let output = render_to_string(&text, 30, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_button() {
    let button = Button::new("Click Me");
    let output = render_to_string(&button, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_button_short() {
    let button = Button::new("OK");
    let output = render_to_string(&button, 10, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_column_layout() {
    let col = Column::new()
        .child(Text::new("Header"))
        .child(Text::new("Content"))
        .child(Text::new("Footer"));
    let output = render_to_string(&col, 20, 6);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_row_layout() {
    let row = Row::new()
        .child(Text::new("Left"))
        .child(Text::new("Center"))
        .child(Text::new("Right"));
    let output = render_to_string(&row, 30, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_nested_layout() {
    let layout = Column::new()
        .child(Text::new("Title"))
        .child(
            Row::new()
                .child(Text::new("Sidebar"))
                .child(Text::new("Main Content")),
        )
        .child(Text::new("Status"));
    let output = render_to_string(&layout, 40, 6);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_list_basic() {
    let list = List::new().items(vec!["Apple", "Banana", "Cherry", "Date"]);
    let output = render_to_string(&list, 30, 8);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_list_empty() {
    let list: List<String> = List::new().items(vec![]);
    let output = render_to_string(&list, 30, 5);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_stack_overlay() {
    let stack = Stack::new()
        .child(Text::new("Background text here"))
        .child(Text::new("Overlay"));
    let output = render_to_string(&stack, 25, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_complex_ui() {
    // Simulate a typical TUI application layout
    let ui = Column::new()
        .child(Text::new("=== Application Title ==="))
        .child(
            Row::new()
                .child(List::new().items(vec!["File", "Edit", "View", "Help"]))
                .child(
                    Column::new()
                        .child(Text::new("Welcome!"))
                        .child(Text::new("Select an option from the menu.")),
                ),
        )
        .child(Text::new("Ready | Line 1, Col 1"));

    let output = render_to_string(&ui, 60, 12);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_truncation() {
    let text = Text::new(
        "This is a very long string that should be truncated when rendered in a small area",
    );
    let output = render_to_string(&text, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_special_chars() {
    let text = Text::new("Tabs:\t\tSpaces   End");
    let output = render_to_string(&text, 30, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_progress_bar_50_percent() {
    let bar = ProgressBar::new(50.0);
    let output = render_to_string(&bar, 40, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_progress_bar_with_label() {
    let bar = ProgressBar::new(75.0).label("Loading");
    let output = render_to_string(&bar, 40, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_progress_bar_native_content() {
    let bar = ProgressBar::new(42.0);
    let content = bar.content();
    insta::assert_snapshot!(format!("{content:?}"));
}

#[test]
fn snapshot_spinner_frame_0() {
    let spinner = Spinner::new();
    let output = render_to_string(&spinner, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_spinner_with_label() {
    let spinner = Spinner::new().label("Loading...");
    let output = render_to_string(&spinner, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_spinner_native_content() {
    let spinner = Spinner::new().label("Working");
    let content = spinner.content();
    insta::assert_snapshot!(format!("{content:?}"));
}

#[test]
fn snapshot_divider_horizontal() {
    let divider = Divider::horizontal();
    let output = render_to_string(&divider, 30, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_divider_custom_char() {
    let divider = Divider::horizontal().char('=');
    let output = render_to_string(&divider, 20, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_badge_primary() {
    let badge = Badge::new("Info");
    let output = render_to_string(&badge, 15, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_badge_success() {
    let badge = Badge::success("OK");
    let output = render_to_string(&badge, 15, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_badge_native_content() {
    let badge = Badge::success("Done");
    let content = badge.content();
    insta::assert_snapshot!(format!("{content:?}"));
}
