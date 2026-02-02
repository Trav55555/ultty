//! Input latency benchmarks measuring the internal processing pipeline.
//!
//! Measures time from receiving PTY output to completing grid updates.
//! The actual PTY round-trip depends on the shell and is not controllable.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ultty::grid::TerminalHandler;
use ultty::grid::{Dimensions, Terminal};
use ultty::protocol::ansi::AnsiParser;

fn bench_keystroke_echo(c: &mut Criterion) {
    let mut group = c.benchmark_group("input_latency");

    group.bench_function("single_char_echo", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(b"x"), |action| {
                handler.handle(action);
            });
        })
    });

    group.bench_function("line_echo", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        let line = b"Hello, World!\r\n";

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(line), |action| {
                handler.handle(action);
            });
        })
    });

    group.bench_function("colored_prompt", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        let prompt = b"\x1b[1;32muser@host\x1b[0m:\x1b[1;34m~/projects\x1b[0m$ ";

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(prompt), |action| {
                handler.handle(action);
            });
        })
    });

    group.bench_function("cursor_movement_sequence", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        let sequence = b"\x1b[H\x1b[2J\x1b[10;20H";

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(sequence), |action| {
                handler.handle(action);
            });
        })
    });

    group.bench_function("scroll_region_update", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        let scroll = b"\x1b[5;20r\x1b[5S\x1b[r";

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(scroll), |action| {
                handler.handle(action);
            });
        })
    });

    group.finish();
}

fn bench_full_refresh(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_refresh");

    group.bench_function("clear_and_redraw_80x24", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();

        let mut full_screen = Vec::new();
        full_screen.extend_from_slice(b"\x1b[H\x1b[2J");
        for row in 0..24 {
            full_screen.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
            full_screen.extend_from_slice(
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz012345678901234567",
            );
        }

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(&full_screen), |action| {
                handler.handle(action);
            });
        })
    });

    group.bench_function("incremental_line_update", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();

        let line_update = b"\x1b[24;1H$ cargo build --release\x1b[K";

        b.iter(|| {
            let mut handler = TerminalHandler::new(&mut terminal);
            parser.feed(black_box(line_update), |action| {
                handler.handle(action);
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_keystroke_echo, bench_full_refresh);
criterion_main!(benches);
