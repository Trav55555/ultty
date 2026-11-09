//! Heap and RSS used by the terminal model, measured, not estimated.
//!
//! A counting global allocator records the live heap bytes each workload
//! leaves behind (and its peak), and `/proc/self/statm` gives the RSS delta.
//! Allocation counts do not depend on machine load, so the heap figures are
//! a deterministic gate: each case fails when it exceeds its budget, which is
//! the bytes measured on 2026-11-07 plus 5%. When a change moves a number on
//! purpose, update the budget in the same commit and say why.
//!
//! cargo bench -p ultty --bench memory   (release profile, LTO: slow to build)
//! cargo test -p ultty --bench memory    (same allocations, builds faster)
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
use ultty_protocol::{Constraints, Display, RegionId};

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
        budget: 185_000,
        peak_budget: None,
        run: || measure(|| Terminal::new(Dimensions::new(80, 24))),
    },
    Case {
        name: "empty_terminal_300x75",
        budget: 1_229_000,
        peak_budget: None,
        run: || measure(|| Terminal::new(Dimensions::new(300, 75))),
    },
    Case {
        name: "typical_10k_lines_80x24",
        budget: 2_915_000,
        peak_budget: None,
        run: || {
            let input = typical_lines(10_024);
            measure(|| filled_terminal(80, 24, &input))
        },
    },
    Case {
        name: "typical_10k_lines_120x40",
        budget: 3_461_000,
        peak_budget: None,
        run: || {
            let input = typical_lines(10_040);
            measure(|| filled_terminal(120, 40, &input))
        },
    },
    Case {
        name: "typical_10k_lines_300x75",
        budget: 4_358_000,
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
        budget: 3_461_000,
        // Measured 4_699_129 (it was 149 MB when reflow decoded the whole
        // scrollback at once) + 5%.
        peak_budget: Some(4_935_000),
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
        budget: 18_980_000,
        peak_budget: None,
        run: || {
            let input = sgr_dense_lines(10_040, 120);
            measure(|| filled_terminal(120, 40, &input))
        },
    },
    Case {
        name: "scene_tree_1000_regions",
        budget: 953_000,
        peak_budget: None,
        run: || measure(|| scene_tree(1000)),
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
        let ok = m.live <= case.budget && case.peak_budget.is_none_or(|peak| m.peak <= peak);
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
