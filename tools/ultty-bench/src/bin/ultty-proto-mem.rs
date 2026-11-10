//! Live heap of the ADR 0022 prototype on the memory bench's budget
//! workloads (feature `proto`).
//!
//! The counting allocator and the workloads (`typical_lines`,
//! `sgr_dense_lines`) are copied from `crates/ultty/benches/memory.rs`, so
//! the reference arm here should land near that bench's own numbers; it
//! measures `Terminal` alone, without the bench's scene-tree cases.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

use ultty_bench::proto::{filled, Arm};

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

/// Live and peak heap bytes of `arm` after `bytes`, net of what was live
/// before (the input itself).
fn measure(arm: Arm, cols: u16, rows: u16, bytes: &[u8]) -> (usize, usize) {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let target = filled(arm, cols, rows, bytes);
    let live = LIVE.load(Ordering::Relaxed).saturating_sub(base);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    drop(black_box(target));
    (live, peak)
}

/// As in `memory.rs`.
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

/// As in `memory.rs`.
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

fn main() {
    // (case, budget from memory.rs, cols, rows, input)
    let cases: [(&str, usize, u16, u16, Vec<u8>); 4] = [
        ("empty_terminal_300x75", 1_229_000, 300, 75, Vec::new()),
        (
            "typical_10k_lines_120x40",
            3_461_000,
            120,
            40,
            typical_lines(10_040),
        ),
        (
            "typical_10k_lines_300x75",
            4_358_000,
            300,
            75,
            typical_lines(10_075),
        ),
        (
            "sgr_dense_10k_lines_120x40",
            18_980_000,
            120,
            40,
            sgr_dense_lines(10_040, 120),
        ),
    ];
    println!(
        "{:<28} {:>4} {:>12} {:>12} {:>12} {:>7}",
        "case", "arm", "live bytes", "peak bytes", "budget", "of ref"
    );
    for (name, budget, cols, rows, input) in &cases {
        let (reference, _) = measure(Arm::Reference, *cols, *rows, input);
        for arm in Arm::ALL {
            let (live, peak) = measure(arm, *cols, *rows, input);
            println!(
                "{name:<28} {:>4} {live:>12} {peak:>12} {budget:>12} {:>6.0}%",
                arm.name(),
                live as f64 / reference as f64 * 100.0
            );
        }
    }
}
