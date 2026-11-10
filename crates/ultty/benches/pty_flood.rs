//! End-to-end cost of a flood of output through the real I/O path.
//!
//! `dd` writes a corpus file into a raw-mode PTY in 64 KiB writes, as
//! `ubench-feed` does (`cat` may splice instead), and the terminal's I/O
//! thread reads, parses and applies it exactly as in the live terminal. The
//! corpora are 32 MiB because spawning and reaping the child costs tens of
//! milliseconds, which hid most of the work at 8 MiB. The bench thread
//! plays the UI: it sleeps until the I/O thread wakes it, waits out a frame
//! interval, then holds the terminal lock while it copies every visible row,
//! as the renderer's row cache does. What is timed is spawn to child exit,
//! so it covers PTY reads and wakeups, lock hand-off with the "renderer",
//! and parse plus grid work, but no GPU.
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

const COLS: u16 = 120;
const ROWS: u16 = 40;
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

/// Run `dd` on `path` in a terminal until it exits; returns the elapsed time.
fn flood(path: &str, rows: &mut Vec<Row>) -> Duration {
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
        COLS,
        ROWS,
    )
    .expect("spawn dd");
    let wakes = Arc::new(Wakes::default());
    event_loop.set_waker({
        let wakes = Arc::clone(&wakes);
        Arc::new(move || wakes.wake())
    });
    let mut last_frame = Instant::now();
    loop {
        wakes.wait(FRAME_INTERVAL);
        let events = event_loop.process().expect("process");
        if events.iter().any(|e| matches!(e, TerminalEvent::Redraw)) {
            if let Some(wait) = FRAME_INTERVAL.checked_sub(last_frame.elapsed()) {
                std::thread::sleep(wait);
            }
            let terminal = event_loop.terminal();
            let buffer = terminal.buffer();
            rows.resize_with(usize::from(ROWS), || Row::new(COLS));
            for (index, row) in rows.iter_mut().enumerate() {
                if let Some(visible) = buffer.visible_row(index as u16) {
                    visible.copy_into(row);
                }
            }
            last_frame = Instant::now();
        }
        if events
            .iter()
            .any(|e| matches!(e, TerminalEvent::ChildExited { .. }))
        {
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
        let mut rows = Vec::new();
        group.throughput(Throughput::Bytes(generated.bytes.len() as u64));
        group.bench_function(format!("{}/{COLS}x{ROWS}", corpus.name()), |b| {
            b.iter_custom(|iters| (0..iters).map(|_| flood(&path, &mut rows)).sum());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_pty_flood);
criterion_main!(benches);
