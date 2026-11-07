//! Keypress-to-pixels latency on X11 (typometer-style).
//!
//! ubench-k2p --window ID --terminal NAME --out PATH [options]
//!
//! The terminal must be running `ubench-echo`, which draws each key at a
//! fixed cell. The probe:
//!   1. calibrates: grabs the whole window, injects one key, waits, grabs
//!      again, and takes the bounding box of the changed pixels as the cell;
//!   2. for each sample, grabs the cell, injects a key with XTest (alternating
//!      two keys so the glyph always changes), starts the clock once the
//!      request is flushed, and polls the cell with GetImage until it differs.
//!
//! What it measures: XTest injection to the new pixels being readable from
//! the X server's framebuffer (or the compositor's, for a composited
//! session). It does not include scanout or the panel: that is one refresh
//! period or more, which only a photodiode or a high-speed camera sees. The
//! poll interval (reported as `resolution_us`) bounds the timing error.
//! Under Xvfb there is no vsync, so the number reflects the application's
//! own pipeline only.
//!
//! Options:
//!   --samples N        (default 200)
//!   --case NAME        recorded case name (default idle)
//!   --claim CLASS      (default live-unknown)
//!   --gap-ms LO,HI     random wait between keys (default 50,300), so samples
//!                      do not phase-lock with vsync
//!   --timeout-ms N     per-sample wait for pixels (default 1000)
//!   --display-session S recorded as env.session
//!   --start-key K      after calibrating, press K and wait 1.5 s before
//!                      sampling (ubench-echo --flood starts its flood on `f`)

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::json;
use ultty_bench::{env, stats};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::protocol::xtest::ConnectionExt as _;

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;

struct Options {
    window: Window,
    terminal: String,
    out: String,
    samples: usize,
    case: String,
    claim: String,
    gap: (u64, u64),
    timeout: Duration,
    session: Option<String>,
    start_key: Option<u8>,
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        window: 0,
        terminal: String::new(),
        out: String::new(),
        samples: 200,
        case: "idle".into(),
        claim: "live-unknown".into(),
        gap: (50, 300),
        timeout: Duration::from_millis(1000),
        session: None,
        start_key: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--window" => {
                let v = value.trim_start_matches("0x");
                let radix = if value.starts_with("0x") { 16 } else { 10 };
                o.window = u32::from_str_radix(v, radix).map_err(|_| "bad --window")?;
            }
            "--terminal" => o.terminal = value,
            "--out" => o.out = value,
            "--samples" => o.samples = value.parse().map_err(|_| "bad --samples")?,
            "--case" => o.case = value,
            "--claim" => o.claim = value,
            "--gap-ms" => {
                let (lo, hi) = value.split_once(',').ok_or("--gap-ms is LO,HI")?;
                o.gap = (
                    lo.parse().map_err(|_| "bad gap")?,
                    hi.parse().map_err(|_| "bad gap")?,
                );
                if o.gap.0 > o.gap.1 {
                    return Err("--gap-ms LO must be <= HI".into());
                }
            }
            "--timeout-ms" => {
                o.timeout = Duration::from_millis(value.parse().map_err(|_| "bad --timeout-ms")?)
            }
            "--display-session" => o.session = Some(value),
            "--start-key" => match value.as_bytes() {
                [b] if b.is_ascii_lowercase() => o.start_key = Some(*b),
                _ => return Err("--start-key must be one lowercase letter".into()),
            },
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if o.window == 0 || o.terminal.is_empty() || o.out.is_empty() {
        return Err("usage: ubench-k2p --window ID --terminal NAME --out PATH [options]".into());
    }
    Ok(o)
}

/// Keycode that produces `keysym` without modifiers.
fn keycode_for(conn: &impl Connection, keysym: u32) -> Result<u8, String> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let per = usize::from(map.keysyms_per_keycode);
    map.keysyms
        .chunks(per)
        .position(|syms| syms.first() == Some(&keysym))
        .map(|i| min + i as u8)
        .ok_or_else(|| format!("no keycode for keysym {keysym:#x}"))
}

#[derive(Clone, Copy)]
struct Rect {
    x: i16,
    y: i16,
    w: u16,
    h: u16,
}

fn grab(conn: &impl Connection, root: Window, r: Rect) -> Result<Vec<u8>, String> {
    Ok(conn
        .get_image(ImageFormat::Z_PIXMAP, root, r.x, r.y, r.w, r.h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("GetImage: {e}"))?
        .data)
}

fn tap(conn: &impl Connection, root: Window, keycode: u8) -> Result<(), String> {
    conn.xtest_fake_input(KEY_PRESS, keycode, 0, root, 0, 0, 0)
        .map_err(|e| e.to_string())?;
    conn.xtest_fake_input(KEY_RELEASE, keycode, 0, root, 0, 0, 0)
        .map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())
}

/// Bounding box of differing 4-byte pixels between two grabs of `area`.
fn changed_box(before: &[u8], after: &[u8], area: Rect) -> Option<Rect> {
    let w = usize::from(area.w);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, (a, b)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
        if a != b {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 != usize::MAX).then(|| Rect {
        x: area.x + x0 as i16,
        y: area.y + y0 as i16,
        w: (x1 - x0 + 1) as u16,
        h: (y1 - y0 + 1) as u16,
    })
}

