//! Lua view tables to ulttui views: every constructor and prop.

use ulttui::widgets::View;
use ulttui_lua::node::{build, CONSTRUCTORS, LAYOUT_PROPS};
use ultty_protocol::{
    AlignItems, AppMessage, BorderType, Color, Content, Dimension, JustifyContent, NamedColor,
    WidgetContent,
};

use crate::common::{ansi, messages, texts, view_of};

/// The smallest valid call of each constructor.
fn minimal_call(name: &str) -> &'static str {
    match name {
        "text" => r#"text("x")"#,
        "column" => r#"column { "x" }"#,
        "row" => r#"row { "x" }"#,
        "card" => r#"card { "x" }"#,
        "scroll" => r#"scroll { "x" }"#,
        "list" => r#"list { items = { "x" } }"#,
        "progress" => r#"progress { value = 0.5 }"#,
        "spinner" => r#"spinner {}"#,
        "button" => r#"button { label = "x" }"#,
        "input" => r#"input { value = "x" }"#,
        other => panic!("no minimal call for constructor {other}; add one"),
    }
}

#[test]
fn every_constructor_builds_a_region_and_takes_every_layout_prop() {
    for spec in CONSTRUCTORS {
        let call = minimal_call(spec.name);
        let node =
            view_of(&format!("ctor-{}", spec.name), call).unwrap_or_else(|e| panic!("{call}: {e}"));
        assert_eq!(node.kind, spec.name);
        let msgs = messages(&node);
        assert!(
            matches!(msgs.first(), Some(AppMessage::CreateRegion { .. })),
            "{call} created no region"
        );

        // Layout props work on every constructor.
        let with_layout = call.replacen(
            '{',
            r#"{ id = "n", width = 7, height = "50%", min_width = 1, max_width = 9,
                 min_height = 1, max_height = "auto", grow = 2, shrink = 0, "#,
            1,
        );
        let with_layout = if with_layout == call {
            // `text("x")` has no table to add props to.
            r#"text { "x", id = "n", width = 7, height = "50%", min_width = 1, max_width = 9,
                 min_height = 1, max_height = "auto", grow = 2, shrink = 0 }"#
                .to_string()
        } else {
            with_layout
        };
        let node = view_of(&format!("layout-{}", spec.name), &with_layout)
            .unwrap_or_else(|e| panic!("{with_layout}: {e}"));
        let view = build(&node);
        let c = view.constraints();
        assert_eq!(c.width, Dimension::Cells(7.0), "{}", spec.name);
        assert_eq!(c.height, Dimension::Percent(50.0), "{}", spec.name);
        assert_eq!(c.min_width, Dimension::Cells(1.0), "{}", spec.name);
        assert_eq!(c.max_width, Dimension::Cells(9.0), "{}", spec.name);
        assert_eq!(c.min_height, Dimension::Cells(1.0), "{}", spec.name);
        assert_eq!(c.max_height, Dimension::Auto, "{}", spec.name);
        assert_eq!(c.flex_grow, 2.0, "{}", spec.name);
        assert_eq!(c.flex_shrink, 0.0, "{}", spec.name);
        assert_eq!(view.widget_id(), Some("n"), "{}", spec.name);
    }
}

#[test]
fn every_documented_prop_is_in_the_api_reference() {
    let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/lua.md"))
        .expect("docs/lua.md exists");
    for spec in CONSTRUCTORS {
        assert!(
            docs.contains(&format!("`{}`", spec.name)),
            "docs/lua.md does not mention `{}`",
            spec.name
        );
        for prop in LAYOUT_PROPS.iter().chain(spec.props) {
            assert!(
                docs.contains(&format!("`{}`", prop.name)),
                "docs/lua.md does not mention prop `{}`",
                prop.name
            );
        }
    }
}

