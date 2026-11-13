//! Heap and RSS used by the terminal model, measured, not estimated.
//!
//! A counting global allocator records the live heap bytes each workload
//! leaves behind (and its peak), and `/proc/self/statm` gives the RSS delta.
//! Allocation counts do not depend on machine load, so the heap figures are
//! a deterministic gate: each case fails when it exceeds its budget, which is
//! the bytes measured when the budget was set plus 5% (the terminal cases on
//! 2026-11-11, after ADR 0022's page storage). When a change moves a number
//! on purpose, update the budget in the same commit and say why.
//!
//! cargo bench -p ultty --bench memory   (release profile, LTO: slow to build)
//! cargo test -p ultty --bench memory    (same allocations, builds faster)
//!
//! The `agent_session_10h` case is the deterministic stand-in for the D5
//! 10-hour wall-clock soak (`scripts/bench/soak.sh`): it feeds two tabs ten
//! simulated hours of agent-shaped output at full speed (about 25 s under
//! `cargo test`) and fails when live heap grows after the first hour.
//! `ULTTY_SOAK_SIM_HOURS=N` changes the length; the budgets assume 10.
//!
//! Exits non-zero when any case is over budget. A case can also budget its
//! peak heap, for work whose transient allocations would stay resident (the
//! allocator rarely returns them). RSS is printed for context only: it
//! depends on the allocator's page reuse and is not gated.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use ultty::grid::{Dimensions, Terminal, TerminalHandler};
use ultty::protocol::ansi::{AnsiOutput, AnsiParser};
use ultty::scene::{Region, SceneTree};
use ultty_protocol::{
    Constraints, Content, Display, RegionId, Role, Semantics, SetPosition, TextContent,
};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn record_alloc(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every call forwards to `System` with the same arguments; the
// counters are side bookkeeping and never affect the returned memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds the contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator with this `layout`.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `realloc`'s contract.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            record_alloc(new_size);
        }
        new
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn rss_bytes() -> Option<usize> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: usize = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * 4096)
}

struct Measurement {
    live: usize,
    peak: usize,
    rss_delta: Option<isize>,
    /// Why the case failed for a reason other than a byte budget (growth
    /// over time, for the soak case).
    failure: Option<String>,
}

/// Run `build`, keep its result alive, and report what it holds.
fn measure<T>(build: impl FnOnce() -> T) -> Measurement {
    let rss_before = rss_bytes();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let value = build();
    let live = LIVE.load(Ordering::Relaxed).saturating_sub(base);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    let rss_delta = rss_before
        .zip(rss_bytes())
        .map(|(before, after)| after as isize - before as isize);
    drop(black_box(value));
    Measurement {
        live,
        peak,
        rss_delta,
        failure: None,
    }
}

fn feed(terminal: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    for chunk in bytes.chunks(64 * 1024) {
        let mut handler = TerminalHandler::new(terminal);
        parser.feed_with_ascii_spans(chunk, |out| match out {
            AnsiOutput::Action(action) => handler.handle(action),
            AnsiOutput::PrintAscii(span) => handler.handle_ascii_span(span),
        });
    }
}

