#![no_main]

use libfuzzer_sys::fuzz_target;
use ultty_protocol::{AppMessage, Decode};

fuzz_target!(|data: &[u8]| {
    let _ = AppMessage::decode(&mut &data[..]);
});
