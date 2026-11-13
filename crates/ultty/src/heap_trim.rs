//! Give freed heap back to the OS after Ultty frees a lot at once.
//!
//! glibc's malloc returns memory to the OS only from the top of each
//! arena. Scrollback is the bulk of Ultty's heap, and when it goes (ED 3
//! `/clear`, a reflow on resize, a closed pane), the freed blocks usually
//! sit below something still live, so the pages stay resident and RSS
//! keeps the highest scrollback ever reached: on the 2026-11-12 soak the
//! main heap held 23.5 MiB with 12 MiB of Rust heap live. `malloc_trim`
//! releases free pages anywhere in every arena.
//!
//! Callers [`request`] a trim where they free a lot; the main loop calls
//! [`maybe_trim`] after each frame, which trims at most once per
//! [`MIN_INTERVAL`]. Elsewhere than glibc it does nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Shortest time between two trims: trimming walks every arena, and the
/// pages it releases fault back in when the heap grows again.
pub const MIN_INTERVAL: Duration = Duration::from_secs(2);

/// Scrollback shrinking by at least this many lines at once is worth a
/// trim (a `/clear` of a full 10 000-line history drops megabytes).
pub const HISTORY_DROP_LINES: usize = 1_000;

static REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_TRIM: Mutex<Option<Instant>> = Mutex::new(None);

/// Asks for a trim on the main loop's next [`maybe_trim`]. Any thread.
pub fn request() {
    REQUESTED.store(true, Ordering::Relaxed);
}

/// Trims if a trim was requested and [`MIN_INTERVAL`] has passed since the
/// last one. A request made too soon stays pending.
pub fn maybe_trim() {
    if !REQUESTED.load(Ordering::Relaxed) {
        return;
    }
    let mut last = LAST_TRIM.lock().unwrap_or_else(PoisonError::into_inner);
    if last.is_some_and(|at| at.elapsed() < MIN_INTERVAL) {
        return;
    }
    REQUESTED.store(false, Ordering::Relaxed);
    let start = Instant::now();
    *last = Some(start);
    let released = trim();
    tracing::debug!(
        target: "ultty::perf",
        released,
        elapsed_us = start.elapsed().as_micros(),
        "heap_trim"
    );
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim() -> bool {
    // SAFETY: malloc_trim takes no pointers and only touches glibc's own
    // allocator state, under its arena locks.
    unsafe { libc::malloc_trim(0) != 0 }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_served_once_then_waits_for_the_interval() {
        request();
        maybe_trim();
        assert!(!REQUESTED.load(Ordering::Relaxed), "served");
        request();
        maybe_trim(); // within MIN_INTERVAL of the first
        assert!(REQUESTED.load(Ordering::Relaxed), "kept pending");
    }
}