/// 84-character log lines, every third one green, CRLF-terminated as a PTY
/// with ONLCR delivers them.
fn typical_lines(count: usize) -> Vec<u8> {
    let line =
        b"the quick brown fox jumps over the lazy dog 0123456789 ABCDEF green 0123456789 end";
    let mut out = Vec::with_capacity(count * (line.len() + 12));
    for i in 0..count {
        if i % 3 == 0 {
            out.extend_from_slice(b"\x1b[32m");
        }
        out.extend_from_slice(line);
        if i % 3 == 0 {
            out.extend_from_slice(b"\x1b[0m");
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Lines that change SGR every few cells: the worst case for run-length
/// encoded scrollback.
fn sgr_dense_lines(count: usize, cols: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..count {
        for c in 0..cols / 3 {
            let color = (i * 7 + c * 13) % 256;
            out.extend_from_slice(format!("\x1b[38;5;{color}m").as_bytes());
            out.extend_from_slice(b"ab ");
        }
        out.extend_from_slice(b"\x1b[0m\r\n");
    }
    out
}

/// Lines of CJK words and the spaces between them, about 40 wide
/// characters (80 columns) each, as the `vt` bench's cjk corpus prints them;
/// every third one green. Scrollback stores wide characters at more bytes a
/// cell than ASCII (ADR 0022, Risks), which no other case covers.
fn cjk_lines(count: usize) -> Vec<u8> {
    const WORDS: [&str; 12] = [
        "日本語",
        "中文字符",
        "한국어",
        "東京都",
        "ひらがな",
        "カタカナ",
        "漢字",
        "終端",
        "文字化け",
        "表示",
        "速度",
        "测试",
    ];
    let mut out = Vec::new();
    for i in 0..count {
        if i % 3 == 0 {
            out.extend_from_slice(b"\x1b[32m");
        }
        let (mut chars, mut word) = (0, i);
        while chars < 27 {
            let text = WORDS[word % WORDS.len()];
            out.extend_from_slice(text.as_bytes());
            out.push(b' ');
            chars += text.chars().count() + 1;
            word += 7;
        }
        if i % 3 == 0 {
            out.extend_from_slice(b"\x1b[0m");
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn scene_tree(regions: usize) -> SceneTree {
    let mut tree = SceneTree::new();
    for i in 1..=regions {
        let mut region = Region::new(RegionId::new(i as u32));
        region.parent = Some(RegionId::ROOT);
        region.constraints = Constraints {
            display: Display::Block,
            ..Default::default()
        };
        tree.insert(region);
    }
    tree
}

/// A table of `rows` rows of `columns` one-word cells, as ulttui's `Table`
/// draws it, with the semantics it sends (ADR 0021) when `semantic`.
fn scene_table(rows: u32, columns: u32, semantic: bool) -> SceneTree {
    let mut tree = SceneTree::new();
    let mut next = 1;
    let mut add = |tree: &mut SceneTree, parent, content, semantics: Semantics| {
        let id = RegionId::new(next);
        next += 1;
        let mut region = Region::new(id);
        region.parent = Some(parent);
        region.content = content;
        if semantic {
            region.set_semantics(semantics);
        }
        tree.insert(region);
        id
    };
    let table = add(
        &mut tree,
        RegionId::ROOT,
        Content::None,
        Semantics {
            role: Role::Table,
            ..Semantics::default()
        },
    );
    for row in 1..=rows {
        let row_id = add(
            &mut tree,
            table,
            Content::None,
            Semantics {
                role: Role::Row,
                position: Some(SetPosition {
                    index: row,
                    size: rows,
                }),
                ..Semantics::default()
            },
        );
        for column in 1..=columns {
            add(
                &mut tree,
                row_id,
                Content::Text(TextContent::new(format!("r{row}c{column}"))),
                Semantics {
                    role: Role::Cell,
                    position: Some(SetPosition {
                        index: column,
                        size: columns,
                    }),
                    ..Semantics::default()
                },
            );
        }
    }
    tree
}

/// Splitmix64: a tiny deterministic generator, so the soak needs no crate.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next_u64() % (hi - lo + 1) as u64) as usize
    }

    fn word(&mut self) -> &'static str {
        AGENT_WORDS[self.range(0, AGENT_WORDS.len() - 1)]
    }
}

/// The word list of `scripts/bench/agent-stream.py`.
const AGENT_WORDS: &[&str] = &[
    "the",
    "terminal",
    "renders",
    "each",
    "region",
    "from",
    "its",
    "constraints",
    "so",
    "the",
    "layout",
    "stays",
    "stable",
    "while",
    "text",
    "streams",
    "in",
    "and",
    "the",
    "scroll",
    "position",
    "holds",
    "tests",
    "pass",
    "build",
    "fails",
    "retry",
    "with",
    "the",
    "fixed",
    "path",
    "parser",
    "grid",
    "buffer",
    "reflow",
    "scrollback",
    "cursor",
    "glyph",
    "atlas",
    "frame",
    "event",
    "socket",
    "pane",
    "tab",
];

/// Milliseconds between tokens (50 a second) and between status box redraws.
const TOKEN_MS: u64 = 20;
const BOX_MS: u64 = 80;
const BOX_LINES: usize = 3;

/// `scripts/bench/agent-stream.py`, driven by simulated time. One `tick`
/// is one token period; the mapping from the Python:
///
/// - `styled_token`: the same 5% bold, 5% inline code, 2% OSC 8 link mix;
/// - `Screen.write` / `redraw_box`: the same synchronized-output wrapper,
///   `ESC [ J` to erase the box, IND x3 + CUU to reserve it, DECSC / DECRC
///   around it; the box redraws once 80 ms have passed, as the Python
///   checks `time.monotonic()` after each token;
/// - a reply is 80 to 600 tokens after a heading, wrapped near the right
///   edge (30% of wraps start a list item); it ends with a tool dump of 200
///   to 3000 lines 30% of the time and a `/clear` of screen and scrollback
///   3% of the time;
/// - the box is as wide as the terminal is when it is drawn (the Python
///   fixes `--cols`; a resized window changes it here), and its elapsed
///   seconds are simulated seconds.
///
/// Same shape and rates, not the same bytes: the random draws differ. The
/// generator is deterministic for a seed.
struct AgentStream {
    rng: Rng,
    replies: u64,
    tokens_left: usize,
    column: usize,
    box_tick: usize,
    next_box_ms: u64,
}

impl AgentStream {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed),
            replies: 0,
            tokens_left: 0,
            column: 0,
            box_tick: 0,
            next_box_ms: 0,
        }
    }

    fn styled_token(&mut self) -> String {
        let word = self.rng.word();
        let r = self.rng.unit();
        if r < 0.05 {
            format!("\x1b[1m{word}\x1b[22m")
        } else if r < 0.10 {
            format!("\x1b[38;2;230;160;80m`{word}`\x1b[39m")
        } else if r < 0.12 {
            format!("\x1b]8;;https://example.com/{word}\x1b\\{word}\x1b]8;;\x1b\\")
        } else {
            word.to_string()
        }
    }

    fn draw_box(&self, out: &mut Vec<u8>, now_ms: u64, cols: usize) {
        const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
        let inner = cols.saturating_sub(4);
        let mut label = format!(
            " {} Working... ({}s, esc to interrupt)",
            SPINNER[self.box_tick % 4],
            now_ms / 1000
        );
        label.truncate(inner);
        let rule = "-".repeat(cols.saturating_sub(2));
        let reserve = "\x1bD".repeat(BOX_LINES);
        let text = format!(
            "{reserve}\x1b[{BOX_LINES}A\x1b7\r\n\x1b[38;5;244m+{rule}+\x1b[39m\r\n\
             \x1b[38;5;244m|\x1b[39m {label:<inner$} \x1b[38;5;244m|\x1b[39m\r\n\
             \x1b[38;5;244m+{rule}+\x1b[39m\x1b8\x1b[?2026l"
        );
        out.extend_from_slice(text.as_bytes());
    }

    /// `Screen.write`: erase the box, write `text`, draw the box again.
    fn write(&self, out: &mut Vec<u8>, text: &str, now_ms: u64, cols: usize) {
        out.extend_from_slice(b"\x1b[?2026h\x1b[J");
        out.extend_from_slice(text.as_bytes());
        self.draw_box(out, now_ms, cols);
    }

    /// Append the bytes the agent writes in the token period at `now_ms`.
    fn tick(&mut self, out: &mut Vec<u8>, now_ms: u64, cols: usize) {
        if self.tokens_left == 0 {
            self.replies += 1;
            let heading = format!("\r\n\x1b[1;38;5;75m## Step {}\x1b[0m\r\n\r\n", self.replies);
            self.write(out, &heading, now_ms, cols);
            self.column = 0;
            self.tokens_left = self.rng.range(80, 600);
        }
        let mut token = self.styled_token();
        if self.column + 12 > cols {
            let bullet = if self.rng.unit() < 0.3 { "- " } else { "" };
            token = format!("\r\n{bullet}{token}");
            self.column = 0;
        } else {
            token.insert(0, ' ');
        }
        self.column += 8;
        self.write(out, &token, now_ms, cols);
        if now_ms >= self.next_box_ms {
            self.box_tick += 1;
            out.extend_from_slice(b"\x1b[?2026h\x1b[J");
            self.draw_box(out, now_ms, cols);
            self.next_box_ms = now_ms + BOX_MS;
        }
        self.tokens_left -= 1;
        if self.tokens_left > 0 {
            return;
        }
        self.write(out, "\r\n", now_ms, cols);
        let r = self.rng.unit();
        if r < 0.3 {
            let lines = self.rng.range(200, 3000);
            let mut body = String::from("\x1b[2m```\x1b[22m\r\n");
            for n in 0..lines {
                body.push_str(&format!("\x1b[38;5;108m{n:5}\x1b[39m  "));
                for k in 0..8 {
                    if k > 0 {
                        body.push(' ');
                    }
                    body.push_str(self.rng.word());
                }
                body.push_str("\r\n");
            }
            body.push_str("\x1b[2m```\x1b[22m\r\n");
            self.write(out, &body, now_ms, cols);
        } else if r < 0.33 {
            self.write(out, "\x1b[H\x1b[2J\x1b[3J", now_ms, cols);
        }
    }
}

