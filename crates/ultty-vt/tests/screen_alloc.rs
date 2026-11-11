//! Moving screen rows allocates nothing (ADR 0022 §2).
//!
//! Scrolling, scroll regions, IL and DL rotate the screen page's slots and
//! clear the rows that come in; none of them may touch the heap. A counting
//! global allocator, as in `crates/ultty/benches/memory.rs`, counts the
//! allocations made on this thread while the operations run.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::{Buffer, Dimensions, Grid, Terminal, TerminalHandler};

struct Counting;

thread_local! {
    /// Allocations (and reallocations) made on this thread while counting.
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn note_allocation() {
    // `try_with`: the slot may be gone while the thread shuts down.
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

// SAFETY: every call forwards to `System` with the same arguments; the
// counter is side bookkeeping and never affects the returned memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: forwarded unchanged; the caller upholds the contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: forwarded unchanged; the caller upholds `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// How many allocations `f` makes on this thread.
fn allocations(f: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|count| count.set(Some(0)));
    f();
    ALLOCATIONS.with(|count| count.replace(None)).unwrap_or(0)
}

/// Feed `bytes` to `terminal` as the PTY path does.
fn feed(terminal: &mut Terminal, bytes: &[u8]) {
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(terminal);
    parser.feed_with_ascii_spans(bytes, |output| match output {
        AnsiOutput::Action(action) => handler.handle(action),
        AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
    });
}

/// A screen with something on every row: plain and styled text, wide
/// characters, combining marks and a ZWJ sequence, and a shell mark.
fn busy_terminal() -> Terminal {
    let mut terminal = Terminal::new(Dimensions::new(40, 12));
    feed(&mut terminal, b"\x1b]133;A\x07");
    for line in 0..12 {
        let text = format!(
            "{line:02} plain \x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m \u{4e2d}\u{6587} e\u{301} \u{1F469}\u{200D}\u{1F4BB}"
        );
        feed(&mut terminal, text.as_bytes());
        if line < 11 {
            feed(&mut terminal, b"\r\n");
        }
    }
    terminal
}

fn assert_no_allocations(what: &str, f: impl FnOnce()) {
    let n = allocations(f);
    assert_eq!(n, 0, "{what} allocated {n} times");
}

#[test]
fn insert_and_delete_lines_allocate_nothing() {
    let mut terminal = busy_terminal();
    let grid: &mut Grid = terminal.grid_mut();
    for row in [1, 5, 11] {
        grid.set_cursor_position(row, 1);
        assert_no_allocations("IL", || grid.insert_lines(3));
        assert_no_allocations("DL", || grid.delete_lines(2));
        assert_no_allocations("IL past the bottom", || grid.insert_lines(40));
    }
    grid.set_scroll_region(3, 9);
    grid.set_cursor_position(4, 1);
    assert_no_allocations("IL in a region", || grid.insert_lines(2));
    assert_no_allocations("DL in a region", || grid.delete_lines(3));
    grid.check_integrity().unwrap();
}

#[test]
fn scrolling_the_screen_or_a_region_allocates_nothing() {
    let mut terminal = busy_terminal();
    let grid: &mut Grid = terminal.grid_mut();
    assert_no_allocations("scroll up", || grid.scroll_up(1));
    assert_no_allocations("scroll up 5", || grid.scroll_up(5));
    assert_no_allocations("scroll down", || grid.scroll_down(3));
    assert_no_allocations("scroll everything off", || grid.scroll_up(40));
    let mut terminal = busy_terminal();
    let grid: &mut Grid = terminal.grid_mut();
    grid.set_scroll_region(2, 10);
    assert_no_allocations("region up", || grid.scroll_up(2));
    assert_no_allocations("region down", || grid.scroll_down(4));
    grid.check_integrity().unwrap();
}

#[test]
fn line_feeds_in_a_region_below_the_top_allocate_nothing() {
    // A region that does not start at the top keeps no history, so a line
    // feed on its bottom margin is only the screen's rotation.
    let mut terminal = busy_terminal();
    feed(&mut terminal, b"\x1b[3;10r\x1b[10;1H");
    let buffer: &mut Buffer = terminal.buffer_mut();
    assert_no_allocations("line feeds", || {
        for _ in 0..30 {
            buffer.newline();
        }
    });
}
