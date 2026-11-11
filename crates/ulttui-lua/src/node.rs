//! View nodes: what the Luau view constructors build, and how a node tree
//! becomes a ulttui view.
//!
//! Every constructor (`text`, `column`, ...) is described by one entry in
//! [`CONSTRUCTORS`]: its props and what its array part holds. Lua calls are
//! checked against that table when the node is built, so a misspelt prop is
//! reported at the line that wrote it. A [`Node`] is plain data; [`build`]
//! turns it into widgets each frame.

use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{AnyUserData, Lua, Table, UserData, Value};
use ratatui::layout::Rect;
use ratatui::Frame;
use ulttui::widgets::{
    Button, Card, Column, List, ProgressBar, RichText, Row, ScrollView, Spinner, Text, TextInput,
    Title, View,
};
use ultty_protocol::{
    AlignItems, AppMessage, BorderStyle, BorderType, Color, Constraints, Content, Dimension,
    JustifyContent, RegionId, Style,
};

/// How a prop's Lua value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    /// A string.
    Str,
    /// A number.
    Num,
    /// A boolean.
    Bool,
    /// A size: a number of cells, `"50%"`, or `"auto"`.
    Dim,
    /// A colour name (`"red"`, `"bright_blue"`), `"#rrggbb"`, or a 0-255 palette index.
    Color,
    /// An array of strings (numbers are converted).
    Strings,
    /// One of a fixed set of strings.
    OneOf(&'static [&'static str]),
}

/// One prop a constructor accepts.
#[derive(Debug)]
pub struct PropSpec {
    pub name: &'static str,
    pub kind: PropKind,
    pub doc: &'static str,
}

/// What the array part of a constructor's table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Nothing: positional values are an error.
    Empty,
    /// One string (or number): the node's text. `text("hi")` is short for `text{"hi"}`.
    Text,
    /// Child views. Strings become `text` nodes, nested arrays are spliced
    /// in, and `nil`/`false` entries are skipped.
    Children,
}

/// A view constructor exposed to Luau.
#[derive(Debug)]
pub struct CtorSpec {
    pub name: &'static str,
    pub doc: &'static str,
    pub body: Body,
    /// Props on top of [`LAYOUT_PROPS`].
    pub props: &'static [PropSpec],
    /// Props that must be given.
    pub required: &'static [&'static str],
}

const fn prop(name: &'static str, kind: PropKind, doc: &'static str) -> PropSpec {
    PropSpec { name, kind, doc }
}

const ALIGN: &[&str] = &["start", "center", "end", "stretch"];
const JUSTIFY: &[&str] = &["start", "center", "end", "between", "around", "evenly"];
const BORDER: &[&str] = &[
    "none", "thin", "thick", "double", "rounded", "dashed", "ascii",
];

/// Props every constructor accepts.
pub const LAYOUT_PROPS: &[PropSpec] = &[
    prop(
        "id",
        PropKind::Str,
        "Widget id: focus target and the region's `id` metadata",
    ),
    prop("width", PropKind::Dim, "Preferred width"),
    prop("height", PropKind::Dim, "Preferred height"),
    prop("min_width", PropKind::Dim, "Minimum width"),
    prop("max_width", PropKind::Dim, "Maximum width"),
    prop("min_height", PropKind::Dim, "Minimum height"),
    prop("max_height", PropKind::Dim, "Maximum height"),
    prop("grow", PropKind::Num, "Flex grow factor"),
    prop("shrink", PropKind::Num, "Flex shrink factor"),
];

const BOX_PROPS: &[PropSpec] = &[
    prop("gap", PropKind::Num, "Cells between children"),
    prop("padding", PropKind::Num, "Cells of padding on every side"),
    prop("border", PropKind::OneOf(BORDER), "Border line style"),
    prop("border_color", PropKind::Color, "Border colour"),
    prop("bg", PropKind::Color, "Background colour"),
    prop(
        "align",
        PropKind::OneOf(ALIGN),
        "Cross-axis alignment of children (native mode)",
    ),
    prop(
        "justify",
        PropKind::OneOf(JUSTIFY),
        "Main-axis distribution of children (native mode)",
    ),
];

