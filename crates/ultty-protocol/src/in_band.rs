//! In-band transport: protocol frames carried in the terminal byte stream.
//!
//! The ordinary frame byte stream (spec §3) is cut into chunks of at most
//! [`MAX_CHUNK`] bytes. Each chunk travels as one APC string:
//!
//! ```text
//! ESC _ U <base64url of the chunk, no padding> ESC \
//! ```
//!
//! A frame may span several strings and one string may hold several frames;
//! the receiver concatenates decoded chunks and reads frames by their length
//! headers, exactly as it would from a socket. Apps write these strings to
//! their stdout and read the terminal's on their stdin.
//!
//! The chunk limit keeps every string under tmux's 1 MiB passthrough cap and
//! bounds what a parser buffers for one string.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;

/// Most frame bytes carried by one APC string.
pub const MAX_CHUNK: usize = 64 * 1024;

/// Bytes that open an in-band string.
pub const START: &[u8] = b"\x1b_U";

/// String terminator.
pub const END: &[u8] = b"\x1b\\";

/// Writers omit padding; readers accept it either way.
const ENGINE: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Append `frames` to `out` as in-band strings of at most [`MAX_CHUNK`]
/// bytes each. Empty input writes nothing.
pub fn encode(frames: &[u8], out: &mut Vec<u8>) {
    for chunk in frames.chunks(MAX_CHUNK) {
        out.extend_from_slice(START);
        let start = out.len();
        let len = base64::encoded_len(chunk.len(), false).expect("chunk length is small");
        out.resize(start + len, 0);
        let written = ENGINE
            .encode_slice(chunk, &mut out[start..])
            .expect("buffer sized by encoded_len");
        out.truncate(start + written);
        out.extend_from_slice(END);
    }
}

/// `frames` as in-band strings.
pub fn encode_to_vec(frames: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.len() * 4 / 3 + 8);
    encode(frames, &mut out);
    out
}

/// The text of an in-band string was not valid base64url.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("in-band string is not valid base64url")]
pub struct InvalidPayload;

/// Decode the text between `ESC _ U` and `ESC \`, appending frame bytes
/// to `out`. On error `out` is left as it was.
pub fn decode_payload(text: &[u8], out: &mut Vec<u8>) -> Result<(), InvalidPayload> {
    let start = out.len();
    ENGINE.decode_vec(text, out).map_err(|_| {
        out.truncate(start);
        InvalidPayload
    })
}

