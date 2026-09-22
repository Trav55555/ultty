#![no_main]

use libfuzzer_sys::fuzz_target;
use ultty_protocol::{Decode, TerminalMessage};

fuzz_target!(|data: &[u8]| {
    let _ = TerminalMessage::decode(&mut &data[..]);
});
