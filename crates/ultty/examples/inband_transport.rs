//! Prototype benchmark for ADR 0018's in-band transport.
//!
//! Compares carrying native protocol frames over the per-tab Unix socket
//! (today) with carrying them in-band as `ESC _ U <base64url> ESC \` on the
//! PTY. The APC framing here is a prototype; it is not wired into Ultty.
//!
//! cargo run --release -p ultty --example inband_transport -- <corpus.bin>
//!
//! The corpus holds real traffic recorded from the example apps. Each record
//! is `dir u8` (0 = app to terminal, 1 = terminal to app), `app u8`,
//! `len u32 LE`, then `len` bytes of concatenated protocol frames. App to
//! terminal records are whole `BeginFrame..EndFrame` updates.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ultty_protocol::{
    AppMessage, Constraints, Content, Decode, Encode, FrameId, ImageScale, ImageWidget, RegionId,
    Style, TerminalMessage, TextContent, WidgetContent,
};
use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

const APP_NAMES: [&str; 5] = ["counter-app", "todo-app", "form-app", "chat", "dashboard"];

struct Record {
    to_terminal: bool,
    app: usize,
    bytes: Vec<u8>,
}

fn load_corpus(path: &str) -> Vec<Record> {
    let data = std::fs::read(path).expect("read corpus");
    let mut records = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let dir = data[i];
        let app = data[i + 1] as usize;
        let len = u32::from_le_bytes(data[i + 2..i + 6].try_into().unwrap()) as usize;
        i += 6;
        records.push(Record {
            to_terminal: dir == 0,
            app,
            bytes: data[i..i + len].to_vec(),
        });
        i += len;
    }
    records
}

/// Wrap one update (any number of concatenated frames) in a single APC.
fn wrap(raw: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b_U");
    let start = out.len();
    out.resize(start + base64::encoded_len(raw.len(), false).unwrap(), 0);
    let written = URL_SAFE_NO_PAD
        .encode_slice(raw, &mut out[start..])
        .unwrap();
    out.truncate(start + written);
    out.extend_from_slice(b"\x1b\\");
}

/// Pulls `ESC _ U ... ESC \` payloads out of a byte stream that may be split
/// at any point. Everything else is passed over, as the VT parser would.
#[derive(Default)]
struct Extractor {
    state: u8, // 0 ground, 1 saw ESC, 2 saw ESC _, 3 in payload, 4 payload ESC
    payload: Vec<u8>,
}

impl Extractor {
    fn feed(&mut self, bytes: &[u8], mut on_payload: impl FnMut(&[u8])) {
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match self.state {
                0 => match bytes[i..].iter().position(|&c| c == 0x1b) {
                    Some(p) => {
                        i += p;
                        self.state = 1;
                    }
                    None => return,
                },
                1 => self.state = if b == b'_' { 2 } else { 0 },
                2 => self.state = if b == b'U' { 3 } else { 0 },
                3 => match bytes[i..].iter().position(|&c| c == 0x1b) {
                    Some(p) => {
                        self.payload.extend_from_slice(&bytes[i..i + p]);
                        i += p;
                        self.state = 4;
                    }
                    None => {
                        self.payload.extend_from_slice(&bytes[i..]);
                        return;
                    }
                },
                _ => {
                    if b == b'\\' {
                        on_payload(&self.payload);
                    }
                    self.payload.clear();
                    self.state = 0;
                }
            }
            i += 1;
        }
    }
}

fn decode_app_frames(mut raw: &[u8]) -> usize {
    let mut n = 0;
    while !raw.is_empty() {
        AppMessage::decode(&mut raw).expect("valid app frame");
        n += 1;
    }
    n
}

fn decode_terminal_frames(mut raw: &[u8]) -> usize {
    let mut n = 0;
    while !raw.is_empty() {
        TerminalMessage::decode(&mut raw).expect("valid terminal frame");
        n += 1;
    }
    n
}

