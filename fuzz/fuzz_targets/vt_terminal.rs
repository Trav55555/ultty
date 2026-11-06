//! Feed arbitrary bytes through the VT parser into a terminal, interleaved
//! with resizes, and assert the terminal stays structurally consistent.
//!
//! Input layout: two bytes pick the starting size, then a sequence of
//! commands. A command byte with the high bit set is a resize (the next two
//! bytes give columns and rows); otherwise its low seven bits are the length
//! of a chunk of bytes to feed to the parser.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ultty_vt::ansi::AnsiParser;
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

fn dims(cols: u8, rows: u8) -> Dimensions {
    Dimensions::new(u16::from(cols % 96) + 1, u16::from(rows % 32) + 1)
}

fuzz_target!(|data: &[u8]| {
    let [cols, rows, rest @ ..] = data else {
        return;
    };
    let mut terminal = Terminal::new(dims(*cols, *rows));
    let mut parser = AnsiParser::new();
    let mut input = rest;

    while let [command, tail @ ..] = input {
        if command & 0x80 != 0 {
            let [c, r, after @ ..] = tail else {
                break;
            };
            terminal.resize(dims(*c, *r));
            input = after;
        } else {
            let len = usize::from(command & 0x7f).min(tail.len());
            let (chunk, after) = tail.split_at(len);
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(chunk, |action| handler.handle(action));
            input = after;
        }
        if let Err(violation) = terminal.check_integrity() {
            panic!("integrity violation: {violation}");
        }
        // Responses (DSR, DECRQM, ...) would otherwise accumulate forever.
        drop(terminal.take_responses());
    }
});
