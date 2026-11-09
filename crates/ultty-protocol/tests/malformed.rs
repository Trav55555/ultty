//! Malformed-input oracle: decoding arbitrary bytes must never panic.
//!
//! These tests complement the cargo-fuzz targets in `fuzz/` (see the crate
//! docs for how to run them) with a cheap, always-on proptest pass.

use bytes::{BufMut, BytesMut};
use proptest::prelude::*;
use ultty_protocol::{AppMessage, Decode, TerminalMessage, FRAME_MAGIC};

/// Every message-type code the decoders know about (both directions).
const KNOWN_MSG_TYPES: &[u16] = &[
    0x0001, 0x0002, 0x0010, 0x0012, 0x0013, 0x0014, 0x0020, 0x0021, 0x0022, 0x0023, 0x0024, 0x0025,
    0x0026, 0x0030, 0x0031, 0x0040, 0x0041, 0x0042, 0x0050, 0x0051, 0x0060, 0x0061, 0x0070, 0x00FF,
];

fn arb_known_msg_type() -> impl Strategy<Value = u16> {
    proptest::sample::select(KNOWN_MSG_TYPES)
}

/// A frame with a valid header (correct magic, known type, payload_len that
/// matches the random payload that follows).
fn arb_framed_payload() -> impl Strategy<Value = Vec<u8>> {
    (
        arb_known_msg_type(),
        proptest::collection::vec(any::<u8>(), 0..4096),
    )
        .prop_map(|(msg_type, payload)| {
            let mut buf = BytesMut::with_capacity(8 + payload.len());
            buf.put_slice(&FRAME_MAGIC);
            buf.put_u16_le(msg_type);
            buf.put_u32_le(payload.len() as u32);
            buf.put_slice(&payload);
            buf.to_vec()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn app_decode_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..8192)
    ) {
        let _ = AppMessage::decode(&mut bytes.as_slice());
    }

    #[test]
    fn terminal_decode_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..8192)
    ) {
        let _ = TerminalMessage::decode(&mut bytes.as_slice());
    }

    #[test]
    fn app_decode_never_panics_on_framed_random_payload(frame in arb_framed_payload()) {
        let mut cursor = frame.as_slice();
        let result = AppMessage::decode(&mut cursor);
        // A successful decode must consume exactly the frame, so a stream of
        // frames stays in sync.
        if result.is_ok() {
            prop_assert!(cursor.is_empty(), "decoder left {} trailing bytes", cursor.len());
        }
    }

    #[test]
    fn terminal_decode_never_panics_on_framed_random_payload(frame in arb_framed_payload()) {
        let mut cursor = frame.as_slice();
        let result = TerminalMessage::decode(&mut cursor);
        if result.is_ok() {
            prop_assert!(cursor.is_empty(), "decoder left {} trailing bytes", cursor.len());
        }
    }
}