fn feed_vt(terminal: &mut Terminal, parser: &mut AnsiParser, bytes: &[u8]) {
    let mut handler = TerminalHandler::new(terminal);
    parser.feed_with_ascii_spans(bytes, |out| match out {
        AnsiOutput::Action(action) => handler.handle(action),
        AnsiOutput::PrintAscii(span) => handler.handle_ascii_span(span),
    });
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

// ---------------------------------------------------------------- sizes

fn report_sizes(records: &[Record]) {
    println!("## 1. Bytes on the wire (real traffic, app to terminal)\n");
    println!("| app | updates | raw bytes | in-band bytes | overhead |");
    println!("|---|---:|---:|---:|---:|");
    let (mut all_raw, mut all_wrapped) = (0usize, 0usize);
    for (ai, name) in APP_NAMES.iter().enumerate() {
        let updates: Vec<_> = records
            .iter()
            .filter(|r| r.to_terminal && r.app == ai)
            .collect();
        let raw: usize = updates.iter().map(|r| r.bytes.len()).sum();
        let wrapped: usize = updates
            .iter()
            .map(|r| {
                let mut out = Vec::new();
                wrap(&r.bytes, &mut out);
                out.len()
            })
            .sum();
        all_raw += raw;
        all_wrapped += wrapped;
        println!(
            "| {name} | {} | {raw} | {wrapped} | {:+.0}% |",
            updates.len(),
            (wrapped as f64 / raw as f64 - 1.0) * 100.0
        );
    }
    println!(
        "| **all** | | {all_raw} | {all_wrapped} | {:+.0}% |\n",
        (all_wrapped as f64 / all_raw as f64 - 1.0) * 100.0
    );
}

// ---------------------------------------------------------------- CPU

struct CpuResult {
    label: String,
    raw_bytes: usize,
    socket_ns: f64,
    vt_ns: f64,
    extract_ns: f64,
}

/// Terminal-side CPU for `updates`, repeated until at least ~0.3 s elapse.
fn measure_cpu(label: &str, updates: &[Vec<u8>]) -> CpuResult {
    let raw_bytes: usize = updates.iter().map(Vec::len).sum();
    let mut stream = Vec::new();
    for u in updates {
        wrap(u, &mut stream);
    }
    let reps = (64 * 1024 * 1024 / raw_bytes.max(1)).clamp(3, 20_000);

    let time = |f: &mut dyn FnMut()| {
        f(); // warm up
        let start = Instant::now();
        for _ in 0..reps {
            f();
        }
        start.elapsed().as_nanos() as f64 / reps as f64
    };

    let socket_ns = time(&mut || {
        for u in updates {
            std::hint::black_box(decode_app_frames(u));
        }
    });

    let mut terminal = Terminal::new(Dimensions::new(120, 40));
    let mut parser = AnsiParser::new();
    let vt_ns = time(&mut || feed_vt(&mut terminal, &mut parser, &stream));

    let mut extractor = Extractor::default();
    let mut decoded = Vec::new();
    let extract_ns = time(&mut || {
        extractor.feed(&stream, |payload| {
            decoded.clear();
            URL_SAFE_NO_PAD.decode_vec(payload, &mut decoded).unwrap();
            std::hint::black_box(decode_app_frames(&decoded));
        });
    });

    CpuResult {
        label: label.to_string(),
        raw_bytes,
        socket_ns,
        vt_ns,
        extract_ns,
    }
}

fn synthetic_scene(regions: u32) -> Vec<u8> {
    let mut msgs = vec![AppMessage::BeginFrame {
        frame_id: FrameId(1),
    }];
    for id in 1..=regions {
        msgs.push(AppMessage::CreateRegion {
            id: RegionId::new(id),
            parent: RegionId::new(0),
            insert_index: -1,
            constraints: Constraints::default(),
            style: Style::default(),
        });
        msgs.push(AppMessage::SetContent {
            id: RegionId::new(id),
            content: Content::Text(TextContent::new(format!(
                "row {id}: a line of list content of ordinary length"
            ))),
        });
    }
    msgs.push(AppMessage::EndFrame {
        frame_id: FrameId(1),
    });
    let mut out = Vec::new();
    for m in msgs {
        out.extend_from_slice(&m.encode_to_bytes());
    }
    out
}

fn synthetic_image(side: u32) -> Vec<u8> {
    let data: Vec<u8> = (0..side * side * 4).map(|i| (i * 7 % 251) as u8).collect();
    AppMessage::SetContent {
        id: RegionId::new(1),
        content: Content::Widget(WidgetContent::Image(ImageWidget {
            data,
            width: side,
            height: side,
            scale: ImageScale::Fit,
        })),
    }
    .encode_to_bytes()
    .to_vec()
}

fn report_cpu(records: &[Record]) {
    println!("## 2. Terminal-side CPU per batch\n");
    println!(
        "Socket = decode frames. In-band = VT parser over the APC bytes \
         (today's discard path) + extract + base64 decode + decode frames.\n"
    );
    println!(
        "| batch | raw bytes | socket | in-band | VT parse | extract+b64+decode | in-band MiB/s |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|");
    let mut cases = Vec::new();
    for (ai, name) in APP_NAMES.iter().enumerate() {
        let updates: Vec<Vec<u8>> = records
            .iter()
            .filter(|r| r.to_terminal && r.app == ai)
            .map(|r| r.bytes.clone())
            .collect();
        cases.push(measure_cpu(&format!("{name} session"), &updates));
    }
    let median_update = {
        let mut all: Vec<&Vec<u8>> = records
            .iter()
            .filter(|r| r.to_terminal)
            .map(|r| &r.bytes)
            .collect();
        all.sort_by_key(|b| b.len());
        all[all.len() / 2].clone()
    };
    cases.push(measure_cpu(
        &format!("median real update ({} B)", median_update.len()),
        &[median_update],
    ));
    cases.push(measure_cpu(
        "synthetic 2,000-region scene",
        &[synthetic_scene(2000)],
    ));
    cases.push(measure_cpu(
        "synthetic 512x512 RGBA image",
        &[synthetic_image(512)],
    ));
    for c in cases {
        let inband = c.vt_ns + c.extract_ns;
        let fmt = |ns: f64| {
            if ns >= 1e6 {
                format!("{:.2} ms", ns / 1e6)
            } else {
                format!("{:.1} µs", ns / 1e3)
            }
        };
        println!(
            "| {} | {} | {} | {} | {} | {} | {:.0} |",
            c.label,
            c.raw_bytes,
            fmt(c.socket_ns),
            fmt(inband),
            fmt(c.vt_ns),
            fmt(c.extract_ns),
            c.raw_bytes as f64 / (1024.0 * 1024.0) / (inband / 1e9)
        );
    }
    println!();
}

// ---------------------------------------------------------------- latency

trait Duplex: Read + Write + Send {}
impl<T: Read + Write + Send> Duplex for T {}

/// Open a raw-mode PTY pair as (app side, terminal side).
fn raw_pty() -> (File, File) {
    use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
    let pty = nix::pty::openpty(None, None).expect("openpty");
    let mut t = tcgetattr(&pty.slave).expect("tcgetattr");
    cfmakeraw(&mut t);
    tcsetattr(&pty.slave, SetArg::TCSANOW, &t).expect("tcsetattr");
    (File::from(pty.slave), File::from(pty.master))
}

/// Read until `want` complete payloads (in-band) or `want_bytes` (socket).
fn read_message(
    port: &mut dyn Duplex,
    inband: bool,
    want_bytes: usize,
    extractor: &mut Extractor,
    buf: &mut [u8],
) {
    if inband {
        let mut done = false;
        while !done {
            let n = port.read(buf).expect("read");
            assert!(n > 0, "peer closed");
            extractor.feed(&buf[..n], |p| {
                let raw = URL_SAFE_NO_PAD.decode(p).unwrap();
                std::hint::black_box(raw);
                done = true;
            });
        }
    } else {
        let mut got = 0;
        let cap = buf.len();
        while got < want_bytes {
            let n = port
                .read(&mut buf[..(want_bytes - got).min(cap)])
                .expect("read");
            assert!(n > 0, "peer closed");
            got += n;
        }
    }
}

/// Round trips: app sends `request`, terminal decodes it and sends `reply`.
fn round_trips(
    mut app: Box<dyn Duplex>,
    mut term: Box<dyn Duplex>,
    inband: bool,
    request: &[u8],
    reply: &[u8],
    iterations: usize,
) -> Vec<u64> {
    let encode = |raw: &[u8]| {
        if inband {
            let mut out = Vec::new();
            wrap(raw, &mut out);
            out
        } else {
            raw.to_vec()
        }
    };
    let (req_wire, reply_wire) = (encode(request), encode(reply));
    let (req_len, reply_len) = (request.len(), reply.len());
    let terminal = std::thread::spawn(move || {
        let mut extractor = Extractor::default();
        let mut buf = vec![0u8; 64 * 1024];
        for _ in 0..iterations + 100 {
            read_message(&mut *term, inband, req_len, &mut extractor, &mut buf);
            term.write_all(&reply_wire).unwrap();
        }
        term
    });
    let mut extractor = Extractor::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut samples = Vec::with_capacity(iterations);
    for i in 0..iterations + 100 {
        let start = Instant::now();
        app.write_all(&req_wire).unwrap();
        read_message(&mut *app, inband, reply_len, &mut extractor, &mut buf);
        if i >= 100 {
            samples.push(start.elapsed().as_nanos() as u64);
        }
    }
    drop(terminal.join().unwrap());
    samples.sort_unstable();
    samples
}

fn report_latency(records: &[Record]) {
    println!("## 3. Round-trip latency (app update -> terminal -> reply)\n");
    println!(
        "Same-machine, two threads, blocking I/O. The PTY is in raw mode, as \
         an app's tty is while it runs.\n"
    );
    println!("| exchange | transport | p50 | p99 | p99.9 |");
    println!("|---|---|---:|---:|---:|");
    let mut requests: Vec<&Vec<u8>> = records
        .iter()
        .filter(|r| r.to_terminal)
        .map(|r| &r.bytes)
        .collect();
    requests.sort_by_key(|b| b.len());
    let mut replies: Vec<&Vec<u8>> = records
        .iter()
        .filter(|r| !r.to_terminal)
        .map(|r| &r.bytes)
        .collect();
    replies.sort_by_key(|b| b.len());
    let typical = (requests[requests.len() / 2], replies[replies.len() / 2]);
    let largest = (requests[requests.len() - 1], replies[replies.len() - 1]);
    assert_eq!(decode_terminal_frames(largest.1), 1);
    for (label, (req, rep)) in [
        (
            format!("median ({} B -> {} B)", typical.0.len(), typical.1.len()),
            typical,
        ),
        (
            format!("largest ({} B -> {} B)", largest.0.len(), largest.1.len()),
            largest,
        ),
    ] {
        for inband in [false, true] {
            let samples = if inband {
                let (app, term) = raw_pty();
                round_trips(Box::new(app), Box::new(term), true, req, rep, 20_000)
            } else {
                let (app, term) = UnixStream::pair().unwrap();
                round_trips(Box::new(app), Box::new(term), false, req, rep, 20_000)
            };
            println!(
                "| {label} | {} | {:.1} µs | {:.1} µs | {:.1} µs |",
                if inband { "PTY + APC" } else { "Unix socket" },
                percentile(&samples, 0.5) as f64 / 1e3,
                percentile(&samples, 0.99) as f64 / 1e3,
                percentile(&samples, 0.999) as f64 / 1e3,
            );
        }
    }
    println!();
}

// ---------------------------------------------------------------- bulk

/// Push `raw` one way and time until the receiver has decoded it.
fn bulk_once(
    mut tx: Box<dyn Duplex>,
    mut rx: Box<dyn Duplex>,
    inband: bool,
    raw: &[u8],
) -> Duration {
    let wire = if inband {
        let mut out = Vec::new();
        wrap(raw, &mut out);
        out
    } else {
        raw.to_vec()
    };
    let raw_len = raw.len();
    let start = Instant::now();
    let reader = std::thread::spawn(move || {
        let mut extractor = Extractor::default();
        let mut buf = vec![0u8; 64 * 1024];
        read_message(&mut *rx, inband, raw_len, &mut extractor, &mut buf);
        Instant::now()
    });
    tx.write_all(&wire).unwrap();
    let end = reader.join().unwrap();
    end - start
}

fn report_bulk() {
    println!("## 4. Bulk transfer (one large message, best of 5)\n");
    println!("| payload | direction | Unix socket | PTY + APC |");
    println!("|---|---|---:|---:|");
    let image = synthetic_image(512);
    let scene = synthetic_scene(2000);
    for (label, raw) in [
        ("512x512 RGBA image", &image),
        ("2,000-region scene", &scene),
    ] {
        for to_terminal in [true, false] {
            let best = |inband: bool| {
                (0..5)
                    .map(|_| {
                        let (a, b): (Box<dyn Duplex>, Box<dyn Duplex>) = if inband {
                            let (app, term) = raw_pty();
                            (Box::new(app), Box::new(term))
                        } else {
                            let (app, term) = UnixStream::pair().unwrap();
                            (Box::new(app), Box::new(term))
                        };
                        let (tx, rx) = if to_terminal { (a, b) } else { (b, a) };
                        bulk_once(tx, rx, inband, raw)
                    })
                    .min()
                    .unwrap()
            };
            let (sock, pty) = (best(false), best(true));
            let mib = raw.len() as f64 / (1024.0 * 1024.0);
            println!(
                "| {label} ({:.2} MiB) | {} | {:.2} ms ({:.0} MiB/s) | {:.2} ms ({:.0} MiB/s) |",
                mib,
                if to_terminal {
                    "app -> terminal"
                } else {
                    "terminal -> app"
                },
                sock.as_secs_f64() * 1e3,
                mib / sock.as_secs_f64(),
                pty.as_secs_f64() * 1e3,
                mib / pty.as_secs_f64(),
            );
        }
    }
    println!();
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: inband_transport <corpus.bin>");
    let records = load_corpus(&path);

    // The extractor must survive arbitrary read boundaries.
    for r in &records {
        let mut wire = Vec::new();
        wrap(&r.bytes, &mut wire);
        wire.splice(0..0, b"plain \x1b[1mtext\x1b[0m ".iter().copied());
        for split in [1, 3, 7, 64] {
            let mut ex = Extractor::default();
            let mut got = Vec::new();
            for chunk in wire.chunks(split) {
                ex.feed(chunk, |p| got = URL_SAFE_NO_PAD.decode(p).unwrap());
            }
            assert_eq!(got, r.bytes, "extractor split at {split}");
        }
    }

    report_sizes(&records);
    report_cpu(&records);
    report_latency(&records);
    report_bulk();
}
