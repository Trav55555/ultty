//! Parse a file through the VT core with no PTY or renderer and report
//! MiB/s: the ceiling for end-to-end `cat` throughput.
//!
//! cargo run --release -p ultty-vt --example throughput -- <file> [cols rows]
use std::time::Instant;
use ultty_vt::ansi::{AnsiOutput, AnsiParser};
use ultty_vt::grid::{Dimensions, Terminal, TerminalHandler};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: throughput <file> [cols rows]");
    let cols = args.next().map_or(120, |v| v.parse().expect("cols"));
    let rows = args.next().map_or(40, |v| v.parse().expect("rows"));
    let data = std::fs::read(&path).expect("read input");

    for run in 1..=3 {
        let mut terminal = Terminal::new(Dimensions::new(cols, rows));
        let mut parser = AnsiParser::new();
        let start = Instant::now();
        for chunk in data.chunks(64 * 1024) {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed_with_ascii_spans(chunk, |out| match out {
                AnsiOutput::Action(action) => handler.handle(action),
                AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
            });
        }
        let secs = start.elapsed().as_secs_f64();
        let mib = data.len() as f64 / (1024.0 * 1024.0);
        println!(
            "run {run}: {:.0} ms, {:.0} MiB/s",
            secs * 1000.0,
            mib / secs
        );
    }
}
