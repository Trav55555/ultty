//! Input latency benchmarks measuring the internal processing pipeline.
//!
//! Measures time from receiving PTY output to completing grid updates.
//! The actual PTY round-trip depends on the shell and is not controllable.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use ultty::grid::TerminalHandler;
use ultty::grid::{Dimensions, Terminal};
use ultty::protocol::ansi::AnsiParser;
use unicode_width::UnicodeWidthChar;

fn feed_payload(terminal: &mut Terminal, parser: &mut AnsiParser, payload: &[u8]) {
    let mut handler = TerminalHandler::new(terminal);
    parser.feed(black_box(payload), |action| {
        handler.handle(action);
    });
}

fn feed_ascii_span_candidate(terminal: &mut Terminal, parser: &mut AnsiParser, payload: &[u8]) {
    let mut index = 0;
    while index < payload.len() {
        if payload[index].is_ascii_graphic() || payload[index] == b' ' {
            let start = index;
            index += 1;
            while index < payload.len()
                && (payload[index].is_ascii_graphic() || payload[index] == b' ')
            {
                index += 1;
            }

            for byte in &payload[start..index] {
                if !terminal.is_alternate_screen() {
                    terminal.buffer_mut().write_char(*byte as char);
                } else {
                    terminal.grid_mut().write_char(*byte as char);
                }
            }
        } else {
            let mut handler = TerminalHandler::new(terminal);
            parser.feed(&payload[index..index + 1], |action| {
                handler.handle(action);
            });
            index += 1;
        }
    }
}

fn long_lines_payload(line_count: usize, line_len: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(line_count * (line_len + 1));
    let line = vec![b'x'; line_len];
    for _ in 0..line_count {
        payload.extend_from_slice(&line);
        payload.push(b'\n');
    }
    payload
}

fn fixed_width_lines_payload(line_count: usize, line_len: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(line_count * (line_len + 1));
    let line = vec![b'x'; line_len];
    for _ in 0..line_count {
        payload.extend_from_slice(&line);
        payload.push(b'\n');
    }
    payload
}

fn alternate_screen_payload(inner: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(inner.len() + 16);
    payload.extend_from_slice(b"\x1b[?1049h");
    payload.extend_from_slice(inner);
    payload
}

fn ascii_width_candidate(c: char) -> u16 {
    let codepoint = c as u32;
    if (0x20..=0x7e).contains(&codepoint) {
        1
    } else if codepoint < 0x20 || codepoint == 0x7f {
        0
    } else {
        c.width().unwrap_or(0) as u16
    }
}

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

fn bench_wrap_and_reflow(c: &mut Criterion) {
    let mut group = c.benchmark_group("wrap_hot_path");
    group.sample_size(10);

    let long_wrapped_lines = long_lines_payload(500, 4000);
    let short_hard_lines = fixed_width_lines_payload(25_000, 80);

    group.bench_function("500_lines_4000_cols_soft_wrap", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&long_wrapped_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("25000_lines_80_cols_hard_breaks", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&short_hard_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("resize_reflow_500_wrapped_lines_80_to_120", |b| {
        b.iter_batched(
            || {
                let mut terminal = Terminal::new(Dimensions::new(80, 34));
                let mut parser = AnsiParser::new();
                feed_payload(&mut terminal, &mut parser, &long_wrapped_lines);
                terminal
            },
            |mut terminal| {
                terminal.resize(Dimensions::new(120, 34));
                black_box(terminal.grid().dimensions());
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_char_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("char_width");
    let ascii = "the quick brown fox jumps over the lazy dog 0123456789 ABCDEF";
    let mixed = "日本語テスト 🚀🎨🔥 αβγδε é̃";

    group.bench_function("ascii_candidate_fast_path", |b| {
        b.iter(|| {
            ascii
                .chars()
                .map(|c| ascii_width_candidate(black_box(c)) as usize)
                .sum::<usize>()
        })
    });

    group.bench_function("ascii_unicode_width_direct", |b| {
        b.iter(|| {
            ascii
                .chars()
                .map(|c| UnicodeWidthChar::width(black_box(c)).unwrap_or(0))
                .sum::<usize>()
        })
    });

    group.bench_function("mixed_fallback_path", |b| {
        b.iter(|| {
            mixed
                .chars()
                .map(|c| ascii_width_candidate(black_box(c)) as usize)
                .sum::<usize>()
        })
    });

    group.finish();
}

fn bench_scroll_history(c: &mut Criterion) {
    let mut group = c.benchmark_group("scroll_history");
    group.sample_size(10);

    let hard_lines = fixed_width_lines_payload(25_000, 80);
    let hard_lines_no_history = alternate_screen_payload(&hard_lines);
    let short_lines = fixed_width_lines_payload(25_000, 12);

    group.bench_function("25000_lines_80_cols_main_scrollback", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&hard_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("25000_lines_80_cols_alt_no_scrollback", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(
                    &mut terminal,
                    &mut parser,
                    black_box(&hard_lines_no_history),
                );
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("25000_lines_12_cols_main_scrollback", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&short_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_ascii_span(c: &mut Criterion) {
    let mut group = c.benchmark_group("ascii_span");
    group.sample_size(10);

    let ascii_throughput = long_lines_payload(1024, 4096);
    let hard_lines = fixed_width_lines_payload(25_000, 80);

    group.bench_function("current_parser_4m_ascii", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&ascii_throughput));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("candidate_4m_ascii_span", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_ascii_span_candidate(&mut terminal, &mut parser, black_box(&ascii_throughput));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("current_parser_25k_hard_lines", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_payload(&mut terminal, &mut parser, black_box(&hard_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("candidate_25k_hard_lines", |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed_ascii_span_candidate(&mut terminal, &mut parser, black_box(&hard_lines));
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_keystroke_echo,
    bench_full_refresh,
    bench_wrap_and_reflow,
    bench_char_width,
    bench_scroll_history,
    bench_ascii_span
);
criterion_main!(benches);