fn run(o: &Options) -> Result<(), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("connect: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let geometry = conn
        .get_geometry(o.window)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("window {:#x}: {e}", o.window))?;
    let origin = conn
        .translate_coordinates(o.window, root, 0, 0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let window_area = Rect {
        x: origin.dst_x,
        y: origin.dst_y,
        w: geometry.width,
        h: geometry.height,
    };
    conn.set_input_focus(x11rb::protocol::xproto::InputFocus::PARENT, o.window, 0u32)
        .map_err(|e| e.to_string())?;
    let keys = [
        keycode_for(&conn, u32::from(b'x'))?,
        keycode_for(&conn, u32::from(b'o'))?,
    ];

    // Calibrate on the bottom quarter of the window, where ubench-echo draws
    // (its last row); title bars and blinking elsewhere cannot be mistaken
    // for the echo. Wait until that area is still, find what one key
    // changes, and check that a second key changes the same box.
    let quarter = window_area.h / 4;
    let search = Rect {
        x: window_area.x,
        y: window_area.y + (window_area.h - quarter) as i16,
        w: window_area.w,
        h: quarter,
    };
    let mut before = grab(&conn, root, search)?;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        let again = grab(&conn, root, search)?;
        if again == before {
            break;
        }
        before = again;
    }
    // A slow terminal may not be reading keys yet: try a few times.
    let mut found = None;
    for attempt in 0..4 {
        tap(&conn, root, keys[attempt % 2])?;
        std::thread::sleep(Duration::from_millis(800));
        let after = grab(&conn, root, search)?;
        if let Some(cell) = changed_box(&before, &after, search) {
            found = Some((cell, attempt));
            break;
        }
        before = after;
    }
    let (cell, last) = found.ok_or(
        "no pixels changed after four key presses: is ubench-echo running and the window focused?",
    )?;
    let first = grab(&conn, root, cell)?;
    tap(&conn, root, keys[(last + 1) % 2])?;
    std::thread::sleep(Duration::from_millis(800));
    if grab(&conn, root, cell)? == first {
        return Err(format!(
            "calibration box {}x{} at ({}, {}) did not change on a second key; \
             something else on screen is moving",
            cell.w, cell.h, cell.x, cell.y
        ));
    }
    if let Some(k) = o.start_key {
        tap(&conn, root, keycode_for(&conn, u32::from(k))?)?;
        std::thread::sleep(Duration::from_millis(1500));
    }
    eprintln!(
        "ubench-k2p: watching {}x{} at ({}, {})",
        cell.w, cell.h, cell.x, cell.y
    );

    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut latencies = Vec::with_capacity(o.samples);
    let mut polls = Vec::new();
    let mut timeouts = 0usize;
    for i in 0..o.samples {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let gap = o.gap.0 + rng % (o.gap.1 - o.gap.0 + 1);
        std::thread::sleep(Duration::from_millis(gap));

        let baseline = grab(&conn, root, cell)?;
        // Calibration left keys[(last + 1) % 2] on screen.
        tap(&conn, root, keys[(last + i) % 2])?;
        let start = Instant::now();
        loop {
            let poll_start = Instant::now();
            let now = grab(&conn, root, cell)?;
            polls.push(poll_start.elapsed().as_secs_f64() * 1e6);
            if now != baseline {
                latencies.push(start.elapsed().as_secs_f64() * 1e3);
                break;
            }
            if start.elapsed() > o.timeout {
                timeouts += 1;
                break;
            }
        }
    }

    let resolution = stats::summarize(&polls).map_or(0.0, |s| s.median);
    let mut env = env::snapshot();
    env["session"] = json!(o.session);
    env["load_end"] = json!(env::load_average());
    env["display"] = json!(std::env::var("DISPLAY").ok());
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&o.out)
        .map_err(|e| format!("open {}: {e}", o.out))?;
    for (block, ms) in latencies.iter().enumerate() {
        let line = json!({
            "suite": "lat.k2p.external",
            "case": o.case,
            "arm": o.terminal,
            "block": block,
            "value": ms,
            "unit": "ms",
            "resolution_us": resolution,
            "method": "xtest+getimage",
            "claim": o.claim,
            "rev": env["rev"],
            "env": env,
        });
        writeln!(out, "{line}").map_err(|e| format!("write {}: {e}", o.out))?;
    }

    let mut sorted = latencies.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |q: f64| sorted.get(((sorted.len() as f64 - 1.0) * q).round() as usize);
    eprintln!(
        "ubench-k2p {}: n={} timeouts={} p50={:?} p90={:?} p99={:?} max={:?} ms, poll {:.0} us",
        o.terminal,
        sorted.len(),
        timeouts,
        pct(0.5),
        pct(0.9),
        pct(0.99),
        sorted.last(),
        resolution
    );
    if latencies.is_empty() {
        return Err("no sample saw the pixels change".into());
    }
    if timeouts * 10 > o.samples {
        return Err(format!("{timeouts} of {} samples timed out", o.samples));
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args().and_then(|o| run(&o)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ubench-k2p: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_box_finds_the_differing_pixels() {
        let area = Rect {
            x: 10,
            y: 20,
            w: 4,
            h: 3,
        };
        let before = vec![0u8; 4 * 4 * 3];
        let mut after = before.clone();
        // Pixels (1,1) and (2,2) change.
        after[(4 + 1) * 4] = 9;
        after[(2 * 4 + 2) * 4 + 1] = 9;
        let r = changed_box(&before, &after, area).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (11, 21, 2, 2));
        assert!(changed_box(&before, &before, area).is_none());
    }
}
