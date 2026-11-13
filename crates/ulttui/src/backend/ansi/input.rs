//! Terminal input for the ANSI backend: read every byte that is there, decode
//! it into events.
//!
//! crossterm's reader takes at most 1 KiB per readiness edge and its mio
//! registration is edge-triggered, so after a larger burst (a paste) the rest
//! sat in the kernel until the next keypress while the event loop's
//! level-triggered wait saw the terminal readable forever. Reading here,
//! until the read would block, keeps the wait and the reads in step.
//!
//! The decoding follows crossterm 0.29's rules for the input the backend
//! asks for: keys (legacy and `CSI 1;mods X` encodings) and mouse reports
//! (SGR, urxvt and X10).

use std::collections::VecDeque;
use std::io;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use crate::event::{Event, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind};

const ESC: u8 = 0x1b;
/// An unfinished sequence longer than this is garbage: drop a byte and go on.
const MAX_PENDING: usize = 256;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
/// Maximum accepted bracketed paste in bytes. Oversized pastes are discarded
/// through their closing marker so their contents cannot become key commands.
const MAX_PASTE: usize = 1024 * 1024;

/// Reads a terminal's input and turns it into events.
#[derive(Default)]
pub(super) struct InputReader {
    decoder: Decoder,
    events: VecDeque<Event>,
    escape_deadline: Option<Instant>,
}

impl InputReader {
    /// Read all input available on `fd` now, without waiting, and decode it.
    /// Returns false once the terminal has hung up.
    pub(super) fn read_available(&mut self, fd: RawFd) -> io::Result<bool> {
        let mut buf = [0u8; 4096];
        let mut open = true;
        while crate::wake::poll_readable(&[fd], Some(Duration::ZERO))? {
            // SAFETY: `buf` is valid for `buf.len()` bytes; `fd` is open.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                match err.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => break,
                    // A pty whose other end has closed reads EIO.
                    _ if err.raw_os_error() == Some(libc::EIO) => {
                        open = false;
                        break;
                    }
                    _ => return Err(err),
                }
            }
            if n == 0 {
                open = false;
                break;
            }
            self.decoder.feed(&buf[..n as usize], &mut self.events);
        }
        // Nothing more has arrived: a lone ESC is the Escape key, not the
        // start of a sequence.
        if self.decoder.pending == [ESC] && self.decoder.paste.is_none() {
            let deadline = *self
                .escape_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(30));
            if Instant::now() >= deadline {
                self.decoder.finish_escape(&mut self.events);
                self.escape_deadline = None;
            }
        } else {
            self.escape_deadline = None;
        }
        Ok(open)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.escape_deadline
    }

    pub(super) fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub(super) fn has_events(&self) -> bool {
        !self.events.is_empty()
    }
}

/// Bytes to events, keeping an unfinished sequence for the next bytes.
#[derive(Default)]
pub(super) struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
    paste_overflow: bool,
}

/// One step of decoding the front of the buffer.
enum Step {
    /// `usize` bytes make up one input, which may not be one we report.
    Done(usize, Option<Event>),
    /// The bytes so far are the start of a sequence.
    Incomplete,
}

impl Decoder {
    pub(super) fn feed(&mut self, bytes: &[u8], out: &mut VecDeque<Event>) {
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        while start < self.pending.len() {
            let remaining = &self.pending[start..];
            if self.paste.is_some() {
                let end = remaining
                    .windows(PASTE_END.len())
                    .position(|w| w == PASTE_END);
                let take = end.unwrap_or_else(|| {
                    // Retain a possible partial end marker across reads.
                    let keep = (1..PASTE_END.len())
                        .rev()
                        .find(|&n| remaining.ends_with(&PASTE_END[..n]))
                        .unwrap_or(0);
                    remaining.len() - keep
                });
                if let Some(paste) = &mut self.paste {
                    if !self.paste_overflow && paste.len().saturating_add(take) <= MAX_PASTE {
                        paste.extend_from_slice(&remaining[..take]);
                    } else {
                        paste.clear();
                        self.paste_overflow = true;
                    }
                }
                start += take;
                if end.is_some() {
                    start += PASTE_END.len();
                    if let Some(paste) = self.paste.take() {
                        if !self.paste_overflow {
                            out.push_back(Event::Paste {
                                text: String::from_utf8_lossy(&paste).into_owned(),
                                focused_id: None,
                            });
                        }
                    }
                    self.paste_overflow = false;
                    continue;
                }
                break;
            }
            if remaining.starts_with(PASTE_START) {
                self.paste = Some(Vec::new());
                start += PASTE_START.len();
                continue;
            }
            if PASTE_START.starts_with(remaining) {
                break;
            }
            match parse(remaining) {
                Step::Done(len, event) => {
                    out.extend(event);
                    start += len;
                }
                Step::Incomplete if remaining.len() > MAX_PENDING => start += 1,
                Step::Incomplete => break,
            }
        }
        self.pending.drain(..start);
    }

