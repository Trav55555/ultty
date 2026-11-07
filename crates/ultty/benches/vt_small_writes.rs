//! Parse plus grid cost of small and medium writes.
//!
//! This is not input latency: no key event, PTY, render or present is
//! involved. It times what the I/O thread does with one PTY read: feed the
//! bytes through `feed_with_ascii_spans` (the production entry point) into a
//! `TerminalHandler`. End-to-end latency lives in the live drivers
//! (`ubench-echo`, `ubench-k2p`; see docs/perf/README.md).
//!
//! Multi-line payloads end every line with CRLF, as a PTY with ONLCR
//! delivers them. A bare LF moves down without returning to column 0, so the
//! cursor staircases off the right edge and the workload stops resembling
//! real output (the 2026-11-07 probe measured a 45% difference).
//!
//! (Formerly `input_latency.rs`; results under the old group names are not
//! comparable, because those payloads used bare LF.)

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use ultty::grid::TerminalHandler;
use ultty::grid::{Dimensions, Terminal};
use ultty::protocol::ansi::{AnsiOutput, AnsiParser};
use unicode_width::UnicodeWidthChar;

/// The production path: ASCII runs go straight to `handle_ascii_span`.
fn feed(terminal: &mut Terminal, parser: &mut AnsiParser, payload: &[u8]) {
    let mut handler = TerminalHandler::new(terminal);
    parser.feed_with_ascii_spans(black_box(payload), |output| match output {
        AnsiOutput::Action(action) => handler.handle(action),
        AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
    });
}

/// The per-character action path, kept only to show what the span fast
/// path saves.
fn feed_actions(terminal: &mut Terminal, parser: &mut AnsiParser, payload: &[u8]) {
    let mut handler = TerminalHandler::new(terminal);
    parser.feed(black_box(payload), |action| handler.handle(action));
}

/// `line_count` lines of `line_len` ASCII characters, CRLF-terminated.
fn crlf_lines(line_count: usize, line_len: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(line_count * (line_len + 2));
    let line = vec![b'x'; line_len];
    for _ in 0..line_count {
        payload.extend_from_slice(&line);
        payload.extend_from_slice(b"\r\n");
    }
    payload
}

fn alternate_screen_payload(inner: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(inner.len() + 16);
    payload.extend_from_slice(b"\x1b[?1049h");
    payload.extend_from_slice(inner);
    payload
}

fn bench_small_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("small_writes");
    let cases: [(&str, &[u8]); 6] = [
        ("single_char_echo", b"x"),
        ("line_echo", b"Hello, World!\r\n"),
        (
            "colored_prompt",
            b"\x1b[1;32muser@host\x1b[0m:\x1b[1;34m~/projects\x1b[0m$ ",
        ),
        ("cursor_movement_sequence", b"\x1b[H\x1b[2J\x1b[10;20H"),
        ("scroll_region_update", b"\x1b[5;20r\x1b[5S\x1b[r"),
        ("cjk_echo", "日本".as_bytes()),
    ];
    for (name, payload) in cases {
        group.bench_function(name, |b| {
            let mut terminal = Terminal::new(Dimensions::new(80, 24));
            let mut parser = AnsiParser::new();
            b.iter(|| feed(&mut terminal, &mut parser, payload))
        });
    }
    group.finish();
}

fn bench_full_refresh(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_refresh");

    let mut full_screen = Vec::new();
    full_screen.extend_from_slice(b"\x1b[H\x1b[2J");
    for row in 0..24 {
        full_screen.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        full_screen.extend_from_slice(
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz012345678901234567",
        );
    }
    let line_update = b"\x1b[24;1H$ cargo build --release\x1b[K";

    group.bench_function("clear_and_redraw_80x24", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        b.iter(|| feed(&mut terminal, &mut parser, &full_screen))
    });
    group.bench_function("incremental_line_update", |b| {
        let mut terminal = Terminal::new(Dimensions::new(80, 24));
        let mut parser = AnsiParser::new();
        b.iter(|| feed(&mut terminal, &mut parser, line_update))
    });
    group.finish();
}

