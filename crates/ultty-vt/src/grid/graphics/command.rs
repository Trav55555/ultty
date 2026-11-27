//! The control data of a Kitty graphics command, parsed into typed keys.
//!
//! A command is `ESC _ G <control data> ; <payload> ESC \`. The control
//! data is a comma-separated list of `key=value` pairs, each key one ASCII
//! letter (spec, "The graphics escape code" and "Control data reference").
//! Unknown keys are ignored. A value that does not parse marks the command
//! malformed; the keys around it still parse, so that the reply can carry
//! the client's ids.

use super::reply::Error;

/// What a command does (`a=`). The spec's default is `t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `t`: transmit and store.
    Transmit,
    /// `T`: transmit and place.
    TransmitAndPlace,
    /// `q`: decode and validate, reply, store nothing.
    Query,
    /// `p`: place an image sent earlier.
    Place,
    /// `d`: delete placements or images.
    Delete,
    /// `f`, `a` or `c`: animation, refused in slice 1.
    Animation,
    /// Any other letter.
    Unknown,
}

/// One bit per control key, for remembering which keys a command set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Keys(u64);

impl Keys {
    fn bit(key: u8) -> u64 {
        match key {
            b'a'..=b'z' => 1 << (key - b'a'),
            b'A'..=b'Z' => 1 << (26 + key - b'A'),
            _ => 0,
        }
    }

    fn insert(&mut self, key: u8) {
        self.0 |= Self::bit(key);
    }

    /// Whether the command set `key`.
    pub fn contains(self, key: u8) -> bool {
        self.0 & Self::bit(key) != 0
    }

    /// Whether the command set no key outside `allowed`.
    pub fn only(self, allowed: &[u8]) -> bool {
        let mask = allowed.iter().fold(0, |mask, &key| mask | Self::bit(key));
        self.0 & !mask == 0
    }
}

/// A parsed graphics command. Numeric keys the command did not set hold
/// the spec's default, 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// `a`
    pub verb: Verb,
    /// `q`: 1 suppresses `OK` replies, 2 suppresses errors too.
    pub quiet: u8,
    /// `i`: the client's image id.
    pub image_id: u32,
    /// `I`: the client's image number.
    pub image_number: u32,
    /// `p`: the placement id.
    pub placement_id: u32,
    /// `f`: 24, 32 (the default) or 100.
    pub format: u32,
    /// `t`: the transmission medium, `d` by default.
    pub medium: u8,
    /// `o`: the compression, `z` or none.
    pub compression: Option<u8>,
    /// `m`: whether more chunks follow.
    pub more: bool,
    /// `s`, `v`: the pixel width and height of RGB and RGBA data.
    pub width: u32,
    pub height: u32,
    /// `S`: the size of PNG data before compression.
    pub size: u32,
    /// `x`, `y`, `w`, `h`: the source rectangle in pixels; for a delete,
    /// `x` and `y` are a cell, counted from 1.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// `X`, `Y`: the pixel offset inside the first cell.
    pub cell_x: u32,
    pub cell_y: u32,
    /// `c`, `r`: the display size in cells; 0 is the image's own size.
    pub cols: u32,
    pub rows: u32,
    /// `C`: 1 leaves the cursor where it is after a placement.
    pub cursor_policy: u32,
    /// `z`: the stacking order relative to text.
    pub z: i32,
    /// `N`: usage hints; bit 0 is "transient".
    pub hints: u32,
    /// `d`: what to delete, `a` by default.
    pub delete: u8,
    /// `U` is non-zero: a virtual placement for Unicode placeholders (`U+10EEEE`).
    pub virtual_placement: bool,
    /// `P`, `Q`, `H` or `V` is non-zero: a relative placement, refused in
    /// slice 1.
    pub relative: bool,
    /// Which keys the command set.
    pub keys: Keys,
    /// Whether a value did not parse.
    pub malformed: bool,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            verb: Verb::Transmit,
            quiet: 0,
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            format: 32,
            medium: b'd',
            compression: None,
            more: false,
            width: 0,
            height: 0,
            size: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            cell_x: 0,
            cell_y: 0,
            cols: 0,
            rows: 0,
            cursor_policy: 0,
            z: 0,
            hints: 0,
            delete: b'a',
            virtual_placement: false,
            relative: false,
            keys: Keys::default(),
            malformed: false,
        }
    }
}

/// Split a graphics string (the bytes after `G`) into its parsed control
/// data and its payload, the base64 text after the first `;`.
pub fn parse(string: &[u8]) -> (Command, &[u8]) {
    let (control, payload) = match string.iter().position(|&b| b == b';') {
        Some(at) => (&string[..at], &string[at + 1..]),
        None => (string, &[][..]),
    };
    let mut command = Command::default();
    for pair in control.split(|&b| b == b',') {
        if pair.is_empty() {
            continue;
        }
        if !command.set(pair) {
            command.malformed = true;
        }
    }
    (command, payload)
}