#[test]
fn text_plain_and_styled() {
    let plain = view_of("text-plain", r#"text("hello")"#).unwrap();
    let msgs = messages(&plain);
    assert!(msgs.iter().any(|m| matches!(m,
        AppMessage::SetContent { content: Content::Text(t), .. } if t.text == "hello")));

    let number = view_of("text-number", "text(42)").unwrap();
    assert_eq!(texts(&messages(&number)), ["42"]);

    let styled = view_of(
        "text-styled",
        r##"text { "hi", fg = "red", bg = "#102030", bold = true }"##,
    )
    .unwrap();
    let span = messages(&styled)
        .into_iter()
        .find_map(|m| match m {
            AppMessage::SetContent {
                content: Content::RichText(r),
                ..
            } => r.spans.into_iter().next(),
            _ => None,
        })
        .expect("styled text is rich text");
    assert_eq!(span.text, "hi");
    assert_eq!(span.style.fg, Some(Color::Named(NamedColor::Red)));
    assert_eq!(span.style.bg, Some(Color::Rgb(0x10, 0x20, 0x30)));
    assert!(span.style.bold);

    let indexed = view_of("text-indexed", r#"text { "hi", fg = 208 }"#).unwrap();
    let span_fg = format!("{:?}", messages(&indexed));
    assert!(span_fg.contains("Indexed(208)"), "{span_fg}");
}

#[test]
fn column_and_row_box_props() {
    for kind in ["column", "row"] {
        let node = view_of(
            &format!("box-{kind}"),
            &format!(
                r##"{kind} {{ gap = 2, padding = 1, border = "rounded", border_color = "cyan",
                    bg = "#000000", align = "center", justify = "between", "a", "b" }}"##
            ),
        )
        .unwrap();
        let view = build(&node);
        let c = view.constraints();
        let gap = if kind == "column" {
            c.gap.height
        } else {
            c.gap.width
        };
        assert_eq!(gap, Dimension::Cells(2.0), "{kind}");
        // Natively the border takes no room, so padding grows by one cell.
        assert_eq!(c.padding.left, Dimension::Cells(2.0), "{kind}");
        assert_eq!(c.align_items, AlignItems::Center, "{kind}");
        assert_eq!(c.justify_content, JustifyContent::SpaceBetween, "{kind}");
        let style = view.style();
        assert_eq!(style.border.style, BorderType::Rounded, "{kind}");
        assert_eq!(
            style.border_color,
            Some(Color::Named(NamedColor::Cyan)),
            "{kind}"
        );
        assert_eq!(style.bg, Some(Color::Rgb(0, 0, 0)), "{kind}");
        assert_eq!(texts(&messages(&node)), ["a", "b"], "{kind}");
    }

    // Without a border the padding is what was asked for.
    let plain = view_of("box-plain", r#"column { padding = 1, "a" }"#).unwrap();
    assert_eq!(
        build(&plain).constraints().padding.top,
        Dimension::Cells(1.0)
    );
}

#[test]
fn children_strings_nested_lists_and_holes() {
    let node = view_of(
        "children",
        r#"column { "a", { "b", { text("c") } }, nil, false, 4, row { "e" } }"#,
    )
    .unwrap();
    assert_eq!(texts(&messages(&node)), ["a", "b", "c", "4", "e"]);
}

#[test]
fn list_items_and_selection() {
    let node = view_of(
        "list",
        r#"list { id = "l", items = { "one", "two", 3 }, selected = 2 }"#,
    )
    .unwrap();
    let msgs = messages(&node);
    assert_eq!(texts(&msgs), ["one", "two", "3"]);
    // The selected row (the second) is the one drawn with a background.
    let backgrounds: Vec<bool> = msgs
        .iter()
        .filter_map(|m| match m {
            AppMessage::CreateRegion { style, .. } if style.border.style == BorderType::None => {
                Some(style.bg.is_some())
            }
            _ => None,
        })
        .collect();
    assert_eq!(backgrounds, [false, true, false]);
    assert_eq!(build(&node).widget_id(), Some("l"));
}

#[test]
fn widgets_carry_their_props() {
    let progress = view_of("progress", r#"progress { value = 0.25, label = "copy" }"#).unwrap();
    let dump = format!("{:?}", messages(&progress));
    assert!(dump.contains("0.25") && dump.contains("copy"), "{dump}");

    let spinner = view_of("spinner", r#"spinner { frame = 3, label = "wait" }"#).unwrap();
    assert!(format!("{:?}", messages(&spinner)).contains("wait"));

    let button = view_of(
        "button",
        r#"button { id = "go", label = "Go", variant = "danger" }"#,
    )
    .unwrap();
    let view = build(&button);
    assert!(view.is_focusable());
    assert_eq!(view.widget_id(), Some("go"));
    assert!(messages(&button).iter().any(|m| matches!(m,
        AppMessage::SetContent { content: Content::Widget(WidgetContent::Button(b)), .. }
            if b.label == "Go")));

    let input = view_of(
        "input",
        r#"input { id = "name", value = "ann", placeholder = "who?" }"#,
    )
    .unwrap();
    let view = build(&input);
    assert!(view.is_focusable());
    assert_eq!(view.widget_id(), Some("name"));
    assert!(format!("{:?}", messages(&input)).contains("ann"));

    let card = view_of("card", r#"card { title = "T", "body" }"#).unwrap();
    assert_eq!(texts(&messages(&card)), ["T", "body"]);

    let scroll = view_of("scroll", r#"scroll { "a", "b" }"#).unwrap();
    assert_eq!(texts(&messages(&scroll)), ["a", "b"]);
}

#[test]
fn bad_props_are_errors_at_the_line_that_wrote_them() {
    let cases = [
        (
            r#"column { colour = "red" }"#,
            "column: unknown prop 'colour' (expected one of: id, width",
        ),
        (
            r#"text { "x", fg = "mauve" }"#,
            "text: fg: 'mauve' is not a colour",
        ),
        (
            r#"text { "x", width = "wide" }"#,
            "text: width: 'wide' is not a size",
        ),
        (
            r#"row { border = "wavy" }"#,
            "row: border: 'wavy' is not one of",
        ),
        (
            r#"row { gap = "2" }"#,
            "row: gap: expected a finite number, got string",
        ),
        (
            r#"list { selected = 1 }"#,
            "list: missing required prop 'items'",
        ),
        (
            r#"button { "go" }"#,
            "button: missing required prop 'label'",
        ),
        (
            r#"progress { value = 1, "x" }"#,
            "progress: takes no positional values",
        ),
        (r#"column { print }"#, "column: child [1] is a function"),
        (r#"text { "a", "b" }"#, "text: takes one string"),
    ];
    for (i, (expr, expected)) in cases.into_iter().enumerate() {
        let error = view_of(&format!("bad-prop-{i}"), expr).unwrap_err();
        let path = crate::common::script_path(&format!("bad-prop-{i}"));
        // The view function is on line 1 of the generated script.
        let prefix = format!("view: {}:1: {expected}", path.display());
        assert!(
            error.starts_with(&prefix),
            "{expr}\n  got:  {error}\n  want: {prefix}..."
        );
    }
}

#[test]
fn a_view_renders_in_ansi_mode_too() {
    let node = view_of(
        "ansi",
        r#"column { padding = 1, border = "thin", text("Count: 3"), row { gap = 1, "a", "b" } }"#,
    )
    .unwrap();
    let screen = ansi(&build(&node), 20, 6);
    assert!(screen.contains("Count: 3"), "{screen}");
    // ratatui shares a row's width out between its children.
    assert!(
        screen
            .lines()
            .any(|l| l.starts_with("│ a ") && l.contains(" b ")),
        "{screen}"
    );
    assert!(screen.starts_with('┌'), "{screen}");
}