/// Every view constructor, in the order the docs list them.
pub const CONSTRUCTORS: &[CtorSpec] = &[
    CtorSpec {
        name: "text",
        doc: "A line (or lines) of text: `text(\"hi\")` or `text{ \"hi\", fg = \"red\" }`",
        body: Body::Text,
        props: &[
            prop("fg", PropKind::Color, "Text colour"),
            prop("bg", PropKind::Color, "Background colour"),
            prop("bold", PropKind::Bool, "Bold text"),
        ],
        required: &[],
    },
    CtorSpec {
        name: "column",
        doc: "Children stacked top to bottom",
        body: Body::Children,
        props: BOX_PROPS,
        required: &[],
    },
    CtorSpec {
        name: "row",
        doc: "Children side by side",
        body: Body::Children,
        props: BOX_PROPS,
        required: &[],
    },
    CtorSpec {
        name: "card",
        doc: "A bordered, raised panel (ulttui `Card`)",
        body: Body::Children,
        props: &[prop(
            "title",
            PropKind::Str,
            "Heading shown above the children",
        )],
        required: &[],
    },
    CtorSpec {
        name: "scroll",
        doc: "A vertically scrolling area (ulttui `ScrollView`)",
        body: Body::Children,
        props: &[],
        required: &[],
    },
    CtorSpec {
        name: "list",
        doc: "A bordered list of strings with one selected row",
        body: Body::Empty,
        props: &[
            prop("items", PropKind::Strings, "The rows"),
            prop(
                "selected",
                PropKind::Num,
                "1-based index of the selected row",
            ),
        ],
        required: &["items"],
    },
    CtorSpec {
        name: "progress",
        doc: "A progress bar",
        body: Body::Empty,
        props: &[
            prop("value", PropKind::Num, "Progress from 0 to 1"),
            prop("label", PropKind::Str, "Text next to the bar"),
        ],
        required: &["value"],
    },
    CtorSpec {
        name: "spinner",
        doc: "A busy indicator",
        body: Body::Empty,
        props: &[
            prop(
                "frame",
                PropKind::Num,
                "Animation frame; defaults to the clock, so it moves on every redraw",
            ),
            prop("label", PropKind::Str, "Text next to the spinner"),
        ],
        required: &[],
    },
    CtorSpec {
        name: "button",
        doc: "A focusable button; Enter on it arrives as a key message with `focused` set",
        body: Body::Empty,
        props: &[
            prop("label", PropKind::Str, "Button text"),
            prop(
                "variant",
                PropKind::OneOf(&["default", "primary", "danger", "success"]),
                "Colour scheme",
            ),
        ],
        required: &["label"],
    },
    CtorSpec {
        name: "input",
        doc: "A single-line text field showing `value`; the script owns the text",
        body: Body::Empty,
        props: &[
            prop("value", PropKind::Str, "Current text"),
            prop("placeholder", PropKind::Str, "Shown while empty"),
        ],
        required: &[],
    },
];

/// A parsed prop value.
#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    Str(String),
    Num(f64),
    Bool(bool),
    Dim(Dimension),
    Color(Color),
    Strings(Vec<String>),
}

/// One view node built by a Luau constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: &'static str,
    pub props: BTreeMap<&'static str, PropValue>,
    pub text: Option<String>,
    pub children: Vec<Rc<Node>>,
}

