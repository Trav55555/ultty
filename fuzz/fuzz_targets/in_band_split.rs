//! The in-band splitter must find the same strings and the same other bytes
//! however the input is cut into reads, and decoding must never panic.
//!
//! Input layout: one byte picks the read size (1-64), the rest is the
//! stream. Splitting is compared against feeding the stream whole.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ultty_protocol::in_band::{decode_payload, Piece, Splitter};

/// Payload texts and the concatenated non-payload bytes.
fn split(chunks: &mut dyn Iterator<Item = &[u8]>) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut splitter = Splitter::new(256);
    let mut payloads = Vec::new();
    let mut other = Vec::new();
    for chunk in chunks {
        splitter.feed(chunk, |piece| match piece {
            Piece::Payload(text) => {
                let mut raw = Vec::new();
                let _ = decode_payload(text, &mut raw);
                payloads.push(text.to_vec());
            }
            Piece::Other(bytes) => other.extend_from_slice(bytes),
        });
    }
    (payloads, other)
}

fuzz_target!(|data: &[u8]| {
    let [size, stream @ ..] = data else {
        return;
    };
    let size = usize::from(size % 64) + 1;
    let whole = split(&mut std::iter::once(stream));
    let pieces = split(&mut stream.chunks(size));
    assert_eq!(whole, pieces, "read size {size}");
});
