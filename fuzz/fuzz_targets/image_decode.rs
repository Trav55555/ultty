//! Feed raw bytes to the Kitty graphics decoders (PNG, zlib and raw
//! pixels) with the production limits, and assert that none panics and
//! that peak allocation stays within the caps: a decompression bomb or a
//! PNG header claiming a huge image must be refused before it allocates.
#![no_main]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use libfuzzer_sys::fuzz_target;
use ultty_vt::grid::graphics::decode::{self, Encoding, MAX_DECODED_BYTES, MAX_TRANSMITTED_BYTES};

/// Counts live and peak heap bytes, as `benches/memory.rs` does.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only
// updates counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                let live = LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size
                    - layout.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most a decode may hold at once: the inflated PNG, the decoder's
/// frame and the RGBA copy, plus room for the decoder's own buffers.
const PEAK_LIMIT: usize = MAX_TRANSMITTED_BYTES + 2 * MAX_DECODED_BYTES + 16 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let [mode, a, b, rest @ ..] = data else {
        return;
    };
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let input = rest.to_vec();
    let input_bytes = input.len();
    match mode % 4 {
        0 => drop(decode::decode_png(&input)),
        1 => {
            let limit = usize::from(*a) << 8 | usize::from(*b);
            if let Ok(out) = decode::inflate(&input, limit) {
                assert!(out.len() <= limit, "inflate passed its limit");
            }
        }
        _ => {
            let encoding = Encoding {
                format: [24, 32, 100][usize::from(*a) % 3],
                // Up to the full dimension cap, so oversized claims are tried.
                width: u32::from(*b) * 64 + 1,
                height: u32::from(*a) * 64 + 1,
                compressed: mode & 4 != 0,
                size: u32::from(*b) << 12,
            };
            if let Ok(image) = decode::decode(&encoding, input) {
                assert_eq!(
                    image.rgba.len(),
                    image.width as usize * image.height as usize * 4
                );
                assert!(image.rgba.len() <= MAX_DECODED_BYTES);
            }
        }
    }
    let peak = PEAK.load(Ordering::Relaxed) - before;
    assert!(
        peak <= PEAK_LIMIT + input_bytes,
        "peak allocation {peak} past {PEAK_LIMIT}"
    );
});
