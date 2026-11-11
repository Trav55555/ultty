//! Snapshot tests for widget rendering.
//!
//! Uses insta to capture widget output and detect regressions.
//! Run `cargo insta review` to review and approve snapshot changes.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ulttui::event::{Event, Key, KeyEvent, Modifiers, MouseEvent, MouseEventKind};
use ulttui::widgets::{
    Badge, Button, Card, Checkbox, Collapsible, Column, DiffView, Divider, DockLayout, Image,
    Inset, List, MarkdownView, Modal, ProgressBar, Row, ScrollView, Section, Select, Spacer,
    Spinner, Stack, Table, Tabs, Text, TextInput, Toggle, Tree, TreeNode, View,
};

/// Render a widget and return its buffer as a string for snapshot comparison.
///
/// Panics if nothing was drawn: every widget snapshotted through this helper
/// is expected to render something, so an empty snapshot is a regression, not
/// a baseline. Use [`render_to_string_allow_empty`] for the rare widget whose
/// correct output is nothing.
fn render_to_string(widget: &impl View, width: u16, height: u16) -> String {
    let output = render_to_string_allow_empty(widget, width, height);
    assert!(
        !output.is_empty(),
        "widget rendered nothing into a {width}x{height} area"
    );
    output
}

fn render_to_string_allow_empty(widget: &impl View, width: u16, height: u16) -> String {
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

/// Convert a ratatui buffer to a human-readable string: the symbols first,
/// then a summary of every run of non-default styling (`row:x0-x1 fg=.. bg=..
/// mod=..`), so style regressions show up in snapshots too.
fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::{Color, Modifier};

    let mut lines = Vec::new();
    let mut style_runs = Vec::new();

    for y in 0..buffer.area.height {
        let mut line = String::new();
        let mut run: Option<(u16, u16, Color, Color, Modifier)> = None;
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            line.push_str(cell.symbol());

            let styled = cell.fg != Color::Reset
                || cell.bg != Color::Reset
                || cell.modifier != Modifier::empty();
            match run {
                Some((x0, x1, fg, bg, modifier))
                    if styled
                        && x1 + 1 == x
                        && (fg, bg, modifier) == (cell.fg, cell.bg, cell.modifier) =>
                {
                    run = Some((x0, x, fg, bg, modifier));
                }
                _ => {
                    if let Some(finished) = run.take() {
                        style_runs.push((y, finished));
                    }
                    if styled {
                        run = Some((x, x, cell.fg, cell.bg, cell.modifier));
                    }
                }
            }
        }
        if let Some(finished) = run.take() {
            style_runs.push((y, finished));
        }
        // Trim trailing spaces but keep the line
        lines.push(line.trim_end().to_string());
    }

    // Remove trailing empty lines
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    let mut output = lines.join("\n");
    if !style_runs.is_empty() {
        output.push_str("\n-- styles --");
        for (y, (x0, x1, fg, bg, modifier)) in style_runs {
            output.push_str(&format!(
                "\n{y}:{x0}-{x1} fg={fg:?} bg={bg:?} mod={modifier:?}"
            ));
        }
    }
    output
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
    // The two tabs advance to columns 8 and 16 before either renderer sees them.
    let text = Text::new("Tabs:\t\tSpaces   End");
    let output = render_to_string(&text, 30, 1);
    insta::assert_snapshot!(output);

    let ultty_protocol::Content::Text(native) = View::content(&text) else {
        panic!("Expected text");
    };
    assert_eq!(output, native.text);

    let rich = ulttui::widgets::RichText::new()
        .text("Tabs:")
        .text("\t")
        .text("\tSpaces   End");
    assert_eq!(render_to_string(&rich, 30, 1), output);
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

#[test]
fn snapshot_markdown_streaming() {
    let md = MarkdownView::new("Partial **markdown** response").streaming(true);
    let output = render_to_string(&md, 40, 3);
    insta::assert_snapshot!(output);
}

// ── Semantics (ADR 0021) ──────────────────────────────────────────────

