//! Live Rust heap, counted, for soaks (feature `heap-stats`).
//!
//! Process memory (anonymous RSS) mixes Ultty's own heap with the GPU
//! driver's allocations and the allocator's retained pages, so a step in
//! it cannot say whose memory grew. With this feature the `ultty` binary
//! installs [`Counting`] as its global allocator, and the render loop logs
//! the live and peak bytes of Rust allocations every [`LOG_INTERVAL`]
//! (`ultty::perf` target, `heap_stats`). C libraries (the Vulkan driver,
//! fontconfig) allocate through malloc directly and are not counted.
//!
//! Off by default: counting costs two atomic operations per allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How often the render loop logs the counters.
pub const LOG_INTERVAL: Duration = Duration::from_secs(10);

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, with live and peak bytes counted.
pub struct Counting;

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

/// Live and peak bytes of Rust heap, as counted so far.
pub fn snapshot() -> (usize, usize) {
    (LIVE.load(Ordering::Relaxed), PEAK.load(Ordering::Relaxed))
}

/// When the counters were last logged; `None` before the first log.
static LAST_LOG: Mutex<Option<Instant>> = Mutex::new(None);

/// Logs the counters if [`LOG_INTERVAL`] has passed since the last log.
/// Called from the render loop, so it logs only while frames are drawn.
pub fn maybe_log() {
    let mut last = LAST_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if last.is_some_and(|at| at.elapsed() < LOG_INTERVAL) {
        return;
    }
    *last = Some(Instant::now());
    let (live, peak) = snapshot();
    tracing::info!(
        target: "ultty::perf",
        heap_live_bytes = live,
        heap_peak_bytes = peak,
        "heap_stats"
    );
}
