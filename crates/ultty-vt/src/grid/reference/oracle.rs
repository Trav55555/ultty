//! The differential oracle (ADR 0022, migration slice 2).
//!
//! Random streams drive a live [`crate::grid::Terminal`] and a
//! [`super::Terminal`] over the frozen storage side by side, through the same
//! parser on both feed paths, with resizes, scrollback-limit changes,
//! scrolling back and inline-surface row rewrites in between. After every
//! step the two must agree on the cursor and its state, every visible row,
//! every scrollback row (cells, grapheme extras, wrapped, marks, anchors),
//! `history_len` and the anchors.
//!
//! The streams are `tests/support/streams.rs` (the ones `tests/integrity.rs`
//! uses) plus fragments aimed at what the page-storage slices rewrite: scroll
//! regions with IL, DL, SU, SD and reverse index, wide characters and
//! clusters at the margins, SGR churn, shell marks, bursts of lines that
//! fill and evict a small scrollback, and resizes.
//!
//! 1024 cases by default, about 2 s; set `ULTTY_ORACLE_CASES` for a longer
//! run (100000 takes a few minutes).

use proptest::prelude::*;

use crate::ansi::{AnsiOutput, AnsiParser};
use crate::grid::{self as live, Cell, Dimensions};

#[path = "../../../tests/support/streams.rs"]
mod streams;

use streams::Step;

#[derive(Debug, Clone)]
enum Op {
    /// Bytes or a resize, as `tests/integrity.rs` makes them.
    Stream(Step),
    /// Resize to this many rows, keeping the width: rows move between the
    /// screen and scrollback without reflow.
    Height(u16),
    /// Resize to this many columns, keeping the height: reflow.
    Width(u16),
    /// `Terminal::set_scrollback_limit`.
    ScrollbackLimit(usize),
    /// Scroll the viewport back this many rows; 0 returns to the bottom.
    ScrollView(usize),
    /// Rewrite the row this many rows above the bottom of the active
    /// buffer, as an inline surface does (`Buffer::replace_row`).
    ReplaceRow(usize, String),
}

/// SGR changes between bits of text, so rows hold many style runs.
fn sgr_churn() -> impl Strategy<Value = Vec<u8>> {
    let sgr = prop::sample::select(vec![
        "\x1b[0m",
        "\x1b[1m",
        "\x1b[1;3;4m",
        "\x1b[4:3m",
        "\x1b[7m",
        "\x1b[9;2m",
        "\x1b[31m",
        "\x1b[38;5;196m",
        "\x1b[48;5;21m",
        "\x1b[38;2;1;2;3m",
        "\x1b[48;2;250;0;7m",
        "\x1b[58;5;3m",
        "\x1b[39;49m",
        "\x1b[22;23;24;27m",
    ]);
    prop::collection::vec((sgr, streams::text()), 1..6).prop_map(|parts| {
        let mut out = Vec::new();
        for (sgr, text) in parts {
            out.extend(sgr.as_bytes());
            out.extend(text);
        }
        out
    })
}

/// OSC 133 shell marks around a line of text.
fn shell_marks() -> impl Strategy<Value = Vec<u8>> {
    let mark = prop::sample::select(vec![
        "\x1b]133;A\x07",
        "\x1b]133;B\x07",
        "\x1b]133;C\x07",
        "\x1b]133;D;0\x07",
        "\x1b]133;D;1\x1b\\",
    ]);
    prop::collection::vec((mark, streams::text()), 1..5).prop_map(|parts| {
        let mut out = Vec::new();
        for (mark, text) in parts {
            out.extend(mark.as_bytes());
            out.extend(text);
            out.extend(b"\r\n");
        }
        out
    })
}