    /// No more input is waiting: resolve a lone ESC as the Escape key.
    pub(super) fn finish_escape(&mut self, out: &mut VecDeque<Event>) {
        if self.paste.is_none() && self.pending == [ESC] {
            self.pending.clear();
            out.push_back(key(Key::Escape, Modifiers::default()));
        }
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key(KeyEvent {
        key,
        modifiers,
        focused_id: None,
    })
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::default()
    }
}

fn parse(buf: &[u8]) -> Step {
    match buf[0] {
        ESC => parse_escape(buf),
        b'\r' => Step::Done(1, Some(key(Key::Enter, Modifiers::default()))),
        b'\t' => Step::Done(1, Some(key(Key::Tab, Modifiers::default()))),
        0x7f => Step::Done(1, Some(key(Key::Backspace, Modifiers::default()))),
        // Ctrl+C quits, as it did through crossterm.
        0x03 => Step::Done(1, Some(Event::Quit)),
        // In raw mode \n is Ctrl+J; the terminal sends \r for Enter.
        c @ 0x01..=0x1a => Step::Done(1, Some(key(Key::Char((c - 1 + b'a') as char), ctrl()))),
        c @ 0x1c..=0x1f => Step::Done(1, Some(key(Key::Char((c - 0x1c + b'4') as char), ctrl()))),
        0x00 => Step::Done(1, Some(key(Key::Char(' '), ctrl()))),
        _ => parse_char(buf),
    }
}

fn parse_char(buf: &[u8]) -> Step {
    let len = match buf[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A stray continuation byte or an invalid lead byte.
        _ => return Step::Done(1, None),
    };
    if buf.len() < len {
        return Step::Incomplete;
    }
    match std::str::from_utf8(&buf[..len])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(c) => {
            let modifiers = Modifiers {
                shift: c.is_uppercase(),
                ..Modifiers::default()
            };
            Step::Done(len, Some(key(Key::Char(c), modifiers)))
        }
        None => Step::Done(1, None),
    }
}

fn parse_escape(buf: &[u8]) -> Step {
    let Some(&second) = buf.get(1) else {
        return Step::Incomplete;
    };
    match second {
        b'[' => parse_csi(buf),
        b'O' => match buf.get(2) {
            None => Step::Incomplete,
            Some(&last) => Step::Done(3, legacy_key(last).map(|k| key(k, Modifiers::default()))),
        },
        ESC => Step::Done(2, Some(key(Key::Escape, Modifiers::default()))),
        // ESC before a key is Alt+that key.
        _ => match parse(&buf[1..]) {
            Step::Incomplete => Step::Incomplete,
            Step::Done(len, event) => Step::Done(len + 1, event.map(with_alt)),
        },
    }
}

fn with_alt(event: Event) -> Event {
    match event {
        Event::Key(mut key_event) => {
            key_event.modifiers.alt = true;
            Event::Key(key_event)
        }
        other => other,
    }
}

/// The key a final byte names in `ESC O x` or `CSI [1;mods] x`.
fn legacy_key(last: u8) -> Option<Key> {
    Some(match last {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'P'..=b'S' => Key::F(1 + last - b'P'),
        _ => return None,
    })
}

/// The key `CSI n ~` names.
fn tilde_key(n: u16) -> Option<Key> {
    Some(match n {
        1 | 7 => Key::Home,
        2 => Key::Insert,
        3 => Key::Delete,
        4 | 8 => Key::End,
        5 => Key::PageUp,
        6 => Key::PageDown,
        11..=15 => Key::F((n - 10) as u8),
        17..=21 => Key::F((n - 11) as u8),
        23..=26 => Key::F((n - 12) as u8),
        // As crossterm numbers them (29 and 31 are both F14).
        28..=29 => Key::F((n - 15) as u8),
        31..=34 => Key::F((n - 17) as u8),
        _ => return None,
    })
}

