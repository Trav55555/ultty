//! Drive the Kitty graphics protocol (ADR 0026) with structured commands
//! built from fuzz input, interleaved with text and resizes, and assert the
//! terminal stays consistent and every reply matches the reply grammar.
//!
//! The `vt_terminal` target reaches the APC framing but rarely builds a
//! command with a valid payload; this one always does. Each step reads an
//! op byte: a resize, a run of text, or a graphics command whose keys come
//! from a fixed set with fuzz values, and whose payload is fuzz bytes,
//! optionally zlib-compressed, always base64-encoded and split into chunks.
#![no_main]

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use libfuzzer_sys::fuzz_target;
use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

/// Keys a command may carry, with how their value is drawn.
const KEYS: &[(u8, Value)] = &[
    (b'a', Value::Of(b"tTqpdfac")),
    (b'i', Value::Small),
    (b'I', Value::Small),
    (b'p', Value::Small),
    (b'q', Value::Of(b"012")),
    (b'f', Value::Of(b"\x18\x20\x64\x08")),
    (b't', Value::Of(b"dfts")),
    (b'o', Value::Of(b"zx")),
    (b's', Value::Small),
    (b'v', Value::Small),
    (b'S', Value::Number),
    (b'x', Value::Small),
    (b'y', Value::Small),
    (b'w', Value::Small),
    (b'h', Value::Small),
    (b'X', Value::Small),
    (b'Y', Value::Small),
    (b'c', Value::Small),
    (b'r', Value::Small),
    (b'C', Value::Of(b"01")),
    (b'z', Value::Signed),
    (b'N', Value::Of(b"01")),
    (b'd', Value::Of(b"aAiInNcCpPqQrRxXyYzZfF")),
    (b'U', Value::Of(b"01")),
];

#[derive(Clone, Copy)]
enum Value {
    /// One character from the set; `f` maps bytes to formats.
    Of(&'static [u8]),
    /// 0 to 63.
    Small,
    /// Any `u32`.
    Number,
    /// Any `i32`, often near the z-layer thresholds.
    Signed,
}

struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }

    fn bytes(&mut self, n: usize) -> &[u8] {
        let (taken, rest) = self.0.split_at(n.min(self.0.len()));
        self.0 = rest;
        taken
    }

    fn u32(&mut self) -> u32 {
        let b = self.bytes(4);
        b.iter().fold(0, |acc, &byte| acc << 8 | u32::from(byte))
    }
}

fn value(input: &mut Input, key: u8, kind: Value) -> String {
    match kind {
        Value::Of(set) => {
            let pick = set[usize::from(input.byte().unwrap_or(0)) % set.len()];
            if key == b'f' {
                u32::from(pick).to_string()
            } else {
                char::from(pick).to_string()
            }
        }
        Value::Small => (input.byte().unwrap_or(0) % 64).to_string(),
        Value::Number => input.u32().to_string(),
        Value::Signed => match input.byte().unwrap_or(0) % 4 {
            0 => (i32::MIN / 2).to_string(),
            1 => (i32::MIN / 2 - 1).to_string(),
            2 => (input.u32() as i32).to_string(),
            _ => (i32::from(input.byte().unwrap_or(0) as i8)).to_string(),
        },
    }
}