/// A scroll region, then line operations inside and around it.
fn region_ops() -> impl Strategy<Value = Vec<u8>> {
    let op = prop_oneof![
        Just(b"\n".to_vec()),
        Just(b"\x1bM".to_vec()),
        Just(b"\x1bD".to_vec()),
        (0u16..=4).prop_map(|n| format!("\x1b[{n}L").into_bytes()),
        (0u16..=4).prop_map(|n| format!("\x1b[{n}M").into_bytes()),
        (0u16..=4).prop_map(|n| format!("\x1b[{n}S").into_bytes()),
        (0u16..=4).prop_map(|n| format!("\x1b[{n}T").into_bytes()),
        (1u16..=13).prop_map(|r| format!("\x1b[{r}H").into_bytes()),
        streams::text(),
    ];
    (1u16..=13, 1u16..=13, prop::collection::vec(op, 1..12)).prop_map(|(top, bottom, ops)| {
        let mut out = format!("\x1b[{top};{bottom}r\x1b[{bottom}H").into_bytes();
        for op in ops {
            out.extend(op);
        }
        out
    })
}

/// Wide characters and clusters printed at or next to the right margin.
fn at_margin() -> impl Strategy<Value = Vec<u8>> {
    let place = prop_oneof![
        Just("\x1b[999C".to_string()),
        Just("\x1b[999C\x08".to_string()),
        (1u16..=41).prop_map(|c| format!("\x1b[{c}G")),
    ];
    let glyph = prop::sample::select(vec![
        "\u{4e2d}",
        "\u{4e2d}\u{6587}",
        "\u{1f600}",
        "e\u{301}",
        "\u{1f44d}\u{1f3fd}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        "\u{2764}\u{fe0f}",
        "\u{1f1fa}\u{1f1f8}",
        "a\u{4e2d}",
    ]);
    prop::collection::vec((place, glyph), 1..5).prop_map(|parts| {
        let mut out = Vec::new();
        for (place, glyph) in parts {
            out.extend(place.as_bytes());
            out.extend(glyph.as_bytes());
        }
        out
    })
}

/// Many lines at once, to fill scrollback and evict from it.
fn burst() -> impl Strategy<Value = Vec<u8>> {
    (streams::text(), 1usize..40).prop_map(|(line, count)| {
        let mut out = Vec::new();
        for _ in 0..count {
            out.extend(&line);
            out.extend(b"\r\n");
        }
        out
    })
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => streams::step().prop_map(Op::Stream),
        3 => sgr_churn().prop_map(|b| Op::Stream(Step::Bytes(b))),
        2 => shell_marks().prop_map(|b| Op::Stream(Step::Bytes(b))),
        3 => region_ops().prop_map(|b| Op::Stream(Step::Bytes(b))),
        2 => at_margin().prop_map(|b| Op::Stream(Step::Bytes(b))),
        2 => burst().prop_map(|b| Op::Stream(Step::Bytes(b))),
        2 => (1u16..=12).prop_map(Op::Height),
        1 => (1u16..=40).prop_map(Op::Width),
        1 => (0usize..=30).prop_map(Op::ScrollbackLimit),
        1 => (0usize..=20).prop_map(Op::ScrollView),
        1 => (0usize..=30, "[a-z \u{4e2d}]{0,12}").prop_map(|(at, text)| Op::ReplaceRow(at, text)),
    ]
}

/// What a row shows: everything storage must keep, nothing it may not.
#[derive(Debug, PartialEq)]
struct RowSnapshot {
    cells: Vec<Cell>,
    extras: Vec<(u16, Vec<char>)>,
    wrapped: bool,
    marks: String,
    anchor: String,
}

#[derive(Debug, PartialEq)]
struct BufferSnapshot {
    history_len: usize,
    max_history: usize,
    scroll_offset: usize,
    /// Rows from the top of the viewport, as `visible_row` gives them.
    visible: Vec<RowSnapshot>,
    /// Every row, oldest scrollback row first, as `row_at` gives them.
    rows: Vec<RowSnapshot>,
    /// `marks_at` and `anchor_at` for every row, which skip the decode.
    heads: Vec<(String, String)>,
    /// Live anchors with their first rows.
    anchors: Vec<(u16, String, Option<usize>)>,
}

#[derive(Debug, PartialEq)]
struct TerminalSnapshot {
    dims: Dimensions,
    alternate: bool,
    /// Position, visibility, shape, blinking and pending wrap.
    cursor: String,
    attrs: Cell,
    auto_wrap: bool,
    insert_mode: bool,
    scroll_region: Option<(u16, u16)>,
    anchor_events: String,
    main: BufferSnapshot,
    /// The alternate screen's buffer while it is shown.
    active: Option<BufferSnapshot>,
}