/// xterm's modifier parameter: 1 + a bit mask.
fn key_modifiers(param: u16) -> Modifiers {
    let mask = param.saturating_sub(1);
    Modifiers {
        shift: mask & 1 != 0,
        alt: mask & 2 != 0,
        ctrl: mask & 4 != 0,
        meta: mask & 32 != 0,
    }
}

fn parse_csi(buf: &[u8]) -> Step {
    // The Linux console's F1-F5: CSI [ A..E.
    if buf.get(2) == Some(&b'[') {
        return match buf.get(3) {
            None => Step::Incomplete,
            Some(&last @ b'A'..=b'E') => {
                Step::Done(4, Some(key(Key::F(1 + last - b'A'), Modifiers::default())))
            }
            Some(_) => Step::Done(4, None),
        };
    }
    // X10 mouse: CSI M and three raw bytes, which may be anything.
    if buf.get(2) == Some(&b'M') {
        if buf.len() < 6 {
            return Step::Incomplete;
        }
        let event = mouse(
            u16::from(buf[3]).saturating_sub(32),
            u16::from(buf[4]).saturating_sub(32),
            u16::from(buf[5]).saturating_sub(32),
            false,
        );
        return Step::Done(6, event);
    }
    // Parameter and intermediate bytes, then one final byte.
    let Some(end) = buf[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
        return Step::Incomplete;
    };
    let end = end + 2;
    let len = end + 1;
    let final_byte = buf[end];
    let body = &buf[2..end];
    let sgr = body.first() == Some(&b'<');
    let params: Vec<u16> = std::str::from_utf8(if sgr { &body[1..] } else { body })
        .unwrap_or("")
        .split(';')
        .map(|p| p.split(':').next().unwrap_or("").parse().unwrap_or(0))
        .collect();
    let param = |i: usize| params.get(i).copied().unwrap_or(0);

    let event = match final_byte {
        b'M' | b'm' if sgr => mouse(param(0), param(1), param(2), final_byte == b'm'),
        // urxvt mouse: the button is offset by 32.
        b'M' if params.len() == 3 => mouse(param(0).saturating_sub(32), param(1), param(2), false),
        b'Z' => Some(key(
            Key::Tab,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        )),
        b'~' => tilde_key(param(0)).map(|k| key(k, key_modifiers(param(1)))),
        // A cursor position report (CSI row;col R), never F3: crossterm
        // reads every CSI .. R as one too. F3 arrives as ESC O R.
        b'R' => None,
        // Focus reports and anything else the backend does not use.
        _ => legacy_key(final_byte).map(|k| key(k, key_modifiers(param(1)))),
    };
    Step::Done(len, event)
}