/// A tab's terminal with the parser that feeds it, kept across writes.
struct Tab {
    terminal: Terminal,
    parser: AnsiParser,
    stream: AgentStream,
}

impl Tab {
    fn new(seed: u64, dims: Dimensions) -> Self {
        Self {
            terminal: Terminal::new(dims),
            parser: AnsiParser::new(),
            stream: AgentStream::new(seed),
        }
    }

    fn step(&mut self, out: &mut Vec<u8>, now_ms: u64) {
        out.clear();
        let cols = usize::from(self.terminal.dimensions().cols);
        self.stream.tick(out, now_ms, cols);
        for chunk in out.chunks(64 * 1024) {
            let mut handler = TerminalHandler::new(&mut self.terminal);
            self.parser
                .feed_with_ascii_spans(chunk, |output| match output {
                    AnsiOutput::Action(action) => handler.handle(action),
                    AnsiOutput::PrintAscii(span) => handler.handle_ascii_span(span),
                });
        }
    }
}

/// The soak's window sizes (1600x1000, 1200x800, 1920x1150, 1000x700
/// pixels, cycled every five minutes) as cell grids: pixels over an 8x16
/// cell, rounded down.
const SOAK_GRIDS: [(u16, u16); 4] = [(200, 62), (150, 50), (240, 71), (125, 43)];
const RESIZE_EVERY_MS: u64 = 5 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;
const SAMPLE_MS: u64 = 60 * 1000;