/// One graphics command, as the APC strings that carry it.
fn command(input: &mut Input) -> Vec<u8> {
    let mask = input.u32();
    let mut control: Vec<String> = Vec::new();
    for (n, &(key, kind)) in KEYS.iter().enumerate() {
        if mask & (1 << n) != 0 {
            control.push(format!("{}={}", char::from(key), value(input, key, kind)));
        }
    }
    let len = usize::from(input.byte().unwrap_or(0)) * 4;
    let mut payload = input.bytes(len).to_vec();
    let flags = input.byte().unwrap_or(0);
    if flags & 1 != 0 {
        payload = miniz_oxide::deflate::compress_to_vec_zlib(&payload, 1);
    }
    if flags & 2 != 0 {
        // A small, valid image; the key choices decide what is done with it.
        payload = vec![0x80; 4 * 4 * 4];
    }
    let text = BASE64.encode(&payload);
    let chunk = (usize::from(flags >> 2) + 1) * 4;
    let pieces: Vec<&[u8]> = if text.is_empty() {
        vec![&[][..]]
    } else {
        text.as_bytes().chunks(chunk).collect()
    };
    let mut out = Vec::new();
    for (n, piece) in pieces.iter().enumerate() {
        let more = u8::from(n + 1 < pieces.len());
        let keys = if n == 0 {
            control.join(",")
        } else {
            String::new()
        };
        let sep = if keys.is_empty() { "" } else { "," };
        out.extend_from_slice(format!("\x1b_G{keys}{sep}m={more};").as_bytes());
        out.extend_from_slice(piece);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Whether `reply` is `ESC _ G i=N[,I=N][,p=N];OK ESC \` or the same with
/// `CODE:text` in place of `OK`, in printable ASCII.
fn well_formed(reply: &[u8]) -> bool {
    let Some(body) = reply
        .strip_prefix(b"\x1b_G")
        .and_then(|rest| rest.strip_suffix(b"\x1b\\"))
    else {
        return false;
    };
    let Some(at) = body.iter().position(|&b| b == b';') else {
        return false;
    };
    let (ids, message) = (&body[..at], &body[at + 1..]);
    let ids_ok = !ids.is_empty()
        && ids.split(|&b| b == b',').all(|field| match field {
            [b'i' | b'I' | b'p', b'=', digits @ ..] => {
                !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
            }
            _ => false,
        });
    let message_ok = message == b"OK"
        || message
            .iter()
            .position(|&b| b == b':')
            .is_some_and(|colon| {
                let (code, text) = (&message[..colon], &message[colon + 1..]);
                !code.is_empty()
                    && code.iter().all(u8::is_ascii_uppercase)
                    && text.iter().all(|&b| (0x20..0x7f).contains(&b))
            });
    ids_ok && message_ok
}

fn dims(cols: u8, rows: u8) -> Dimensions {
    Dimensions::new(u16::from(cols % 96) + 1, u16::from(rows % 32) + 1)
}

fuzz_target!(|data: &[u8]| {
    let mut input = Input(data);
    let (Some(cols), Some(rows), Some(quota)) = (input.byte(), input.byte(), input.byte()) else {
        return;
    };
    let mut terminal = Terminal::new(dims(cols, rows));
    terminal.set_cell_pixel_size(u16::from(cols % 16) + 1, u16::from(rows % 24) + 1);
    // A small quota makes eviction common; 0 turns images off.
    terminal.set_image_storage_limit(usize::from(quota) * 64);
    let mut parser = AnsiParser::new();
    while let Some(op) = input.byte() {
        let bytes = match op % 8 {
            0 => {
                let (c, r) = (input.byte().unwrap_or(0), input.byte().unwrap_or(0));
                terminal.resize(dims(c, r));
                Vec::new()
            }
            1 => {
                let len = usize::from(input.byte().unwrap_or(0) % 64);
                input.bytes(len).to_vec()
            }
            2 => b"\x1b[2J\x1b[?1049h\x1b[3J\x1b[?1049l"[..usize::from(op >> 3) % 25].to_vec(),
            _ => command(&mut input),
        };
        let mut handler = TerminalHandler::new(&mut terminal);
        parser.feed(&bytes, |action| handler.handle(action));
        if let Err(violation) = terminal.check_integrity() {
            panic!("integrity violation: {violation}");
        }
        for reply in terminal.take_responses() {
            if reply.starts_with(b"\x1b_G") {
                assert!(well_formed(&reply), "malformed reply {reply:?}");
            }
        }
    }
});