/// A mouse report: `code` is the button and flags, `x`/`y` 1-based cells.
fn mouse(code: u16, x: u16, y: u16, released: bool) -> Option<Event> {
    // Buttons 8 and up (back, forward, ...): not reported, as in crossterm.
    if code >= 128 {
        return None;
    }
    let modifiers = Modifiers {
        shift: code & 4 != 0,
        alt: code & 8 != 0,
        ctrl: code & 16 != 0,
        meta: false,
    };
    let button = match code & 3 {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    };
    let kind = if code & 64 != 0 && code & 32 != 0 {
        // Motion with a wheel "button" held.
        MouseEventKind::Moved
    } else if code & 64 != 0 {
        match code & 3 {
            0 => MouseEventKind::ScrollUp,
            1 => MouseEventKind::ScrollDown,
            // Horizontal scrolling is not an event here.
            _ => return None,
        }
    } else if code & 32 != 0 {
        match button {
            Some(button) => MouseEventKind::Drag(button),
            None => MouseEventKind::Moved,
        }
    } else if released {
        MouseEventKind::Up(button.unwrap_or(MouseButton::Left))
    } else {
        match button {
            Some(button) => MouseEventKind::Down(button),
            // X10 and urxvt report any release as button 3.
            None => MouseEventKind::Up(MouseButton::Left),
        }
    };
    Some(Event::Mouse(MouseEvent {
        kind,
        x: x.saturating_sub(1),
        y: y.saturating_sub(1),
        modifiers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Vec<Event> {
        let mut decoder = Decoder::default();
        let mut out = VecDeque::new();
        decoder.feed(bytes, &mut out);
        decoder.finish_escape(&mut out);
        out.into()
    }

    fn keys(bytes: &[u8]) -> Vec<(Key, Modifiers)> {
        decode(bytes)
            .into_iter()
            .map(|e| match e {
                Event::Key(k) => (k.key, k.modifiers),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn plain(k: Key) -> (Key, Modifiers) {
        (k, Modifiers::default())
    }

    fn mods(shift: bool, alt: bool, ctrl: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            ctrl,
            meta: false,
        }
    }

    #[test]
    fn bracketed_paste_preserves_split_markers_utf8_and_controls() {
        let bytes = "\x1b[200~hé\r\n\x03\x1b[A\x1b[201~q".as_bytes();
        for split in 0..=bytes.len() {
            let mut decoder = Decoder::default();
            let mut events = VecDeque::new();
            decoder.feed(&bytes[..split], &mut events);
            decoder.feed(&bytes[split..], &mut events);
            assert!(
                matches!(events.pop_front(), Some(Event::Paste { text, .. }) if text == "hé\r\n\x03\x1b[A"),
                "split {split}"
            );
            assert!(matches!(
                events.pop_front(),
                Some(Event::Key(KeyEvent {
                    key: Key::Char('q'),
                    ..
                }))
            ));
            assert!(events.is_empty());
        }
    }

    #[test]
    fn oversized_and_unclosed_pastes_never_emit_keys() {
        let mut decoder = Decoder::default();
        let mut events = VecDeque::new();
        decoder.feed(PASTE_START, &mut events);
        decoder.feed(&vec![b'x'; MAX_PASTE + 1], &mut events);
        decoder.feed(b"\x03\r\x1b[20", &mut events);
        decoder.finish_escape(&mut events);
        assert!(events.is_empty());
        decoder.feed(b"1~q", &mut events);
        assert!(matches!(
            events.pop_front(),
            Some(Event::Key(KeyEvent {
                key: Key::Char('q'),
                ..
            }))
        ));
        assert!(events.is_empty());
    }

    #[test]
    fn text_and_control_bytes() {
        assert_eq!(
            keys("aZé\r\t\x7f".as_bytes()),
            vec![
                plain(Key::Char('a')),
                (Key::Char('Z'), mods(true, false, false)),
                plain(Key::Char('é')),
                plain(Key::Enter),
                plain(Key::Tab),
                plain(Key::Backspace),
            ]
        );
        assert_eq!(
            keys(b"\x01\n\x00\x1c"),
            vec![
                (Key::Char('a'), ctrl()),
                (Key::Char('j'), ctrl()),
                (Key::Char(' '), ctrl()),
                (Key::Char('4'), ctrl()),
            ]
        );
        assert!(matches!(decode(b"\x03")[..], [Event::Quit]));
    }

    #[test]
    fn escape_sequences_for_keys() {
        assert_eq!(
            keys(b"\x1b[A\x1b[B\x1b[C\x1b[D\x1bOH\x1b[F\x1bOP\x1b[2~\x1b[3~\x1b[5~\x1b[6~\x1b[15~\x1b[24~"),
            vec![
                plain(Key::Up),
                plain(Key::Down),
                plain(Key::Right),
                plain(Key::Left),
                plain(Key::Home),
                plain(Key::End),
                plain(Key::F(1)),
                plain(Key::Insert),
                plain(Key::Delete),
                plain(Key::PageUp),
                plain(Key::PageDown),
                plain(Key::F(5)),
                plain(Key::F(12)),
            ]
        );
        assert_eq!(
            keys(b"\x1b[1;5A\x1b[3;2~\x1b[Z\x1bx\x1b\x1b"),
            vec![
                (Key::Up, mods(false, false, true)),
                (Key::Delete, mods(true, false, false)),
                (Key::Tab, mods(true, false, false)),
                (Key::Char('x'), mods(false, true, false)),
                plain(Key::Escape),
            ]
        );
    }

    #[test]
    fn a_lone_escape_waits_only_while_more_input_is_coming() {
        let mut decoder = Decoder::default();
        let mut out = VecDeque::new();
        decoder.feed(b"\x1b", &mut out);
        assert!(out.is_empty(), "could be the start of a sequence");
        decoder.feed(b"[A", &mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        assert_eq!(keys(b"\x1b"), vec![plain(Key::Escape)]);
    }

    #[test]
    fn sequences_and_characters_split_across_reads() {
        let input = "x\x1b[1;5Cé€".as_bytes();
        for split in 0..=input.len() {
            let mut decoder = Decoder::default();
            let mut out = VecDeque::new();
            decoder.feed(&input[..split], &mut out);
            decoder.feed(&input[split..], &mut out);
            let got: Vec<_> = out
                .into_iter()
                .map(|e| match e {
                    Event::Key(k) => k.key,
                    other => panic!("{other:?}"),
                })
                .collect();
            assert_eq!(
                got,
                vec![Key::Char('x'), Key::Right, Key::Char('é'), Key::Char('€')],
                "split at {split}"
            );
        }
    }

    #[test]
    fn mouse_reports() {
        let events = decode(b"\x1b[<0;10;5M\x1b[<0;10;5m\x1b[<64;1;1M\x1b[<65;1;1M\x1b[<34;3;4M\x1b[<35;3;4M\x1b[M !!");
        let kinds: Vec<_> = events
            .iter()
            .map(|e| match e {
                Event::Mouse(m) => (m.kind, m.x, m.y),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                (MouseEventKind::Down(MouseButton::Left), 9, 4),
                (MouseEventKind::Up(MouseButton::Left), 9, 4),
                (MouseEventKind::ScrollUp, 0, 0),
                (MouseEventKind::ScrollDown, 0, 0),
                (MouseEventKind::Drag(MouseButton::Right), 2, 3),
                (MouseEventKind::Moved, 2, 3),
                (MouseEventKind::Down(MouseButton::Left), 0, 0),
            ]
        );
    }

    #[test]
    fn linux_console_and_high_function_keys() {
        assert_eq!(
            keys(b"\x1b[[A\x1b[[E\x1b[28~\x1b[29~\x1b[31~\x1b[34~q"),
            vec![
                plain(Key::F(1)),
                plain(Key::F(5)),
                plain(Key::F(13)),
                plain(Key::F(14)),
                plain(Key::F(14)),
                plain(Key::F(17)),
                plain(Key::Char('q')),
            ]
        );
        // Not stray letters: an unknown CSI [ x is dropped whole.
        assert_eq!(keys(b"\x1b[[Zq"), vec![plain(Key::Char('q'))]);
    }

    #[test]
    fn reports_the_backend_does_not_use_are_dropped() {
        // A cursor position report, not F3 with modifiers; F3 is ESC O R.
        assert_eq!(
            keys(b"\x1b[12;40R\x1bORq"),
            vec![plain(Key::F(3)), plain(Key::Char('q'))]
        );
        // Back/forward buttons (128+) and horizontal wheel: no event.
        assert!(decode(b"\x1b[<128;1;1M\x1b[<129;1;1m\x1b[<66;1;1M").is_empty());
        // Wheel held while moving is motion.
        assert!(matches!(
            decode(b"\x1b[<96;2;2M")[..],
            [Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                ..
            })]
        ));
    }

    #[test]
    fn garbage_does_not_stall_the_decoder() {
        let mut input = b"\x1b[".to_vec();
        input.extend(std::iter::repeat_n(b'1', MAX_PENDING + 10));
        input.extend(b"\x1b[Aq");
        let got = decode(&input);
        assert!(matches!(
            got.last(),
            Some(Event::Key(KeyEvent {
                key: Key::Char('q'),
                ..
            }))
        ));
    }

    #[test]
    fn the_reader_takes_everything_that_is_waiting() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let (mut writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let burst = vec![b'a'; 10_000];
        writer.write_all(&burst).unwrap();
        let mut input = InputReader::default();
        assert!(input.read_available(reader.as_raw_fd()).unwrap());
        let mut count = 0;
        while input.next_event().is_some() {
            count += 1;
        }
        assert_eq!(count, burst.len());
        assert!(!crate::wake::poll_readable(&[reader.as_raw_fd()], Some(Duration::ZERO)).unwrap());
        drop(writer);
        assert!(
            !input.read_available(reader.as_raw_fd()).unwrap(),
            "hang-up"
        );
    }
}