/// The `SetSemantics` each widget's native render sends, one line per
/// region that gets one, as `widget: region role label … state`. Regions
/// that send none are left to what the terminal derives from content.
#[test]
fn snapshot_widget_semantics() {
    use ulttui::widgets::{render_view_messages, Badge, ErrorState, LoadingState, Semantic, Title};
    use ultty_protocol::{AppMessage, Live, Role};

    let widgets: Vec<(&str, Box<dyn View>)> = vec![
        ("Text", Box::new(Text::new("plain"))),
        ("Column", Box::new(Column::new().child(Text::new("a")))),
        ("Button", Box::new(Button::new("Run"))),
        (
            "Button+description",
            Box::new(Button::new("Run").description("Runs tests")),
        ),
        ("Checkbox", Box::new(Checkbox::new("Accept").checked(true))),
        ("Toggle", Box::new(Toggle::new("Dark mode"))),
        (
            "ProgressBar",
            Box::new(ProgressBar::new(42.0).label("Upload")),
        ),
        ("Spinner", Box::new(Spinner::new().label("Working"))),
        ("LoadingState", Box::new(LoadingState::new("Loading"))),
        (
            "ErrorState",
            Box::new(ErrorState::new("disk full").hint("free space")),
        ),
        ("Badge", Box::new(Badge::success("Done"))),
        ("Title", Box::new(Title::new("Chat"))),
        (
            "Section",
            Box::new(Section::new().title("Files").child(Text::new("a"))),
        ),
        ("Divider", Box::new(Divider::horizontal())),
        ("Spacer", Box::new(Spacer)),
        (
            "Image",
            Box::new(Image::new(vec![0; 4], 1, 1).label("A red dot")),
        ),
        (
            "TextInput",
            Box::new(TextInput::new().placeholder("Type").label("Message")),
        ),
        (
            "TextInput+password",
            Box::new(TextInput::new().value("pw").password()),
        ),
        (
            "Select",
            Box::new(Select::new(vec!["a".into(), "b".into()]).label("Letter")),
        ),
        ("Select+expanded", {
            let mut select = Select::new(vec!["a".into(), "b".into()])
                .label("Letter")
                .selected(1);
            select.on_key(&KeyEvent {
                key: Key::Enter,
                modifiers: Modifiers::default(),
                focused_id: None,
            });
            Box::new(select)
        }),
        (
            "List",
            Box::new(List::new().items(vec!["one", "two"]).label("Numbers")),
        ),
        (
            "Tabs",
            Box::new(Tabs::new().labels(vec!["Log", "Diff"]).active(1)),
        ),
        (
            "Tree",
            Box::new(
                Tree::new(
                    TreeNode::with_children("src", vec![TreeNode::new("main.rs")]).expanded(true),
                )
                .label("Files"),
            ),
        ),
        (
            "Table",
            Box::new(
                Table::new()
                    .headers(vec!["Name", "Age"])
                    .rows(vec![vec!["Ada", "36"], vec!["Linus", "28"]]),
            ),
        ),
        (
            "Collapsible",
            Box::new(Collapsible::new(Text::new("More"), Text::new("body"))),
        ),
        (
            "Modal",
            Box::new(
                Modal::new()
                    .title("Delete?")
                    .body("Really?")
                    .buttons(vec!["Yes", "No"]),
            ),
        ),
        ("MarkdownView", Box::new(MarkdownView::new("# Done"))),
        (
            "MarkdownView+streaming",
            Box::new(MarkdownView::new("Partial").streaming(true)),
        ),
        (
            "Semantic",
            Box::new(
                Semantic::new(Text::new("Bot: hi"))
                    .role(Role::Log)
                    .label("Messages")
                    .live(Live::Polite),
            ),
        ),
    ];

    let mut out = String::new();
    for (name, widget) in &widgets {
        for message in render_view_messages(widget.as_ref()).unwrap() {
            if let AppMessage::SetSemantics { id, semantics } = message {
                let mut line = format!("{name}: r{} {}", id.0, semantics.role.as_str());
                if let Some(label) = &semantics.label {
                    line.push_str(&format!(" {label:?}"));
                }
                if let Some(description) = &semantics.description {
                    line.push_str(&format!(" desc={description:?}"));
                }
                if semantics.live != Live::Off {
                    line.push_str(&format!(" live={}", semantics.live.as_str()));
                }
                for (set, flag) in [
                    (semantics.busy, "busy"),
                    (semantics.hidden, "hidden"),
                    (semantics.modal, "modal"),
                    (semantics.selected, "selected"),
                ] {
                    if set {
                        line.push(' ');
                        line.push_str(flag);
                    }
                }
                if semantics.toggled != Default::default() {
                    line.push_str(&format!(" toggled={:?}", semantics.toggled));
                }
                if semantics.expanded != Default::default() {
                    line.push_str(&format!(" expanded={:?}", semantics.expanded));
                }
                if let Some(value) = semantics.value {
                    line.push_str(&format!(
                        " value={}/{}..{}",
                        value.now, value.min, value.max
                    ));
                }
                if let Some(value_text) = &semantics.value_text {
                    line.push_str(&format!(" value_text={value_text:?}"));
                }
                if let Some(position) = semantics.position {
                    line.push_str(&format!(" pos={}/{}", position.index, position.size));
                }
                if semantics.level > 0 {
                    line.push_str(&format!(" level={}", semantics.level));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    insta::assert_snapshot!(out);
}

/// The positions of the regions a widget's native render marks selected.
fn selected_positions(view: &dyn View) -> Vec<Option<u32>> {
    use ultty_protocol::AppMessage;
    ulttui::widgets::render_view_messages(view)
        .unwrap()
        .into_iter()
        .filter_map(|message| match message {
            AppMessage::SetSemantics { semantics, .. } if semantics.selected => {
                Some(semantics.position.map(|position| position.index))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn list_and_tabs_mark_only_their_selected_item_selected() {
    let list = List::new().items(vec!["one", "two", "three"]).selected(1);
    assert_eq!(selected_positions(&list), vec![Some(2)]);

    let tabs = Tabs::new().labels(vec!["Log", "Diff", "Files"]).active(2);
    assert_eq!(selected_positions(&tabs), vec![Some(3)]);
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
fn snapshot_card_nested_content() {
    let card = Card::new()
        .child(Text::new("Player"))
        .child(
            Row::new()
                .child(Badge::success("Ready"))
                .child(Text::new("HP 12")),
        )
        .child(ProgressBar::new(65.0).label("charge"));
    let output = render_to_string(&card, 36, 7);
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
    let output = render_to_string_allow_empty(&modal, 60, 12);
    assert!(output.is_empty(), "hidden modal must draw nothing");
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
fn snapshot_column_with_gap() {
    let col = Column::new()
        .gap(1.0)
        .child(Text::new("Top"))
        .child(Text::new("Middle"))
        .child(Text::new("Bottom"));
    let output = render_to_string(&col, 20, 7);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_row_with_fixed_and_flex_children() {
    let row = Row::new()
        .gap(2.0)
        .child(Button::new("Fixed"))
        .child(Spacer)
        .child(Button::new("Tail"));
    let output = render_to_string(&row, 40, 1);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_scrollview_after_scroll_down() {
    let mut scroll = ScrollView::new();
    scroll.set_content_height(10.0);
    scroll.set_viewport_height(3.0);
    let handled = scroll.on_event(&Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        x: 0,
        y: 0,
        modifiers: Modifiers::default(),
    }));
    assert!(handled);

    let output = render_to_string(&scroll, 30, 5);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_scrollview_scrolled_shows_window_of_child() {
    let lines: Vec<String> = (0..20).map(|i| format!("Line {i}")).collect();
    let mut scroll = ScrollView::new().child(Text::new(lines.join("\n")));
    scroll.set_content_height(20.0);
    scroll.set_viewport_height(5.0);
    scroll.scroll_to(10.0);

    // 7 rows: 2 for the border, 5 for the viewport.
    let output = render_to_string(&scroll, 20, 7);
    assert!(output.contains("Line 10"));
    assert!(output.contains("Line 14"));
    assert!(!output.contains("Line 9"));
    assert!(!output.contains("Line 15"));
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

// ── User overrides (ADR 0023) ─────────────────────────────────────────

/// The three-button row both override snapshots draw.
fn override_fixture() -> Row {
    Row::new()
        .gap(1.0)
        .child(Button::new("Save").id("save"))
        .child(Button::new("Delete").id("delete"))
        .child(Button::new("Quit").id("quit"))
}

/// `view` under `overrides`, drawn both ways: the native messages in brief,
/// then the ANSI buffer.
fn render_overridden(
    overrides: std::rc::Rc<ulttui::overrides::Overrides>,
    view: &dyn View,
) -> String {
    use ulttui::overrides::with_overrides;
    use ulttui::widgets::{render_view_ansi, render_view_messages};
    use ultty_protocol::{AppMessage, Content, WidgetContent};

    let (native, ansi) = with_overrides(overrides, || {
        let native = render_view_messages(view).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
        terminal
            .draw(|frame| render_view_ansi(view, frame, frame.area()))
            .unwrap();
        (native, buffer_to_string(terminal.backend().buffer()))
    });

    let mut out = String::from("-- native --\n");
    for message in native {
        match message {
            AppMessage::CreateRegion {
                id,
                constraints,
                style,
                ..
            } => out.push_str(&format!(
                "r{} create display={:?} fg={:?} bg={:?} bold={}\n",
                id.0, constraints.display, style.fg, style.bg, style.bold
            )),
            AppMessage::SetContent {
                id,
                content: Content::Widget(WidgetContent::Button(button)),
            } => out.push_str(&format!("r{} button {:?}\n", id.0, button.label)),
            other => out.push_str(&format!("{other:?}\n")),
        }
    }
    out.push_str("-- ansi --\n");
    out.push_str(&ansi);
    out
}

/// One app view under a user's overrides, drawn both ways: a button with
/// new text, colour and bold, one hidden, one with a new background.
#[test]
fn snapshot_user_overrides_native_and_ansi() {
    let overrides = ulttui::overrides::Overrides::parse(
        r##"
        [widget.save]
        text = "Write"
        fg = "red"
        bold = true
        [widget.delete]
        hidden = true
        [widget.quit]
        bg = "#203040"
        "##,
    )
    .unwrap();
    insta::assert_snapshot!(render_overridden(
        std::rc::Rc::new(overrides),
        &override_fixture()
    ));
}

/// The same view under a Luau override file that computes the same
/// changes from what each widget shows; the output matches the TOML one.
#[cfg(feature = "lua-overrides")]
#[test]
fn snapshot_user_luau_overrides_native_and_ansi() {
    let overrides = ulttui::overrides::Overrides::parse_luau(
        r##"
        return {
          widget = function(id, info)
            if info.role ~= "button" then return nil end
            if info.text == "Save" then return { text = "Write", fg = "red", bold = true } end
            if info.text:find("^Del") then return { hidden = true } end
            if id == "quit" then return { bg = "#203040" } end
          end,
        }
        "##,
        "snapshot.luau",
    )
    .unwrap();
    insta::assert_snapshot!(render_overridden(
        std::rc::Rc::new(overrides),
        &override_fixture()
    ));
}
