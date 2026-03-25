//! Snapshot tests for widget rendering.
//!
//! Uses insta to capture widget output and detect regressions.
//! Run `cargo insta review` to review and approve snapshot changes.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ulttui::widgets::{
    Badge, Button, Card, Checkbox, Collapsible, Column, DiffView, Divider, DockLayout, Image,
    Inset, List, MarkdownView, Modal, ProgressBar, Row, ScrollView, Section, Select, Spacer,
    Spinner, Stack, Table, Tabs, Text, TextInput, Toggle, Tree, TreeNode, View,
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

// ── Checkbox ──────────────────────────────────────────────────────────

#[test]
fn snapshot_checkbox_unchecked() {
    let cb = Checkbox::new("Accept terms");
    let output = render_to_string(&cb, 25, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_checkbox_checked() {
    let cb = Checkbox::new("Accept terms").checked(true);
    let output = render_to_string(&cb, 25, 1);
    insta::assert_snapshot!(output);
}

// ── Toggle ────────────────────────────────────────────────────────────

#[test]
fn snapshot_toggle_off() {
    let toggle = Toggle::new("Dark mode");
    let output = render_to_string(&toggle, 25, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_toggle_on() {
    let toggle = Toggle::new("Dark mode").on(true);
    let output = render_to_string(&toggle, 25, 1);
    insta::assert_snapshot!(output);
}

// ── Select ────────────────────────────────────────────────────────────

#[test]
fn snapshot_select_collapsed() {
    let select = Select::new(vec![
        "Option A".to_string(),
        "Option B".to_string(),
        "Option C".to_string(),
    ]);
    let output = render_to_string(&select, 30, 1);
    insta::assert_snapshot!(output);
}

// ── TextInput ─────────────────────────────────────────────────────────

#[test]
fn snapshot_text_input_empty() {
    let input = TextInput::new().placeholder("Enter name...");
    let output = render_to_string(&input, 30, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_text_input_with_value() {
    let input = TextInput::new().value("Hello world");
    let output = render_to_string(&input, 30, 1);
    insta::assert_snapshot!(output);
}

// ── Table ─────────────────────────────────────────────────────────────

#[test]
fn snapshot_table_basic() {
    let table = Table::new()
        .headers(vec!["Name", "Age", "City"])
        .column_widths(vec![15.0, 5.0, 10.0])
        .rows(vec![
            vec!["Alice".to_string(), "30".to_string(), "NYC".to_string()],
            vec!["Bob".to_string(), "25".to_string(), "LA".to_string()],
            vec!["Carol".to_string(), "35".to_string(), "Chicago".to_string()],
        ]);
    let output = render_to_string(&table, 40, 6);
    insta::assert_snapshot!(output);
}

// ── Tabs ──────────────────────────────────────────────────────────────

#[test]
fn snapshot_tabs_widget() {
    let tabs = Tabs::new().labels(vec!["Home", "Settings", "About"]);
    let output = render_to_string(&tabs, 40, 1);
    insta::assert_snapshot!(output);
}

// ── Tree ──────────────────────────────────────────────────────────────

#[test]
fn snapshot_tree_expanded() {
    let tree = Tree::new(
        TreeNode::with_children(
            "Root",
            vec![
                TreeNode::new("Child A"),
                TreeNode::with_children("Child B", vec![TreeNode::new("Grandchild")])
                    .expanded(true),
            ],
        )
        .expanded(true),
    );
    let output = render_to_string(&tree, 30, 6);
    insta::assert_snapshot!(output);
}

// ── ScrollView ────────────────────────────────────────────────────────

#[test]
fn snapshot_scrollview_basic() {
    let sv = ScrollView::new().child(
        Column::new()
            .child(Text::new("Line 1"))
            .child(Text::new("Line 2"))
            .child(Text::new("Line 3")),
    );
    let output = render_to_string(&sv, 30, 5);
    insta::assert_snapshot!(output);
}

// ── DockLayout ────────────────────────────────────────────────────────

#[test]
fn snapshot_dock_layout() {
    let dock = DockLayout::new()
        .dock_top(Text::new("Header"))
        .dock_bottom(Text::new("Footer"))
        .center(Text::new("Main content"));
    let output = render_to_string(&dock, 30, 5);
    insta::assert_snapshot!(output);
}

// ── Modal ─────────────────────────────────────────────────────────────

#[test]
fn snapshot_modal_visible() {
    let modal = Modal::new()
        .title("Confirm")
        .body("Are you sure?")
        .buttons(vec!["Yes", "No"])
        .visible(true);
    let output = render_to_string(&modal, 60, 12);
    insta::assert_snapshot!(output);
}

// ── Collapsible ───────────────────────────────────────────────────────

#[test]
fn snapshot_collapsible_expanded() {
    let widget =
        Collapsible::new(Text::new("Details"), Text::new("Hidden content here")).expanded(true);
    let output = render_to_string(&widget, 30, 3);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_collapsible_collapsed() {
    let widget = Collapsible::new(Text::new("Details"), Text::new("Hidden content here"));
    let output = render_to_string(&widget, 30, 1);
    insta::assert_snapshot!(output);
}

// ── DiffView ──────────────────────────────────────────────────────────

#[test]
fn snapshot_diff_view() {
    use ulttui::widgets::parse_unified_diff;
    let diff_text = "--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,3 @@\n unchanged\n-old line\n+new line\n unchanged";
    let lines = parse_unified_diff(diff_text);
    let diff = DiffView::new(lines);
    let output = render_to_string(&diff, 50, 6);
    insta::assert_snapshot!(output);
}

// ── MarkdownView ──────────────────────────────────────────────────────

#[test]
fn snapshot_markdown_basic() {
    let md = MarkdownView::new("# Title\n\nSome **bold** and *italic* text.\n\n- Item 1\n- Item 2");
    let output = render_to_string(&md, 40, 8);
    insta::assert_snapshot!(output);
}

// ── Image ─────────────────────────────────────────────────────────────

#[test]
fn snapshot_image_placeholder() {
    let img = Image::new(vec![0u8; 400], 10, 10);
    let output = render_to_string(&img, 30, 3);
    insta::assert_snapshot!(output);
}

// ── Card / Section / Inset ────────────────────────────────────────────

#[test]
fn snapshot_card() {
    let card = Card::new().child(Text::new("Card content"));
    let output = render_to_string(&card, 30, 5);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_section_with_title() {
    let section = Section::new()
        .title("My Section")
        .child(Text::new("Body text"));
    let output = render_to_string(&section, 30, 4);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_inset() {
    let inset = Inset::new(Text::new("Padded"), 2.0);
    let output = render_to_string(&inset, 20, 5);
    insta::assert_snapshot!(output);
}

// ── Spacer ────────────────────────────────────────────────────────────

#[test]
fn snapshot_column_with_spacer() {
    let col = Column::new()
        .child(Text::new("Top"))
        .child(Spacer)
        .child(Text::new("Bottom"));
    let output = render_to_string(&col, 20, 5);
    insta::assert_snapshot!(output);
}

// ══════════════════════════════════════════════════════════════════════
// Widget state variant tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn snapshot_progress_bar_zero() {
    let bar = ProgressBar::new(0.0);
    let output = render_to_string(&bar, 40, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_progress_bar_full() {
    let bar = ProgressBar::new(100.0);
    let output = render_to_string(&bar, 40, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_modal_hidden() {
    let modal = Modal::new()
        .title("Hidden")
        .body("Should not appear")
        .visible(false);
    let output = render_to_string(&modal, 60, 12);
    insta::assert_snapshot!(output);
}

// ── Layout edge cases ────────────────────────────────────────────────

#[test]
fn snapshot_narrow_column() {
    let col = Column::new()
        .child(Text::new("Hello world"))
        .child(Button::new("OK"));
    let output = render_to_string(&col, 5, 3);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_single_cell() {
    let text = Text::new("X");
    let output = render_to_string(&text, 1, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_wide_table() {
    let table = Table::new()
        .headers(vec!["A", "B", "C", "D", "E"])
        .column_widths(vec![20.0, 20.0, 20.0, 20.0, 20.0])
        .rows(vec![vec![
            "data1".to_string(),
            "data2".to_string(),
            "data3".to_string(),
            "data4".to_string(),
            "data5".to_string(),
        ]]);
    let output = render_to_string(&table, 120, 4);
    insta::assert_snapshot!(output);
}