impl Command {
    /// Apply one `key=value` pair; `false` if it is malformed.
    fn set(&mut self, pair: &[u8]) -> bool {
        let [key, b'=', value @ ..] = pair else {
            return false;
        };
        let key = *key;
        if !key.is_ascii_alphabetic() || value.is_empty() {
            return false;
        }
        self.keys.insert(key);
        let unsigned = || parse_u32(value);
        let ok = match key {
            b'a' => single(value).map(|v| {
                self.verb = match v {
                    b't' => Verb::Transmit,
                    b'T' => Verb::TransmitAndPlace,
                    b'q' => Verb::Query,
                    b'p' => Verb::Place,
                    b'd' => Verb::Delete,
                    b'f' | b'a' | b'c' => Verb::Animation,
                    _ => Verb::Unknown,
                };
            }),
            b't' => single(value).map(|v| self.medium = v),
            b'o' => single(value).map(|v| self.compression = Some(v)),
            b'd' => single(value).map(|v| self.delete = v),
            // Only whether `e` is present matters: feature queries are
            // refused.
            b'e' => single(value).map(drop),
            b'q' => unsigned().map(|v| self.quiet = v.min(2) as u8),
            b'm' => unsigned().map(|v| self.more = v == 1),
            b'z' => parse_i32(value).map(|v| self.z = v),
            b'H' | b'V' => parse_i32(value).map(|v| self.relative |= v != 0),
            b'i' | b'I' | b'p' | b'f' | b's' | b'v' | b'S' | b'x' | b'y' | b'w' | b'h' | b'X'
            | b'Y' | b'c' | b'r' | b'C' | b'N' | b'U' | b'P' | b'Q' | b'O' => {
                let Some(v) = unsigned() else {
                    return false;
                };
                match key {
                    b'i' => self.image_id = v,
                    b'I' => self.image_number = v,
                    b'p' => self.placement_id = v,
                    b'f' => self.format = v,
                    b's' => self.width = v,
                    b'v' => self.height = v,
                    b'S' => self.size = v,
                    b'x' => self.x = v,
                    b'y' => self.y = v,
                    b'w' => self.w = v,
                    b'h' => self.h = v,
                    b'X' => self.cell_x = v,
                    b'Y' => self.cell_y = v,
                    b'c' => self.cols = v,
                    b'r' => self.rows = v,
                    b'C' => self.cursor_policy = v,
                    b'N' => self.hints = v,
                    b'U' => self.virtual_placement = v != 0,
                    b'P' | b'Q' => self.relative |= v != 0,
                    // `O` is for file media, which are refused.
                    _ => {}
                }
                Some(())
            }
            // Unknown keys are ignored, whatever their value.
            _ => Some(()),
        };
        ok.is_some()
    }

    /// The error for a command that slice 1 refuses or that is malformed,
    /// checked before anything else is done with it. `None` for a command
    /// to carry out.
    pub fn refusal(&self) -> Option<Error> {
        if self.malformed {
            return Some(Error::MALFORMED);
        }
        if self.image_id != 0 && self.image_number != 0 {
            return Some(Error::ID_AND_NUMBER);
        }
        match self.verb {
            Verb::Animation => return Some(Error::ANIMATION),
            Verb::Unknown => return Some(Error::UNKNOWN_ACTION),
            _ => {}
        }
        if self.keys.contains(b'e') {
            return Some(Error::FEATURE_QUERY);
        }
        if matches!(self.verb, Verb::Delete) {
            return None;
        }
        let transmits = matches!(
            self.verb,
            Verb::Transmit | Verb::TransmitAndPlace | Verb::Query
        );
        if transmits {
            match self.medium {
                b'd' => {}
                b'f' | b't' | b's' => return Some(Error::MEDIUM),
                _ => return Some(Error::UNKNOWN_MEDIUM),
            }
            match self.format {
                24 | 32 | 100 => {}
                8 | 16 => return Some(Error::INDEXED),
                _ => return Some(Error::UNKNOWN_FORMAT),
            }
            if self.compression.is_some_and(|c| c != b'z') {
                return Some(Error::UNKNOWN_COMPRESSION);
            }
        }
        let places = matches!(self.verb, Verb::TransmitAndPlace | Verb::Place);
        if places && self.relative {
            return Some(Error::RELATIVE);
        }
        None
    }
}

/// The value of a one-character key.
fn single(value: &[u8]) -> Option<u8> {
    match value {
        [v] => Some(*v),
        _ => None,
    }
}