// The two terminals are different types with the same API, so the code that
// reads and drives them is written once, as macros.

macro_rules! row_snapshot {
    ($row:expr) => {{
        let row = $row;
        RowSnapshot {
            cells: row.cells().copied().collect(),
            extras: (0..row.width())
                .filter_map(|col| row.grapheme_extra(col).map(|extra| (col, extra.to_vec())))
                .collect(),
            wrapped: row.is_wrapped(),
            marks: format!("{:?}", row.marks()),
            anchor: format!("{:?}", row.anchor()),
        }
    }};
}

// `$own` turns a buffer's row accessor result into an owned row: live
// buffers hand out `RowView`s (`to_row`), the reference hands out
// `Cow<Row>`s (`into_owned`).
macro_rules! buffer_snapshot {
    ($buffer:expr, $own:ident) => {{
        let buffer = $buffer;
        let screen_rows = buffer.dimensions().rows;
        let total = buffer.history_len() + usize::from(screen_rows);
        BufferSnapshot {
            history_len: buffer.history_len(),
            max_history: buffer.max_history(),
            scroll_offset: buffer.scroll_offset(),
            visible: (0..screen_rows)
                .map(|r| row_snapshot!(&buffer.visible_row(r).expect("visible row").$own()))
                .collect(),
            rows: (0..total)
                .map(|i| row_snapshot!(&buffer.row_at(i).expect("row").$own()))
                .collect(),
            heads: (0..total)
                .map(|i| {
                    (
                        format!("{:?}", buffer.marks_at(i)),
                        format!("{:?}", buffer.anchor_at(i)),
                    )
                })
                .collect(),
            anchors: buffer
                .anchors()
                .map(|(tag, anchor)| (tag, format!("{anchor:?}"), buffer.anchor_line(tag)))
                .collect(),
        }
    }};
}

macro_rules! terminal_snapshot {
    ($terminal:expr, $own:ident) => {{
        let terminal = $terminal;
        let anchor_events = format!("{:?}", terminal.take_anchor_events());
        let grid = terminal.grid();
        let cursor = grid.cursor();
        TerminalSnapshot {
            dims: terminal.dimensions(),
            alternate: terminal.is_alternate_screen(),
            cursor: format!(
                "{:?} visible {} {:?} blinking {} wrap_pending {}",
                cursor.position, cursor.visible, cursor.shape, cursor.blinking, cursor.wrap_pending
            ),
            attrs: *grid.attrs(),
            auto_wrap: grid.auto_wrap(),
            insert_mode: grid.insert_mode(),
            scroll_region: grid.scroll_region().map(|r| (r.top, r.bottom)),
            anchor_events,
            main: buffer_snapshot!(terminal.main_buffer(), $own),
            active: terminal
                .is_alternate_screen()
                .then(|| buffer_snapshot!(terminal.buffer(), $own)),
        }
    }};
}

macro_rules! apply {
    ($terminal:expr, $parser:expr, $row:ty, $handler:ty, $op:expr, $spans:expr) => {{
        let terminal = $terminal;
        match $op {
            Op::Stream(Step::Bytes(bytes)) => {
                let mut handler = <$handler>::new(terminal);
                if $spans {
                    $parser.feed_with_ascii_spans(bytes, |output| match output {
                        AnsiOutput::Action(action) => handler.handle(action),
                        AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
                    });
                } else {
                    $parser.feed(bytes, |action| handler.handle(action));
                }
            }
            Op::Stream(Step::Resize(cols, rows)) => {
                terminal.resize(Dimensions::new(*cols, *rows));
            }
            Op::Height(rows) => {
                let cols = terminal.dimensions().cols;
                terminal.resize(Dimensions::new(cols, *rows));
            }
            Op::Width(cols) => {
                let rows = terminal.dimensions().rows;
                terminal.resize(Dimensions::new(*cols, rows));
            }
            Op::ScrollbackLimit(lines) => terminal.set_scrollback_limit(*lines),
            Op::ScrollView(0) => terminal.buffer_mut().scroll_to_bottom(),
            Op::ScrollView(n) => terminal.buffer_mut().scroll_up(*n),
            Op::ReplaceRow(from_end, text) => {
                let buffer = terminal.buffer_mut();
                let total = buffer.history_len() + usize::from(buffer.dimensions().rows);
                if let Some(index) = total.checked_sub(from_end + 1) {
                    let mut row = <$row>::new(buffer.dimensions().cols);
                    for (col, c) in (0..).zip(text.chars()) {
                        if let Some(cell) = row.get_mut(col) {
                            *cell = Cell::new(c);
                        }
                    }
                    buffer.replace_row(index, &row);
                }
            }
        }
    }};
}

