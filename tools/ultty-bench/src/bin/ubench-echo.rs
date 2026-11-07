//! Terminal-neutral echo target for keypress-to-pixels latency. Run it
//! *inside* the terminal under test; an external probe (`ubench-k2p` on
//! X11, `scripts/bench/k2p-wayland.py` on Wayland) injects keys and watches
//! the pixels.
//!
//! ubench-echo [--flood MIBS|max] [--quit-byte N]
//!
//! Each key byte read from /dev/tty is drawn immediately, alone, at a fixed
//! cell: column 2 of the last row. A shell and line discipline would add
//! their own variable delay; this program adds one read and one write. The
//! probe alternates two keys so every keystroke changes the cell.
//!
//! --flood RATE scrolls text through a DECSTBM region covering every row
//! but the last, at RATE MiB/s (or as fast as the terminal accepts with
//! `max`), so latency can be measured while the terminal is busy. The flood
//! starts at the first `f` key, after the probe has calibrated on a still
//! screen; otherwise the moving text would be mistaken for the echo. Echo and
//! flood writes share one lock, so their escape sequences never interleave.
//!
//! Exits on Ctrl-C (byte 3) or the --quit-byte (default `Q`).

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ultty_bench::corpus::{self, Corpus};
use ultty_bench::tty::{stty_size, RawTty};

/// The key that starts the flood (`ubench-k2p --start-key f`).
const FLOOD_START: u8 = b'f';

enum Flood {
    Off,
    Rate(f64),
    Max,
}

fn parse_args() -> Result<(Flood, u8), String> {
    let mut flood = Flood::Off;
    let mut quit = b'Q';
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--flood" if value == "max" => flood = Flood::Max,
            "--flood" => {
                let rate: f64 = value.parse().map_err(|_| "bad --flood rate")?;
                if rate <= 0.0 {
                    return Err("--flood rate must be positive".into());
                }
                flood = Flood::Rate(rate);
            }
            "--quit-byte" => quit = value.parse().map_err(|_| "bad --quit-byte")?,
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    Ok((flood, quit))
}

fn run() -> Result<(), String> {
    let (flood, quit) = parse_args()?;
    let (rows, _cols) = stty_size().map_err(|e| format!("stty size: {e}"))?;
    let tty = Arc::new(Mutex::new(
        RawTty::open().map_err(|e| format!("open /dev/tty: {e}"))?,
    ));
    let echo_at = format!("\x1b[{rows};2H");
    {
        let mut t = tty.lock().map_err(|_| "tty lock poisoned")?;
        let setup = format!(
            "\x1b[0m\x1b[H\x1b[2J\x1b[?25l\x1b[1;{}r\x1b[{};1H",
            rows.saturating_sub(1).max(1),
            rows.saturating_sub(1).max(1)
        );
        t.write_all(setup.as_bytes())
            .and_then(|()| t.flush())
            .map_err(|e| e.to_string())?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let go = Arc::new(AtomicBool::new(false));
    let flood_thread = match flood {
        Flood::Off => None,
        Flood::Rate(_) | Flood::Max => {
            let rate = match flood {
                Flood::Rate(r) => Some(r * 1024.0 * 1024.0),
                _ => None,
            };
            let tty = Arc::clone(&tty);
            let stop = Arc::clone(&stop);
            let go = Arc::clone(&go);
            Some(std::thread::spawn(move || {
                flood_loop(&tty, &stop, &go, rate)
            }))
        }
    };

    let result = echo_loop(&tty, &echo_at, quit, &go);
    stop.store(true, Ordering::Relaxed);
    if let Some(handle) = flood_thread {
        if handle.join().is_err() {
            eprintln!("ubench-echo: flood thread panicked");
        }
    }
    if let Ok(mut t) = tty.lock() {
        let restore = t
            .write_all(b"\x1b[r\x1b[?25h\x1b[0m\x1b[H\x1b[2J")
            .and_then(|()| t.flush());
        if let Err(e) = restore {
            eprintln!("ubench-echo: restoring the terminal failed: {e}");
        }
    }
    result
}

fn echo_loop(tty: &Mutex<RawTty>, echo_at: &str, quit: u8, go: &AtomicBool) -> Result<(), String> {
    // Keys are read on a separate handle, so waiting for one never blocks
    // the flood writer; only the echo write takes the lock.
    let mut reader = tty
        .lock()
        .map_err(|_| "tty lock poisoned")?
        .reader()
        .map_err(|e| e.to_string())?;
    loop {
        let byte = reader
            .read_byte(Duration::from_millis(100))
            .map_err(|e| e.to_string())?;
        let Some(byte) = byte else { continue };
        if byte == 3 || byte == quit {
            return Ok(());
        }
        if byte == FLOOD_START {
            go.store(true, Ordering::Relaxed);
        }
        let glyph = if byte.is_ascii_graphic() { byte } else { b'?' };
        let mut frame = Vec::with_capacity(24);
        frame.extend_from_slice(b"\x1b7");
        frame.extend_from_slice(echo_at.as_bytes());
        frame.push(glyph);
        frame.extend_from_slice(b"\x1b8");
        let mut t = tty.lock().map_err(|_| "tty lock poisoned")?;
        t.write_all(&frame)
            .and_then(|()| t.flush())
            .map_err(|e| e.to_string())?;
    }
}

/// Write CRLF lines from the ascii and sgr-dense corpora in small, whole
/// chunks; `rate` is bytes per second, `None` for unthrottled.
fn flood_loop(tty: &Mutex<RawTty>, stop: &AtomicBool, go: &AtomicBool, rate: Option<f64>) {
    let mut text = corpus::generate(Corpus::SgrDense, 1024 * 1024).bytes;
    text.extend_from_slice(&corpus::generate(Corpus::Ascii, 1024 * 1024).bytes);
    // Split at line ends so no escape sequence spans two writes.
    let mut chunks = Vec::new();
    let mut start = 0;
    for (i, w) in text.windows(2).enumerate() {
        if w == b"\r\n" && i + 2 - start >= 4096 {
            chunks.push(&text[start..i + 2]);
            start = i + 2;
        }
    }
    // Wait for the start key, so the probe can calibrate on a still screen.
    while !go.load(Ordering::Relaxed) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let began = Instant::now();
    let mut sent = 0f64;
    for chunk in chunks.iter().cycle() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(rate) = rate {
            let due = Duration::from_secs_f64(sent / rate);
            if let Some(wait) = due.checked_sub(began.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let Ok(mut t) = tty.lock() else { return };
        if t.write_all(chunk).is_err() {
            return;
        }
        sent += chunk.len() as f64;
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ubench-echo: {e}");
            ExitCode::FAILURE
        }
    }
}