/// A decimal `u32`, as the spec's "positive integer" keys take.
fn parse_u32(value: &[u8]) -> Option<u32> {
    if value.is_empty() || value.len() > 10 || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

/// A decimal `i32`, optionally negative.
fn parse_i32(value: &[u8]) -> Option<i32> {
    let digits = value.strip_prefix(b"-").unwrap_or(value);
    if digits.is_empty() || digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_spec() {
        let (command, payload) = parse(b"");
        assert_eq!(command, Command::default());
        assert_eq!(command.verb, Verb::Transmit);
        assert_eq!((command.format, command.medium), (32, b'd'));
        assert!(payload.is_empty());
    }

    #[test]
    fn keys_and_payload_are_split_at_the_first_semicolon() {
        let (command, payload) =
            parse(b"a=T,f=100,i=7,p=3,q=2,m=1,z=-5,c=10,r=4,X=2,Y=3,C=1,N=1;QUJD;x");
        assert_eq!(command.verb, Verb::TransmitAndPlace);
        assert_eq!(
            (command.format, command.image_id, command.placement_id),
            (100, 7, 3)
        );
        assert_eq!((command.quiet, command.more, command.z), (2, true, -5));
        assert_eq!(
            (command.cols, command.rows, command.cell_x, command.cell_y),
            (10, 4, 2, 3)
        );
        assert_eq!((command.cursor_policy, command.hints), (1, 1));
        assert_eq!(payload, b"QUJD;x");
        assert!(!command.malformed);
    }

    #[test]
    fn unknown_keys_and_empty_items_are_ignored() {
        let (command, _) = parse(b",i=4,,k=9,K=word,");
        assert_eq!(command.image_id, 4);
        assert!(!command.malformed);
    }

    #[test]
    fn a_bad_value_marks_the_command_malformed_but_keeps_its_ids() {
        for control in [
            &b"i=5,s=abc"[..],
            b"i=5,s=",
            b"i=5,s",
            b"i=5,a=tt",
            b"i=5,z=1x",
        ] {
            let (command, _) = parse(control);
            assert!(command.malformed, "{control:?}");
            assert_eq!(command.image_id, 5);
            assert_eq!(command.refusal(), Some(Error::MALFORMED));
        }
    }

    #[test]
    fn numbers_past_32_bits_are_malformed() {
        assert!(parse(b"i=4294967296").0.malformed);
        assert_eq!(parse(b"i=4294967295").0.image_id, u32::MAX);
        assert!(parse(b"z=2147483648").0.malformed);
        assert_eq!(parse(b"z=-2147483648").0.z, i32::MIN);
    }

    #[test]
    fn slice_one_refuses_what_it_does_not_support() {
        let refused = [
            (&b"i=1,t=f"[..], Error::MEDIUM),
            (b"i=1,t=t", Error::MEDIUM),
            (b"i=1,t=s", Error::MEDIUM),
            (b"i=1,a=q,t=f", Error::MEDIUM),
            (b"i=1,t=x", Error::UNKNOWN_MEDIUM),
            (b"i=1,f=8", Error::INDEXED),
            (b"i=1,f=16", Error::INDEXED),
            (b"i=1,f=7", Error::UNKNOWN_FORMAT),
            (b"i=1,o=x", Error::UNKNOWN_COMPRESSION),
            (b"i=1,a=T,P=2", Error::RELATIVE),
            (b"i=1,a=p,H=-1", Error::RELATIVE),
            (b"i=1,a=f", Error::ANIMATION),
            (b"i=1,a=a", Error::ANIMATION),
            (b"i=1,a=c", Error::ANIMATION),
            (b"i=1,a=q,e=T", Error::FEATURE_QUERY),
            (b"i=1,I=2", Error::ID_AND_NUMBER),
            (b"i=1,a=Z", Error::UNKNOWN_ACTION),
        ];
        for (control, error) in refused {
            assert_eq!(parse(control).0.refusal(), Some(error), "{control:?}");
        }
    }

    #[test]
    fn supported_commands_are_not_refused() {
        for control in [
            &b"i=1,f=24,s=1,v=1"[..],
            b"a=T,f=100,o=z",
            b"a=q,i=1,t=d,f=32",
            b"a=p,i=1,p=2,z=-1",
            b"a=d,d=Z,z=1",
            b"a=p,i=1,U=0,P=0,H=0",
            b"a=p,i=1,U=1,c=4,r=2",
            b"a=T,i=1,U=1,f=32,s=1,v=1",
            b"m=1",
        ] {
            assert_eq!(parse(control).0.refusal(), None, "{control:?}");
        }
    }
}
