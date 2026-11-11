//! End-to-end cost of a flood of output through the real I/O path.
//!
//! `dd` writes a corpus file into a raw-mode PTY in 64 KiB writes, as
//! `ubench-feed` does (`cat` may splice instead), and the terminal's I/O
//! thread reads, parses and applies it exactly as in the live terminal. The
//! corpora are 32 MiB because spawning and reaping the child costs tens of
//! milliseconds, which hid most of the work at 8 MiB. The bench thread
//! plays the UI: it sleeps until the I/O thread wakes it, collects the
//! events, and once a frame interval has passed since the last frame holds
//! the terminal lock while it copies every visible row, as the renderer's
//! row cache does. What is timed is spawn to child exit, so it covers PTY
//! reads and wakeups, lock hand-off with the "renderer", and parse plus grid
//! work, but no GPU.
//!
//! Like the app, the bench thread waits for the next frame on the waker, not
//! in a blind sleep, so it sees the child's exit as soon as it is reported.
//! A blind sleep rounded every run up to the next frame boundary: times came
//! out as whole frames plus a constant, and ascii sat at 9 frames (about 219
//! MiB/s on a c7a.2xlarge) whatever the VT core did. See
//! `docs/perf/2026-11-11-pty-flood-frame-quantum.md`.
//!
//! The headless parse+grid ceiling is `ultty-bench vt`; the live numbers
//! with a real window are `scripts/bench/compare.sh`. This sits between the
//! two and runs under callgrind (`--separate-threads=yes` splits the I/O
//! thread from the bench thread).

use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use ultty::event_loop::{EventLoop, TerminalEvent};
use ultty::grid::Row;
use ultty_bench::corpus::{self, Corpus};

/// Window sizes in cells: the common one and a large one, where every
/// scrolled row and every frame's copy is about five times wider.
const GEOMETRIES: [(u16, u16); 2] = [(120, 40), (300, 75)];
const CORPUS_BYTES: usize = 32 * 1024 * 1024;
/// The UI's frame pacing (`FRAME_INTERVAL` in the app).
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Wakes from the I/O thread, as the event loop proxy delivers them.
#[derive(Default)]
struct Wakes {
    pending: Mutex<bool>,
    cond: Condvar,
}

impl Wakes {
    fn wake(&self) {
        *self.pending.lock().unwrap() = true;
        self.cond.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let pending = self.pending.lock().unwrap();
        let (mut pending, _) = self
            .cond
            .wait_timeout_while(pending, timeout, |pending| !*pending)
            .unwrap();
        *pending = false;
    }
}

/// Run `dd` on `path` in a `cols` x `lines` terminal until it exits;
/// returns the elapsed time.
fn flood(path: &str, (cols, lines): (u16, u16), rows: &mut Vec<Row>) -> Duration {
    let start = Instant::now();
    // Raw mode, as `ubench-feed` sets it: with output processing on, the
    // line discipline's ONLCR pass costs more than the terminal does.
    let mut event_loop = EventLoop::with_command(
        "/bin/sh",
        &[
            "/bin/sh",
            "-c",
            "stty raw -echo && exec dd if=\"$0\" bs=65536 status=none",
            path,
        ],
        cols,
        lines,
    )
    .expect("spawn dd");
    let wakes = Arc::new(Wakes::default());
    event_loop.set_waker({
        let wakes = Arc::clone(&wakes);
        Arc::new(move || wakes.wake())
    });
    let mut last_frame = Instant::now();
    let mut needs_frame = false;
    loop {
        let timeout = if needs_frame {
            FRAME_INTERVAL.saturating_sub(last_frame.elapsed())
        } else {
            FRAME_INTERVAL
        };
        wakes.wait(timeout);
        let events = event_loop.process().expect("process");
        needs_frame |= events.iter().any(|e| matches!(e, TerminalEvent::Redraw));
        let exited = events
            .iter()
            .any(|e| matches!(e, TerminalEvent::ChildExited { .. }));
        // On exit, copy the last output before the clock stops.
        if needs_frame && (exited || last_frame.elapsed() >= FRAME_INTERVAL) {
            let terminal = event_loop.terminal();
            let buffer = terminal.buffer();
            rows.resize_with(usize::from(lines), || Row::new(cols));
            for (index, row) in rows.iter_mut().enumerate() {
                if let Some(visible) = buffer.visible_row(index as u16) {
                    visible.copy_into(row);
                }
            }
            last_frame = Instant::now();
            needs_frame = false;
        }
        if exited {
            return start.elapsed();
        }
    }
}

fn bench_pty_flood(c: &mut Criterion) {
    let mut group = c.benchmark_group("pty_flood");
    group.sample_size(10);
    let dir = tempfile::tempdir().expect("tempdir");
    let only = std::env::var("PTY_FLOOD_CORPUS").ok();
    for corpus in [
        Corpus::Ascii,
        Corpus::UnicodeMixed,
        Corpus::Cjk,
        Corpus::CursorRedraw,
        Corpus::SgrDense,
    ] {
        if only.as_deref().is_some_and(|name| name != corpus.name()) {
            continue;
        }
        let generated = corpus::generate(corpus, CORPUS_BYTES);
        let path = dir.path().join(corpus.name());
        std::fs::File::create(&path)
            .and_then(|mut file| file.write_all(&generated.bytes))
            .expect("write corpus");
        let path = path.to_str().expect("utf-8 temp path").to_owned();
        group.throughput(Throughput::Bytes(generated.bytes.len() as u64));
        for geometry @ (cols, lines) in GEOMETRIES {
            let mut rows = Vec::new();
            group.bench_function(format!("{}/{cols}x{lines}", corpus.name()), |b| {
                b.iter_custom(|iters| (0..iters).map(|_| flood(&path, geometry, &mut rows)).sum());
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_pty_flood);
criterion_main!(benches);