/// Feed `payload` into a fresh 80x34 terminal per iteration.
fn bench_fresh(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    payload: &[u8],
) {
    group.throughput(Throughput::Bytes(payload.len() as u64));
    group.bench_function(name, |b| {
        b.iter_batched(
            || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
            |(mut terminal, mut parser)| {
                feed(&mut terminal, &mut parser, payload);
                black_box(terminal.grid().cursor().position);
            },
            BatchSize::LargeInput,
        )
    });
}

fn bench_wrap_and_reflow(c: &mut Criterion) {
    let mut group = c.benchmark_group("wrap_hot_path");
    group.sample_size(10);

    let long_wrapped_lines = crlf_lines(500, 4000);
    bench_fresh(
        &mut group,
        "500_lines_4000_cols_soft_wrap",
        &long_wrapped_lines,
    );
    bench_fresh(
        &mut group,
        "25000_lines_80_cols_hard_breaks",
        &crlf_lines(25_000, 80),
    );

    group.throughput(Throughput::Elements(1));
    group.bench_function("resize_reflow_500_wrapped_lines_80_to_120", |b| {
        b.iter_batched(
            || {
                let mut terminal = Terminal::new(Dimensions::new(80, 34));
                let mut parser = AnsiParser::new();
                feed(&mut terminal, &mut parser, &long_wrapped_lines);
                terminal
            },
            |mut terminal| {
                terminal.resize(Dimensions::new(120, 34));
                black_box(terminal.grid().dimensions());
            },
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

/// The width function the grid uses (`Grid::write_char` and
/// `Buffer::write_char` call `UnicodeWidthChar::width`), then the same
/// text through the whole write path.
fn bench_char_width(c: &mut Criterion) {
    let mut group = c.benchmark_group("char_width");
    let ascii = "the quick brown fox jumps over the lazy dog 0123456789 ABCDEF";
    let mixed = "日本語テスト 🚀🎨🔥 αβγδε é̃";

    for (name, text) in [("ascii", ascii), ("mixed", mixed)] {
        group.bench_function(format!("unicode_width_{name}"), |b| {
            b.iter(|| {
                text.chars()
                    .map(|c| UnicodeWidthChar::width(black_box(c)).unwrap_or(0))
                    .sum::<usize>()
            })
        });
        let line = format!("{text}\r\n");
        group.bench_function(format!("write_path_{name}"), |b| {
            let mut terminal = Terminal::new(Dimensions::new(120, 40));
            let mut parser = AnsiParser::new();
            b.iter(|| feed(&mut terminal, &mut parser, line.as_bytes()))
        });
    }

    group.finish();
}

fn bench_scroll_history(c: &mut Criterion) {
    let mut group = c.benchmark_group("scroll_history");
    group.sample_size(10);

    let hard_lines = crlf_lines(25_000, 80);
    bench_fresh(
        &mut group,
        "25000_lines_80_cols_main_scrollback",
        &hard_lines,
    );
    bench_fresh(
        &mut group,
        "25000_lines_80_cols_alt_no_scrollback",
        &alternate_screen_payload(&hard_lines),
    );
    bench_fresh(
        &mut group,
        "25000_lines_12_cols_main_scrollback",
        &crlf_lines(25_000, 12),
    );

    group.finish();
}

fn bench_ascii_span(c: &mut Criterion) {
    let mut group = c.benchmark_group("ascii_span");
    group.sample_size(10);

    for (name, payload) in [
        ("4m_ascii", crlf_lines(1024, 4096)),
        ("25k_hard_lines", crlf_lines(25_000, 80)),
    ] {
        group.throughput(Throughput::Bytes(payload.len() as u64));
        group.bench_function(format!("action_path_{name}"), |b| {
            b.iter_batched(
                || (Terminal::new(Dimensions::new(80, 34)), AnsiParser::new()),
                |(mut terminal, mut parser)| {
                    feed_actions(&mut terminal, &mut parser, &payload);
                    black_box(terminal.grid().cursor().position);
                },
                BatchSize::LargeInput,
            )
        });
        bench_fresh(&mut group, &format!("production_{name}"), &payload);
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_small_writes,
    bench_full_refresh,
    bench_wrap_and_reflow,
    bench_char_width,
    bench_scroll_history,
    bench_ascii_span
);
criterion_main!(benches);