/// What [`Splitter::feed`] found in the input.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Bytes outside any in-band string, in order.
    Other(&'a [u8]),
    /// The text of one complete in-band string (still base64).
    Payload(&'a [u8]),
}

/// Separates in-band strings from the rest of a byte stream that may be split
/// at any point. Apps use it on stdin, where in-band strings from the
/// terminal arrive alongside replies such as DA1.
///
/// Strings whose text grows past `max_payload` bytes are discarded.
#[derive(Debug)]
pub struct Splitter {
    buf: Vec<u8>,
    payload: Vec<u8>,
    state: SplitState,
    max_payload: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitState {
    Ground,
    /// Saw `ESC` (and `_` when `matched == 2`) outside a string.
    Opening {
        matched: usize,
    },
    InPayload,
    /// Saw `ESC` inside a string.
    PayloadEsc,
    /// Inside a string that is being discarded.
    Discarding,
    DiscardingEsc,
}

impl Default for Splitter {
    fn default() -> Self {
        Self::new(2 * MAX_CHUNK)
    }
}

impl Splitter {
    /// A splitter that keeps strings of up to `max_payload` text bytes.
    pub fn new(max_payload: usize) -> Self {
        Self {
            buf: Vec::new(),
            payload: Vec::new(),
            state: SplitState::Ground,
            max_payload,
        }
    }

    /// Feed `data` and call `on_piece` for everything that is complete.
    ///
    /// Bytes of a possible string opener at the end of `data` are held until
    /// the next call decides whether they start a string.
    pub fn feed(&mut self, data: &[u8], mut on_piece: impl FnMut(Piece<'_>)) {
        let mut other_start = 0;
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            match self.state {
                SplitState::Ground => match data[i..].iter().position(|&c| c == 0x1b) {
                    Some(p) => {
                        i += p;
                        if i > other_start {
                            on_piece(Piece::Other(&data[other_start..i]));
                        }
                        other_start = i;
                        self.state = SplitState::Opening { matched: 1 };
                    }
                    None => break,
                },
                SplitState::Opening { matched } => {
                    if b == START[matched] {
                        if matched + 1 == START.len() {
                            self.state = SplitState::InPayload;
                            self.payload.clear();
                            self.buf.clear();
                            other_start = i + 1;
                        } else {
                            self.state = SplitState::Opening {
                                matched: matched + 1,
                            };
                        }
                    } else {
                        // Not ours: the held opener bytes are ordinary output.
                        self.state = SplitState::Ground;
                        if !self.buf.is_empty() {
                            on_piece(Piece::Other(&std::mem::take(&mut self.buf)));
                        }
                        continue;
                    }
                }
                SplitState::InPayload => {
                    let run = data[i..]
                        .iter()
                        .position(|&c| c == 0x1b)
                        .unwrap_or(data.len() - i);
                    if self.payload.len() + run > self.max_payload {
                        self.payload = Vec::new();
                        self.state = SplitState::Discarding;
                        continue;
                    }
                    self.payload.extend_from_slice(&data[i..i + run]);
                    i += run;
                    if i < data.len() {
                        self.state = SplitState::PayloadEsc;
                    } else {
                        break;
                    }
                }
                SplitState::PayloadEsc => {
                    if b == b'\\' {
                        on_piece(Piece::Payload(&self.payload));
                        self.payload.clear();
                        self.state = SplitState::Ground;
                        other_start = i + 1;
                    } else {
                        // ESC never appears in base64; the string is broken.
                        self.payload = Vec::new();
                        self.state = SplitState::Discarding;
                        continue;
                    }
                }
                SplitState::Discarding => {
                    if b == 0x1b {
                        self.state = SplitState::DiscardingEsc;
                    }
                }
                SplitState::DiscardingEsc => {
                    self.state = if b == b'\\' {
                        other_start = i + 1;
                        SplitState::Ground
                    } else {
                        SplitState::Discarding
                    };
                }
            }
            i += 1;
        }

        match self.state {
            SplitState::Ground => {
                if other_start < data.len() {
                    on_piece(Piece::Other(&data[other_start..]));
                }
            }
            SplitState::Opening { .. } => {
                // Hold the partial opener; it may continue in the next call.
                self.buf
                    .extend_from_slice(&data[other_start.min(data.len())..]);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_all(chunks: &[&[u8]]) -> (Vec<Vec<u8>>, Vec<u8>) {
        let mut s = Splitter::default();
        let mut payloads = Vec::new();
        let mut other = Vec::new();
        for c in chunks {
            s.feed(c, |p| match p {
                Piece::Other(b) => other.extend_from_slice(b),
                Piece::Payload(t) => {
                    let mut raw = Vec::new();
                    decode_payload(t, &mut raw).unwrap();
                    payloads.push(raw);
                }
            });
        }
        (payloads, other)
    }

    #[test]
    fn encode_chunks_large_input() {
        let frames = vec![7u8; MAX_CHUNK * 2 + 10];
        let wire = encode_to_vec(&frames);
        assert_eq!(wire.windows(3).filter(|w| *w == START).count(), 3);
        let (payloads, other) = split_all(&[&wire]);
        assert!(other.is_empty());
        assert_eq!(payloads.concat(), frames);
        assert!(payloads.iter().all(|p| p.len() <= MAX_CHUNK));
    }

    #[test]
    fn encode_writes_nothing_for_empty_input() {
        assert!(encode_to_vec(&[]).is_empty());
    }

    #[test]
    fn splitter_handles_every_split_point() {
        let mut wire = b"\x1b[?62;22c".to_vec();
        wire.extend(encode_to_vec(b"hello frames"));
        wire.extend_from_slice(b"\x1b[A plain \x1b_Gkitty\x1b\\");
        wire.extend(encode_to_vec(&[0, 1, 2, 255]));
        for split in 1..wire.len() {
            let chunks: Vec<&[u8]> = wire.chunks(split).collect();
            let (payloads, other) = split_all(&chunks);
            assert_eq!(
                payloads,
                vec![b"hello frames".to_vec(), vec![0, 1, 2, 255]],
                "split {split}"
            );
            assert_eq!(
                other,
                b"\x1b[?62;22c\x1b[A plain \x1b_Gkitty\x1b\\".to_vec(),
                "split {split}"
            );
        }
    }

    #[test]
    fn splitter_discards_oversized_strings_and_recovers() {
        let mut s = Splitter::new(8);
        let mut payloads = Vec::new();
        let mut wire = b"\x1b_U".to_vec();
        wire.extend_from_slice(&[b'A'; 20]);
        wire.extend_from_slice(b"\x1b\\");
        wire.extend(encode_to_vec(b"ok"));
        s.feed(&wire, |p| {
            if let Piece::Payload(t) = p {
                payloads.push(t.to_vec());
            }
        });
        assert_eq!(payloads, vec![b"b2s".to_vec()]);
    }

    #[test]
    fn decode_rejects_bad_text_and_leaves_output_untouched() {
        let mut out = vec![1];
        assert_eq!(decode_payload(b"!!!", &mut out), Err(InvalidPayload));
        assert_eq!(out, vec![1]);
        decode_payload(b"b2s=", &mut out).unwrap();
        assert_eq!(out, vec![1, b'o', b'k']);
    }
}