/// Run `ops` on both terminals; the first step after which they differ.
fn run(dims: Dimensions, scrollback: usize, ops: &[Op], spans: bool) -> Result<(), String> {
    let mut live = live::Terminal::new(dims);
    let mut reference = super::Terminal::new(dims);
    let (mut live_parser, mut reference_parser) = (AnsiParser::new(), AnsiParser::new());
    live.set_inline_anchors(true);
    reference.set_inline_anchors(true);
    live.set_scrollback_limit(scrollback);
    reference.set_scrollback_limit(scrollback);
    for (index, op) in ops.iter().enumerate() {
        apply!(
            &mut live,
            live_parser,
            live::Row,
            live::TerminalHandler,
            op,
            spans
        );
        apply!(
            &mut reference,
            reference_parser,
            super::Row,
            super::TerminalHandler,
            op,
            spans
        );
        let (got, want) = (
            terminal_snapshot!(&mut live, to_row),
            terminal_snapshot!(&mut reference, into_owned),
        );
        if got != want {
            return Err(format!(
                "after step {index} ({op:?}): {}",
                first_difference(&got, &want)
            ));
        }
    }
    Ok(())
}

/// The first part of two snapshots that differs, live then reference.
fn first_difference(live: &TerminalSnapshot, reference: &TerminalSnapshot) -> String {
    fn buffers(name: &str, live: &BufferSnapshot, reference: &BufferSnapshot) -> Option<String> {
        if live == reference {
            return None;
        }
        let rows = |what: &str, a: &[RowSnapshot], b: &[RowSnapshot]| {
            a.iter()
                .zip(b)
                .position(|(a, b)| a != b)
                .map(|i| format!("{name} {what} row {i}:\n live {:?}\n  ref {:?}", a[i], b[i]))
        };
        rows("visible", &live.visible, &reference.visible)
            .or_else(|| rows("absolute", &live.rows, &reference.rows))
            .or_else(|| Some(format!("{name}:\n live {live:?}\n  ref {reference:?}")))
    }
    buffers("main", &live.main, &reference.main)
        .or_else(|| match (&live.active, &reference.active) {
            (Some(a), Some(b)) => buffers("alternate", a, b),
            _ => None,
        })
        .unwrap_or_else(|| {
            let strip = |s: &TerminalSnapshot| {
                format!(
                    "{:?} alternate {} cursor {} attrs {:?} wrap {} insert {} region {:?} events {}",
                    s.dims,
                    s.alternate,
                    s.cursor,
                    s.attrs,
                    s.auto_wrap,
                    s.insert_mode,
                    s.scroll_region,
                    s.anchor_events
                )
            };
            format!("terminal:\n live {}\n  ref {}", strip(live), strip(reference))
        })
}

fn cases() -> u32 {
    std::env::var("ULTTY_ORACLE_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(1024)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn live_storage_matches_reference(
        cols in 1u16..=40,
        rows in 1u16..=12,
        scrollback in 0usize..=40,
        ops in prop::collection::vec(op(), 1..64),
    ) {
        for spans in [false, true] {
            if let Err(difference) = run(Dimensions::new(cols, rows), scrollback, &ops, spans) {
                prop_assert!(false, "spans {}: {}", spans, difference);
            }
        }
    }
}