/// Live heap of one simulated hour, sampled every simulated minute.
struct HourStats {
    /// Smallest and largest sample: the floor just after a `/clear` and the
    /// ceiling with the scrollback full.
    min: usize,
    max: usize,
}

/// Simulated hours: 10 unless `ULTTY_SOAK_SIM_HOURS` says otherwise. Ten
/// hours take about 25 s under `cargo test` and less under `cargo bench`.
fn soak_hours() -> u64 {
    std::env::var("ULTTY_SOAK_SIM_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
}

/// How far the last hour's ceiling or floor may sit above hour 1's (the
/// warm-up) and still pass. Measured 2026-11-12 over ten hours the ceiling
/// stayed within 10.57 to 11.06 MB (margin: 5% of it) and the floor, which
/// depends on how much a `/clear` left, between 0.9 and 2.2 MB. A leak that
/// grows with work (a few bytes per token, a page per resize) adds more
/// than that over ten hours.
const CEILING_MARGIN: usize = 500_000;
const FLOOR_MARGIN: usize = 1_000_000;

/// Why the hourly series (hour 1 onward) shows growth, if it does.
///
/// The end-of-hour figure alone is not a usable signal: a `/clear` drops
/// the scrollback, so it swings by several MB with how long ago the last
/// one was. Growth shows in the ceiling (scrollback full) and the floor
/// (just after a clear), so both are held to hour 1's value plus a margin,
/// and neither may rise every hour.
fn soak_growth(hours: &[HourStats]) -> Option<String> {
    let first = hours.first()?;
    let last = hours.last()?;
    for (name, margin, pick) in [
        (
            "ceiling",
            CEILING_MARGIN,
            (|h: &HourStats| h.max) as fn(&HourStats) -> usize,
        ),
        ("floor", FLOOR_MARGIN, |h: &HourStats| h.min),
    ] {
        let (a, b) = (pick(first), pick(last));
        if b > a + margin {
            return Some(format!(
                "live heap {name} grew from {a} to {b} bytes (margin {margin})"
            ));
        }
        if hours.len() >= 4 && hours.windows(2).all(|w| pick(&w[1]) > pick(&w[0])) {
            return Some(format!("live heap {name} rose every hour"));
        }
    }
    None
}

/// Two tabs fed `hours` simulated hours of agent output, resized every five
/// minutes, live heap sampled every simulated minute and reported per hour.
/// Hour 1 is warm-up: the scrollback is full by then.
fn agent_session(hours: u64) -> Measurement {
    let rss_before = rss_bytes();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let (cols, rows) = SOAK_GRIDS[0];
    let mut tabs = [
        Tab::new(1, Dimensions::new(cols, rows)),
        Tab::new(2, Dimensions::new(cols, rows)),
    ];
    let mut out = Vec::new();
    let mut series = Vec::new();
    let (mut min, mut max) = (usize::MAX, 0);
    let mut resizes = 0;
    let mut now_ms = 0;
    println!("  hour   end of hour   floor (min)   ceiling (max)");
    while now_ms < hours * HOUR_MS {
        if now_ms > 0 && now_ms % RESIZE_EVERY_MS == 0 {
            resizes += 1;
            let (cols, rows) = SOAK_GRIDS[resizes % SOAK_GRIDS.len()];
            for tab in &mut tabs {
                tab.terminal.resize(Dimensions::new(cols, rows));
            }
        }
        for tab in &mut tabs {
            tab.step(&mut out, now_ms);
        }
        now_ms += TOKEN_MS;
        if now_ms % SAMPLE_MS == 0 {
            let live = LIVE.load(Ordering::Relaxed).saturating_sub(base);
            (min, max) = (min.min(live), max.max(live));
            if now_ms % HOUR_MS == 0 {
                println!("  {:>4} {live:>13} {min:>13} {max:>15}", now_ms / HOUR_MS);
                series.push(HourStats { min, max });
                (min, max) = (usize::MAX, 0);
            }
        }
    }
    // The end of the run is a point on the `/clear` sawtooth, so the figure
    // gated against the budget is the highest sample, not the last.
    let live = series.iter().map(|h| h.max).max().unwrap_or(0);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    let rss_delta = rss_before
        .zip(rss_bytes())
        .map(|(before, after)| after as isize - before as isize);
    drop(black_box(tabs));
    Measurement {
        live,
        peak,
        rss_delta,
        failure: soak_growth(&series),
    }
}

struct Case {
    name: &'static str,
    /// Live heap bytes allowed after the workload (measured + 5%).
    budget: usize,
    /// Peak heap bytes allowed during it, where that is gated.
    peak_budget: Option<usize>,
    run: fn() -> Measurement,
}

fn filled_terminal(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    feed(&mut terminal, bytes);
    terminal
}

const CASES: &[Case] = &[
    Case {
        name: "empty_terminal_80x24",
        budget: 38_800,
        peak_budget: None,
        run: || measure(|| Terminal::new(Dimensions::new(80, 24))),
    },
    Case {
        name: "empty_terminal_300x75",
        budget: 394_000,
        peak_budget: None,
        run: || measure(|| Terminal::new(Dimensions::new(300, 75))),
    },
    Case {
        name: "typical_10k_lines_80x24",
        budget: 767_800,
        peak_budget: None,
        run: || {
            let input = typical_lines(10_024);
            measure(|| filled_terminal(80, 24, &input))
        },
    },
    Case {
        name: "typical_10k_lines_120x40",
        budget: 1_223_400,
        peak_budget: None,
        run: || {
            let input = typical_lines(10_040);
            measure(|| filled_terminal(120, 40, &input))
        },
    },
    Case {
        name: "typical_10k_lines_300x75",
        budget: 1_527_000,
        peak_budget: None,
        run: || {
            let input = typical_lines(10_075);
            measure(|| filled_terminal(300, 75, &input))
        },
    },
    Case {
        // Resizing reflows the scrollback; it must stay as compact as it
        // was (found by the 2026-11-08 soak: RSS grew with each resize).
        name: "typical_10k_lines_120x40_resized",
        budget: 1_221_300,
        // Measured 1_448_974 + 5%. It was 149 MB when reflow decoded the
        // whole scrollback at once, and 4.7 MB before history pages.
        peak_budget: Some(1_521_500),
        run: || {
            let input = typical_lines(10_040);
            measure(|| {
                let mut terminal = filled_terminal(120, 40, &input);
                for (cols, rows) in [(100, 30), (140, 45), (90, 35), (120, 40)] {
                    terminal.resize(Dimensions::new(cols, rows));
                }
                terminal
            })
        },
    },
    Case {
        name: "sgr_dense_10k_lines_120x40",
        budget: 10_457_200,
        peak_budget: None,
        run: || {
            let input = sgr_dense_lines(10_040, 120);
            measure(|| filled_terminal(120, 40, &input))
        },
    },
    Case {
        name: "cjk_10k_lines_120x40",
        // 4_594_320 with `CompactRow` scrollback; history pages' Chars form
        // holds it in about half that.
        budget: 2_440_200,
        peak_budget: None,
        run: || {
            let input = cjk_lines(10_040);
            measure(|| filled_terminal(120, 40, &input))
        },
    },
    Case {
        name: "scene_tree_1000_regions",
        budget: 953_000,
        peak_budget: None,
        run: || measure(|| scene_tree(1000)),
    },
    Case {
        // The table below without semantics, to price them.
        name: "scene_table_1000x4_plain",
        // Measured 3_730_192 on 2026-11-10 + 5%.
        budget: 3_917_000,
        peak_budget: None,
        run: || measure(|| scene_table(1000, 4, false)),
    },
    Case {
        name: "scene_table_1000x4_semantic",
        // Measured 4_410_328 on 2026-11-10 + 5%: 680 KB more than plain
        // for 5001 regions, about 136 bytes each.
        budget: 4_631_000,
        peak_budget: None,
        run: || measure(|| scene_table(1000, 4, true)),
    },
    Case {
        // Ten simulated hours of two agent tabs (`agent_session`). Live is
        // the highest per-minute sample; the growth check inside is the
        // real gate. Both measured 2026-11-12 + 5%: live 11_058_844, peak
        // 11_322_890. They hold for the default 10 hours, not for
        // `ULTTY_SOAK_SIM_HOURS` overrides.
        name: "agent_session_10h",
        budget: 11_611_800,
        peak_budget: Some(11_889_200),
        run: || agent_session(soak_hours()),
    },
];

fn main() -> ExitCode {
    // `cargo bench` passes `--bench`; a filter argument selects cases.
    let filter = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or_default();
    println!(
        "{:<30} {:>12} {:>12} {:>12} {:>12}  status",
        "case", "live bytes", "budget", "peak bytes", "rss delta"
    );
    let mut over = 0;
    for case in CASES.iter().filter(|case| case.name.contains(&filter)) {
        let m = (case.run)();
        let ok = m.live <= case.budget
            && case.peak_budget.is_none_or(|peak| m.peak <= peak)
            && m.failure.is_none();
        if let Some(why) = &m.failure {
            eprintln!("{}: {why}", case.name);
        }
        if !ok {
            over += 1;
        }
        let rss = m
            .rss_delta
            .map_or_else(|| "n/a".to_string(), |d| d.to_string());
        println!(
            "{:<30} {:>12} {:>12} {:>12} {:>12}  {}",
            case.name,
            m.live,
            case.budget,
            m.peak,
            rss,
            if ok { "ok" } else { "OVER BUDGET" }
        );
    }
    if over > 0 {
        eprintln!("{over} memory case(s) over budget");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