impl Node {
    fn str(&self, name: &str) -> Option<&str> {
        match self.props.get(name) {
            Some(PropValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    fn num(&self, name: &str) -> Option<f64> {
        match self.props.get(name) {
            Some(PropValue::Num(n)) => Some(*n),
            _ => None,
        }
    }

    fn bool(&self, name: &str) -> bool {
        matches!(self.props.get(name), Some(PropValue::Bool(true)))
    }

    fn dim(&self, name: &str) -> Option<Dimension> {
        match self.props.get(name) {
            Some(PropValue::Dim(d)) => Some(*d),
            _ => None,
        }
    }

    fn color(&self, name: &str) -> Option<Color> {
        match self.props.get(name) {
            Some(PropValue::Color(c)) => Some(*c),
            _ => None,
        }
    }

    fn strings(&self, name: &str) -> &[String] {
        match self.props.get(name) {
            Some(PropValue::Strings(items)) => items,
            _ => &[],
        }
    }

    /// A plain text node.
    pub fn text(text: impl Into<String>) -> Self {
        Node {
            kind: "text",
            props: BTreeMap::new(),
            text: Some(text.into()),
            children: Vec::new(),
        }
    }
}

/// A node as Lua sees it: an opaque userdata value.
#[derive(Debug, Clone)]
pub struct NodeHandle(pub Rc<Node>);

impl UserData for NodeHandle {}

/// An error at the Lua line that called into Rust, as `file:line: message`.
pub fn located(lua: &Lua, message: impl std::fmt::Display) -> mlua::Error {
    // Level 1 is the Lua function that called the running Rust function.
    let place = lua.inspect_stack(1, |debug| {
        let source = debug.source().short_src.map(|s| s.into_owned());
        (source, debug.current_line())
    });
    match place {
        Some((Some(src), Some(line))) => mlua::Error::runtime(format!("{src}:{line}: {message}")),
        _ => mlua::Error::runtime(message.to_string()),
    }
}

/// Install one Lua function per entry of [`CONSTRUCTORS`] into `target`.
pub fn register(lua: &Lua, target: &Table) -> mlua::Result<()> {
    for spec in CONSTRUCTORS {
        let f = lua.create_function(move |lua, arg: Value| {
            parse_node(spec, arg)
                .map(|node| NodeHandle(Rc::new(node)))
                .map_err(|message| located(lua, format!("{}: {message}", spec.name)))
        })?;
        target.set(spec.name, f)?;
    }
    Ok(())
}

fn find_prop(spec: &CtorSpec, name: &str) -> Option<&'static PropSpec> {
    LAYOUT_PROPS
        .iter()
        .chain(spec.props.iter())
        .find(|p| p.name == name)
}

fn prop_names(spec: &CtorSpec) -> String {
    LAYOUT_PROPS
        .iter()
        .chain(spec.props.iter())
        .map(|p| p.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check a constructor call's argument against `spec` and build the node.
pub fn parse_node(spec: &CtorSpec, arg: Value) -> Result<Node, String> {
    let mut node = Node {
        kind: spec.name,
        props: BTreeMap::new(),
        text: None,
        children: Vec::new(),
    };
    let table = match arg {
        Value::Table(table) => table,
        Value::String(_) | Value::Integer(_) | Value::Number(_) if spec.body == Body::Text => {
            node.text = Some(scalar_to_string(&arg).unwrap_or_default());
            return Ok(node);
        }
        Value::Nil if spec.body != Body::Text && spec.required.is_empty() => return Ok(node),
        other => {
            return Err(format!(
                "expected a table of props, got {}",
                other.type_name()
            ))
        }
    };

    let mut positional = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|e| e.to_string())?;
        match key {
            Value::String(name) => {
                let name = name.to_string_lossy();
                let Some(prop) = find_prop(spec, &name) else {
                    return Err(format!(
                        "unknown prop '{name}' (expected one of: {})",
                        prop_names(spec)
                    ));
                };
                let parsed = parse_prop(prop.kind, value).map_err(|e| format!("{name}: {e}"))?;
                node.props.insert(prop.name, parsed);
            }
            Value::Integer(i) if i >= 1 => positional.push((i, value)),
            other => {
                return Err(format!(
                    "unexpected key {} (props are named; children go in the array part)",
                    describe_value(&other)
                ))
            }
        }
    }
    positional.sort_by_key(|(i, _)| *i);

    for required in spec.required {
        if !node.props.contains_key(required) {
            return Err(format!("missing required prop '{required}'"));
        }
    }

    match spec.body {
        Body::Empty => {
            if let Some((i, _)) = positional.first() {
                return Err(format!(
                    "takes no positional values, got one at [{i}]; use named props ({})",
                    prop_names(spec)
                ));
            }
        }
        Body::Text => match positional.as_slice() {
            [] => node.text = Some(String::new()),
            [(_, value)] => {
                node.text = Some(scalar_to_string(value).ok_or_else(|| {
                    format!("text must be a string or number, got {}", value.type_name())
                })?)
            }
            _ => return Err("takes one string; join the pieces with `..`".to_string()),
        },
        Body::Children => {
            for (i, value) in positional {
                push_children(&mut node.children, value, &format!("[{i}]"))?;
            }
        }
    }
    Ok(node)
}

fn push_children(out: &mut Vec<Rc<Node>>, value: Value, at: &str) -> Result<(), String> {
    match value {
        Value::Nil | Value::Boolean(false) => Ok(()),
        Value::UserData(ud) => {
            out.push(
                node_from_userdata(&ud).ok_or_else(|| {
                    format!("child {at} is not a view (a userdata of another kind)")
                })?,
            );
            Ok(())
        }
        Value::String(_) | Value::Integer(_) | Value::Number(_) => {
            out.push(Rc::new(Node::text(
                scalar_to_string(&value).unwrap_or_default(),
            )));
            Ok(())
        }
        Value::Table(list) => {
            let mut items = Vec::new();
            for pair in list.pairs::<Value, Value>() {
                let (key, item) = pair.map_err(|e| e.to_string())?;
                match key {
                    Value::Integer(i) => items.push((i, item)),
                    other => {
                        return Err(format!(
                        "child {at} is a table with key {}; a nested child list must be an array",
                        describe_value(&other)
                    ))
                    }
                }
            }
            items.sort_by_key(|(i, _)| *i);
            for (i, item) in items {
                push_children(out, item, &format!("{at}[{i}]"))?;
            }
            Ok(())
        }
        other => Err(format!(
            "child {at} is a {}; expected a view, a string, or an array of views",
            other.type_name()
        )),
    }
}

/// The node behind a userdata value, if it is a view node.
pub fn node_from_userdata(ud: &AnyUserData) -> Option<Rc<Node>> {
    ud.borrow::<NodeHandle>().ok().map(|h| h.0.clone())
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn describe_value(value: &Value) -> String {
    match value {
        Value::String(s) => format!("'{}'", s.to_string_lossy()),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.type_name().to_string(),
    }
}

fn parse_prop(kind: PropKind, value: Value) -> Result<PropValue, String> {
    let wrong = |expected: &str| format!("expected {expected}, got {}", value.type_name());
    match kind {
        PropKind::Str => match &value {
            Value::String(s) => Ok(PropValue::Str(s.to_string_lossy())),
            Value::Integer(_) | Value::Number(_) => {
                Ok(PropValue::Str(scalar_to_string(&value).unwrap_or_default()))
            }
            _ => Err(wrong("a string")),
        },
        PropKind::Num => match value {
            Value::Integer(i) => Ok(PropValue::Num(i as f64)),
            Value::Number(n) if n.is_finite() => Ok(PropValue::Num(n)),
            _ => Err(wrong("a finite number")),
        },
        PropKind::Bool => match value {
            Value::Boolean(b) => Ok(PropValue::Bool(b)),
            _ => Err(wrong("a boolean")),
        },
        PropKind::Dim => parse_dimension(&value).map(PropValue::Dim),
        PropKind::Color => parse_color(&value).map(PropValue::Color),
        PropKind::Strings => match value {
            Value::Table(list) => {
                let mut items = Vec::new();
                for item in list.sequence_values::<Value>() {
                    let item = item.map_err(|e| e.to_string())?;
                    items.push(scalar_to_string(&item).ok_or_else(|| {
                        format!(
                            "item {} is a {}; expected a string",
                            items.len() + 1,
                            item.type_name()
                        )
                    })?);
                }
                Ok(PropValue::Strings(items))
            }
            _ => Err(wrong("an array of strings")),
        },
        PropKind::OneOf(choices) => match &value {
            Value::String(s) => {
                let s = s.to_string_lossy();
                if choices.contains(&s.as_str()) {
                    Ok(PropValue::Str(s))
                } else {
                    Err(format!("'{s}' is not one of: {}", choices.join(", ")))
                }
            }
            _ => Err(wrong(&format!("one of: {}", choices.join(", ")))),
        },
    }
}

fn parse_dimension(value: &Value) -> Result<Dimension, String> {
    let number = |n: f64| {
        if n.is_finite() && n >= 0.0 {
            Ok(Dimension::Cells(n as f32))
        } else {
            Err(format!("{n} is not a size in cells"))
        }
    };
    match value {
        Value::Integer(i) => number(*i as f64),
        Value::Number(n) => number(*n),
        Value::String(s) => {
            let s = s.to_string_lossy();
            let s = s.trim();
            if s == "auto" {
                return Ok(Dimension::Auto);
            }
            if let Some(pct) = s.strip_suffix('%') {
                if let Ok(p) = pct.trim().parse::<f32>() {
                    if p.is_finite() && p >= 0.0 {
                        return Ok(Dimension::Percent(p));
                    }
                }
            }
            Err(format!(
                "'{s}' is not a size (use a number of cells, \"50%\" or \"auto\")"
            ))
        }
        other => Err(format!(
            "expected a number of cells, \"50%\" or \"auto\", got {}",
            other.type_name()
        )),
    }
}

fn parse_color(value: &Value) -> Result<Color, String> {
    match value {
        Value::Integer(i) => u8::try_from(*i)
            .map(Color::Indexed)
            .map_err(|_| format!("palette index {i} is outside 0-255")),
        Value::String(s) => ulttui::theme::parse_color(&s.to_string_lossy()),
        other => Err(format!(
            "expected a colour name, \"#rrggbb\" or 0-255, got {}",
            other.type_name()
        )),
    }
}

fn border_style(name: &str) -> BorderStyle {
    let style = match name {
        "thin" => BorderType::Thin,
        "thick" => BorderType::Thick,
        "double" => BorderType::Double,
        "rounded" => return BorderStyle::rounded(),
        "dashed" => BorderType::Dashed,
        "ascii" => BorderType::Ascii,
        _ => return BorderStyle::NONE,
    };
    BorderStyle {
        style,
        ..BorderStyle::thin()
    }
}

fn align_items(name: &str) -> AlignItems {
    match name {
        "center" => AlignItems::Center,
        "end" => AlignItems::End,
        "stretch" => AlignItems::Stretch,
        _ => AlignItems::Start,
    }
}

fn justify_content(name: &str) -> JustifyContent {
    match name {
        "center" => JustifyContent::Center,
        "end" => JustifyContent::End,
        "between" => JustifyContent::SpaceBetween,
        "around" => JustifyContent::SpaceAround,
        "evenly" => JustifyContent::SpaceEvenly,
        _ => JustifyContent::Start,
    }
}

/// A built widget plus the node's layout props and id.
///
/// The layout props patch the widget's constraints, which both the native
/// layout and the ANSI containers read. Everything else is delegated.
pub struct LuaView {
    inner: Box<dyn View>,
    id: Option<String>,
    patch: Patch,
}

#[derive(Default)]
struct Patch {
    width: Option<Dimension>,
    height: Option<Dimension>,
    min_width: Option<Dimension>,
    max_width: Option<Dimension>,
    min_height: Option<Dimension>,
    max_height: Option<Dimension>,
    grow: Option<f32>,
    shrink: Option<f32>,
    align: Option<AlignItems>,
    justify: Option<JustifyContent>,
    /// Native padding. A native border takes no room in the layout, so a
    /// bordered box pads by one more cell than the ANSI one (whose border
    /// already insets the children).
    padding: Option<f32>,
}

impl View for LuaView {
    fn constraints(&self) -> Constraints {
        let mut c = self.inner.constraints();
        let p = &self.patch;
        let set = |slot: &mut Dimension, v: Option<Dimension>| {
            if let Some(v) = v {
                *slot = v;
            }
        };
        set(&mut c.width, p.width);
        set(&mut c.height, p.height);
        set(&mut c.min_width, p.min_width);
        set(&mut c.max_width, p.max_width);
        set(&mut c.min_height, p.min_height);
        set(&mut c.max_height, p.max_height);
        if let Some(grow) = p.grow {
            c.flex_grow = grow;
        }
        if let Some(shrink) = p.shrink {
            c.flex_shrink = shrink;
        }
        if let Some(align) = p.align {
            c.align_items = align;
        }
        if let Some(justify) = p.justify {
            c.justify_content = justify;
        }
        if let Some(padding) = p.padding {
            c = c.padding_all(Dimension::Cells(padding));
        }
        c
    }

    fn style(&self) -> Style {
        self.inner.style()
    }

    fn content(&self) -> Content {
        self.inner.content()
    }

    fn children_count(&self) -> usize {
        self.inner.children_count()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> ulttui::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> ulttui::Result<Option<RegionId>> {
        self.inner
            .render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        self.inner.render_ansi(frame, area)
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        self.inner.for_each_child(visit)
    }

    fn widget_id(&self) -> Option<&str> {
        self.id.as_deref().or_else(|| self.inner.widget_id())
    }

    fn is_focusable(&self) -> bool {
        self.inner.is_focusable()
    }

    fn scroll_offset(&self) -> Option<(f32, f32)> {
        self.inner.scroll_offset()
    }
}

/// Turn a node tree into ulttui widgets.
pub fn build(node: &Node) -> LuaView {
    let id = node.str("id").map(str::to_string);
    let mut patch = Patch {
        width: node.dim("width"),
        height: node.dim("height"),
        min_width: node.dim("min_width"),
        max_width: node.dim("max_width"),
        min_height: node.dim("min_height"),
        max_height: node.dim("max_height"),
        grow: node.num("grow").map(|g| g as f32),
        shrink: node.num("shrink").map(|s| s as f32),
        ..Patch::default()
    };
    let inner: Box<dyn View> = match node.kind {
        "text" => {
            let text = node.text.clone().unwrap_or_default();
            let (fg, bg, bold) = (node.color("fg"), node.color("bg"), node.bool("bold"));
            if fg.is_none() && bg.is_none() && !bold {
                Box::new(Text::new(text))
            } else {
                let mut style = Style::new();
                style.fg = fg;
                style.bg = bg;
                style.bold = bold;
                Box::new(RichText::new().span(text, style))
            }
        }
        "column" | "row" => {
            let bordered = node.str("border").is_some_and(|b| b != "none");
            let padding = node.num("padding").unwrap_or(0.0) as f32;
            if bordered {
                patch.padding = Some(padding + 1.0);
            }
            patch.align = node.str("align").map(align_items);
            patch.justify = node.str("justify").map(justify_content);
            if node.kind == "column" {
                Box::new(build_box(node, Column::new(), padding))
            } else {
                Box::new(build_box(node, Row::new(), padding))
            }
        }
        "card" => {
            let mut card = Card::new();
            if let Some(title) = node.str("title") {
                card = card.child(Title::new(title));
            }
            for child in &node.children {
                card = card.child(build(child));
            }
            Box::new(card)
        }
        "scroll" => {
            // ScrollView holds one child: stack the children in a column.
            let mut column = Column::new();
            for child in &node.children {
                column = column.child(build(child));
            }
            Box::new(ScrollView::new().child(column))
        }
        "list" => {
            let selected = node.num("selected").unwrap_or(1.0).max(1.0) as usize - 1;
            Box::new(
                List::new()
                    .items(node.strings("items").to_vec())
                    .selected(selected),
            )
        }
        "progress" => {
            // Scripts say 0-1; ProgressBar takes a percentage.
            let mut bar = ProgressBar::new(node.num("value").unwrap_or(0.0) as f32 * 100.0);
            if let Some(label) = node.str("label") {
                bar = bar.label(label);
            }
            Box::new(bar)
        }
        "spinner" => {
            let frame = node
                .num("frame")
                .map_or_else(clock_frame, |f| f.max(0.0) as usize);
            let mut spinner = Spinner::new().frame(frame);
            if let Some(label) = node.str("label") {
                spinner = spinner.label(label);
            }
            Box::new(spinner)
        }
        "button" => {
            let mut button = Button::new(node.str("label").unwrap_or_default());
            if let Some(id) = &id {
                button = button.id(id.clone());
            }
            button = match node.str("variant") {
                Some("primary") => button.primary(),
                Some("danger") => button.danger(),
                Some("success") => button.success(),
                _ => button,
            };
            Box::new(button)
        }
        "input" => {
            let mut input = TextInput::new();
            if let Some(id) = &id {
                input = input.id(id.clone());
            }
            if let Some(value) = node.str("value") {
                input = input.value(value);
            }
            if let Some(placeholder) = node.str("placeholder") {
                input = input.placeholder(placeholder);
            }
            Box::new(input)
        }
        other => Box::new(Text::new(format!("<unknown view kind {other}>"))),
    };
    LuaView { inner, id, patch }
}

/// The builder methods `column` and `row` share.
trait BoxBuilder: Sized {
    fn gap(self, gap: f32) -> Self;
    fn padding(self, padding: f32) -> Self;
    fn border(self, border: BorderStyle) -> Self;
    fn border_color(self, color: Color) -> Self;
    fn bg(self, color: Color) -> Self;
    fn child<V: View + 'static>(self, child: V) -> Self;
}

macro_rules! box_builder {
    ($ty:ty) => {
        impl BoxBuilder for $ty {
            fn gap(self, gap: f32) -> Self {
                <$ty>::gap(self, gap)
            }
            fn padding(self, padding: f32) -> Self {
                <$ty>::padding(self, padding)
            }
            fn border(self, border: BorderStyle) -> Self {
                <$ty>::border(self, border)
            }
            fn border_color(self, color: Color) -> Self {
                <$ty>::border_color(self, color)
            }
            fn bg(self, color: Color) -> Self {
                <$ty>::bg(self, color)
            }
            fn child<V: View + 'static>(self, child: V) -> Self {
                <$ty>::child(self, child)
            }
        }
    };
}
box_builder!(Column);
box_builder!(Row);

fn build_box<B: BoxBuilder>(node: &Node, mut b: B, padding: f32) -> B {
    if let Some(gap) = node.num("gap") {
        b = b.gap(gap as f32);
    }
    if padding > 0.0 {
        b = b.padding(padding);
    }
    if let Some(border) = node.str("border") {
        b = b.border(border_style(border));
    }
    if let Some(color) = node.color("border_color") {
        b = b.border_color(color);
    }
    if let Some(color) = node.color("bg") {
        b = b.bg(color);
    }
    for child in &node.children {
        b = b.child(build(child));
    }
    b
}

fn clock_frame() -> usize {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    (millis / 80) as usize % Spinner::frame_count()
}
