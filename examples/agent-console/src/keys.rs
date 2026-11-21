//! Keys for answering an approval.
//!
//! Over the socket and in ANSI mode the app reads its own terminal, in raw
//! mode while it waits ([`TerminalKeys`]). In-band (Ultty over ssh) the
//! inline console reads the tty for the terminal's replies and keeps the
//! keys typed meanwhile, so they arrive as bytes ([`InBandKeys`]).

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};

/// A key that means something to an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Move the selection up.
    Up,
    /// Move the selection down.
    Down,
    /// Take the selected choice.
    Enter,
    /// Take the choice with this 1-based number.
    Digit(usize),
    /// Ctrl-C: stop the replay.
    Interrupt,
    /// Anything else.
    Other,
}

/// Where an approval reads keys from.
pub trait Keys {
    /// The next key, waiting at most `timeout`. `input` holds the bytes the
    /// terminal session read on the app's behalf since the last call.
    fn next_key(&mut self, input: &[u8], timeout: Duration) -> io::Result<Option<Key>>;

    /// The approval is answered; stop reading keys until the next one.
    fn done(&mut self) {}
}

/// The keys in `bytes`, as a terminal in raw mode sends them.
pub fn parse(bytes: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (key, len) = match bytes[i..] {
            [0x1b, b'[' | b'O', b'A', ..] => (Key::Up, 3),
            [0x1b, b'[' | b'O', b'B', ..] => (Key::Down, 3),
            [b'\r' | b'\n', ..] => (Key::Enter, 1),
            [0x03, ..] => (Key::Interrupt, 1),
            [b'k', ..] => (Key::Up, 1),
            [b'j', ..] => (Key::Down, 1),
            [d @ b'1'..=b'9', ..] => (Key::Digit(usize::from(d - b'0')), 1),
            _ => (Key::Other, 1),
        };
        keys.push(key);
        i += len;
    }
    keys
}

/// Keys from the bytes an in-band session kept, oldest first.
#[derive(Debug, Default)]
pub struct InBandKeys {
    pending: Vec<Key>,
}

impl Keys for InBandKeys {
    fn next_key(&mut self, input: &[u8], timeout: Duration) -> io::Result<Option<Key>> {
        self.pending.extend(parse(input));
        if self.pending.is_empty() {
            std::thread::sleep(timeout);
            return Ok(None);
        }
        Ok(Some(self.pending.remove(0)))
    }
}

/// Keys from this process's terminal, read in raw mode from the first
/// key asked for until the approval is answered.
#[derive(Debug, Default)]
pub struct TerminalKeys {
    raw: Option<RawMode>,
}

/// Raw mode for as long as it lives.
#[derive(Debug)]
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Err(err) = crossterm::terminal::disable_raw_mode() {
            tracing::warn!("could not leave raw mode: {err}");
        }
    }
}

impl Keys for TerminalKeys {
    fn next_key(&mut self, _input: &[u8], timeout: Duration) -> io::Result<Option<Key>> {
        if self.raw.is_none() {
            self.raw = Some(RawMode::enable()?);
        }
        if !event::poll(timeout)? {
            return Ok(None);
        }
        let TermEvent::Key(key) = event::read()? else {
            return Ok(Some(Key::Other));
        };
        if key.kind == KeyEventKind::Release {
            return Ok(Some(Key::Other));
        }
        Ok(Some(match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Key::Interrupt,
            KeyCode::Up | KeyCode::Char('k') => Key::Up,
            KeyCode::Down | KeyCode::Char('j') => Key::Down,
            KeyCode::Enter => Key::Enter,
            KeyCode::Char(d @ '1'..='9') => Key::Digit(d as usize - '0' as usize),
            _ => Key::Other,
        }))
    }

    fn done(&mut self) {
        self.raw = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_reads_arrows_digits_enter_and_ctrl_c() {
        assert_eq!(
            parse(b"\x1b[A\x1bOBj2\rx\x03"),
            [
                Key::Up,
                Key::Down,
                Key::Down,
                Key::Digit(2),
                Key::Enter,
                Key::Other,
                Key::Interrupt
            ]
        );
    }

    #[test]
    fn test_in_band_keys_come_out_one_at_a_time() {
        let mut keys = InBandKeys::default();
        assert_eq!(
            keys.next_key(b"1\r", Duration::ZERO).unwrap(),
            Some(Key::Digit(1))
        );
        assert_eq!(
            keys.next_key(b"", Duration::ZERO).unwrap(),
            Some(Key::Enter)
        );
        assert_eq!(keys.next_key(b"", Duration::ZERO).unwrap(), None);
    }
}
