//! Terminal-neutral throughput driver. Run it *inside* the terminal under
//! test; it needs nothing from the terminal but a PTY.
//!
//! ubench-feed --out results.jsonl --terminal NAME [options] FILE...
//!
//! For each FILE and repeat: wait until the terminal is idle (a cursor
//! position query answered), start the clock, write the file to /dev/tty in
//! 64 KiB writes, send `CSI 6n`, and stop the clock when the reply arrives.
//! Terminals answer in order, so the reply proves every byte was consumed;
//! timing `cat` from a shell only shows when the producer finished.
//!
//! Options:
//!   --out PATH            append JSON lines here (stdout is the terminal under test)
//!   --terminal NAME       arm label (ultty, ghostty, gnome-terminal, alacritty)
//!   --repeat N            passes per file (default 3)
//!   --block N             block number recorded with the samples (default 0)
//!   --claim CLASS         live-realgpu | live-swrast | comparative (default live-unknown)
//!   --expect-size CxR     wait up to --size-wait seconds for this `stty size`;
//!                         fail if it never matches (default: record only)
//!   --size-wait SECS      (default 10)
//!   --status PATH         write "rows cols" here while waiting, for a launcher
//!                         that resizes the window
//!   --timeout SECS        per-pass fence timeout (default 300)
//!   --display-session S   recorded as env.session (x11, wayland, xvfb)
//!
//! Exit status is non-zero if any pass times out or the size never matches.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::json;
use ultty_bench::corpus::{first_bare_lf, sha256_hex};
use ultty_bench::env;
use ultty_bench::tty::{stty_size, RawTty};

const CHUNK: usize = 64 * 1024;

struct Options {
    out: String,
    terminal: String,
    repeat: u32,
    block: u64,
    claim: String,
    expect: Option<(u16, u16)>,
    size_wait: Duration,
    status: Option<String>,
    timeout: Duration,
    session: Option<String>,
    files: Vec<String>,
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        out: String::new(),
        terminal: String::new(),
        repeat: 3,
        block: 0,
        claim: "live-unknown".into(),
        expect: None,
        size_wait: Duration::from_secs(10),
        status: None,
        timeout: Duration::from_secs(300),
        session: None,
        files: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => o.out = value()?,
            "--terminal" => o.terminal = value()?,
            "--repeat" => o.repeat = value()?.parse().map_err(|_| "bad --repeat")?,
            "--block" => o.block = value()?.parse().map_err(|_| "bad --block")?,
            "--claim" => o.claim = value()?,
            "--expect-size" => {
                let v = value()?;
                let (c, r) = v.split_once('x').ok_or("--expect-size is COLSxROWS")?;
                o.expect = Some((
                    c.parse().map_err(|_| "bad cols")?,
                    r.parse().map_err(|_| "bad rows")?,
                ));
            }
            "--size-wait" => {
                o.size_wait = Duration::from_secs(value()?.parse().map_err(|_| "bad --size-wait")?)
            }
            "--status" => o.status = Some(value()?),
            "--timeout" => {
                o.timeout = Duration::from_secs(value()?.parse().map_err(|_| "bad --timeout")?)
            }
            "--display-session" => o.session = Some(value()?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.files.push(arg),
        }
    }
    if o.out.is_empty() || o.terminal.is_empty() || o.files.is_empty() {
        return Err("usage: ubench-feed --out PATH --terminal NAME [options] FILE...".into());
    }
    Ok(o)
}

/// Wait for the expected size, reporting progress through `status`.
fn settle_size(o: &Options) -> Result<(u16, u16), String> {
    let deadline = Instant::now() + o.size_wait;
    loop {
        let (rows, cols) = stty_size().map_err(|e| format!("stty size: {e}"))?;
        if let Some(path) = &o.status {
            // The launcher polls this file; a failed write only slows it down.
            if let Err(e) = std::fs::write(path, format!("{rows} {cols}\n")) {
                eprintln!("ubench-feed: write {path}: {e}");
            }
        }
        match o.expect {
            None => return Ok((cols, rows)),
            Some((c, r)) if c == cols && r == rows => return Ok((cols, rows)),
            Some((c, r)) if Instant::now() >= deadline => {
                return Err(format!(
                    "stty size is {cols}x{rows}, expected {c}x{r}; the comparison needs equal \
                     cell geometry"
                ))
            }
            Some(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn run(o: &Options) -> Result<(), String> {
    let (cols, rows) = settle_size(o)?;
    let mut inputs = Vec::new();
    for path in &o.files {
        let bytes = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        if let Some(at) = first_bare_lf(&bytes) {
            return Err(format!(
                "{path} has a bare LF at byte {at}; corpora must be CRLF"
            ));
        }
        let name = Path::new(path)
            .file_stem()
            .map_or_else(|| path.clone(), |s| s.to_string_lossy().into_owned());
        let sha = sha256_hex(&bytes);
        inputs.push((name, bytes, sha));
    }

    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&o.out)
        .map_err(|e| format!("open {}: {e}", o.out))?;
    let mut env = env::snapshot();
    env["term"] = json!(std::env::var("TERM").ok());
    env["session"] = json!(o.session);
    env["cells"] = json!(format!("{cols}x{rows}"));

    let mut tty = RawTty::open().map_err(|e| format!("open /dev/tty: {e}"))?;
    tty.drain_input().map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    let mut failure = None;
    'outer: for pass in 0..o.repeat {
        for (name, bytes, sha) in &inputs {
            // Start from an idle terminal and a clean screen.
            let reset = b"\x1b[0m\x1b[r\x1b[?1049l\x1b[H\x1b[2J";
            if let Err(e) = tty
                .write_all(reset)
                .and_then(|()| tty.cursor_position_fence(o.timeout).map(drop))
            {
                failure = Some(format!("{name}: terminal not idle: {e}"));
                break 'outer;
            }
            let start = Instant::now();
            let written = bytes
                .chunks(CHUNK)
                .try_for_each(|chunk| tty.write_all(chunk))
                .and_then(|()| tty.cursor_position_fence(o.timeout));
            let secs = start.elapsed().as_secs_f64();
            if let Err(e) = written {
                failure = Some(format!("{name} pass {pass}: {e}"));
                break 'outer;
            }
            lines.push(json!({
                "suite": "tp.live",
                "case": format!("{name}/{cols}x{rows}"),
                "arm": o.terminal,
                "block": o.block * u64::from(o.repeat) + u64::from(pass),
                "value": bytes.len() as f64 / (1024.0 * 1024.0) / secs,
                "unit": "MiB/s",
                "secs": secs,
                "bytes": bytes.len(),
                "corpus": name,
                "corpus_sha256": sha,
                "claim": o.claim,
            }));
        }
    }
    // Leave the terminal usable for whatever runs next.
    let restore = tty
        .write_all(b"\x1b[0m\x1b[r\x1b[?1049l\x1b[H\x1b[2J")
        .and_then(|()| tty.flush());
    drop(tty);
    if let Err(e) = restore {
        eprintln!("ubench-feed: restoring the terminal failed: {e}");
    }

    env["load_end"] = json!(env::load_average());
    for mut line in lines {
        line["rev"] = env["rev"].clone();
        line["env"] = env.clone();
        writeln!(out, "{line}").map_err(|e| format!("write {}: {e}", o.out))?;
    }
    match failure {
        Some(f) => Err(f),
        None => Ok(()),
    }
}

fn main() -> ExitCode {
    let result = parse_args().and_then(|o| run(&o));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ubench-feed: {e}");
            ExitCode::FAILURE
        }
    }
}
