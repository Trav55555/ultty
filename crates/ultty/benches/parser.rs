//! Benchmarks for the ANSI parser.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use ultty::protocol::ansi::AnsiParser;

/// Generate realistic terminal output with escape sequences.
fn generate_terminal_output(size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size);
    let mut i = 0;

    while output.len() < size {
        match i % 10 {
            // Plain text (most common)
            0..=5 => {
                output.extend_from_slice(b"Hello, World! This is some terminal output.\r\n");
            }
            // Cursor movement
            6 => {
                output.extend_from_slice(b"\x1b[10;20H"); // Move to position
                output.extend_from_slice(b"\x1b[5A"); // Cursor up
                output.extend_from_slice(b"\x1b[3B"); // Cursor down
            }
            // SGR colors
            7 => {
                output.extend_from_slice(b"\x1b[1;31m"); // Bold red
                output.extend_from_slice(b"Error: ");
                output.extend_from_slice(b"\x1b[0m"); // Reset
            }
            // True color
            8 => {
                output.extend_from_slice(b"\x1b[38;2;100;150;200m"); // RGB foreground
                output.extend_from_slice(b"Colored text");
                output.extend_from_slice(b"\x1b[39m"); // Default fg
            }
            // Erase and scroll
            9 => {
                output.extend_from_slice(b"\x1b[2J"); // Clear screen
                output.extend_from_slice(b"\x1b[H"); // Home
                output.extend_from_slice(b"\x1b[5S"); // Scroll up
            }
            _ => unreachable!(),
        }
        i += 1;
    }

    output.truncate(size);
    output
}

/// Generate pure escape sequence stream (worst case for parser).
fn generate_escape_heavy(size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size);

    while output.len() < size {
        // Mix of various escape sequences
        output.extend_from_slice(b"\x1b[1;2;3;4;5;6;7m"); // Multiple SGR params
        output.extend_from_slice(b"\x1b[38;2;255;128;64m"); // True color
        output.extend_from_slice(b"\x1b[48;5;200m"); // 256 color
        output.extend_from_slice(b"\x1b[?1049h"); // Alternate screen
        output.extend_from_slice(b"\x1b[100;200H"); // Position
        output.extend_from_slice(b"\x1b[0m"); // Reset
    }

    output.truncate(size);
    output
}

/// Generate plain text (best case for parser).
fn generate_plain_text(size: usize) -> Vec<u8> {
    let text = b"The quick brown fox jumps over the lazy dog. ";
    let mut output = Vec::with_capacity(size);

    while output.len() < size {
        output.extend_from_slice(text);
    }

    output.truncate(size);
    output
}

fn bench_parser_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_throughput");

    // Test different input sizes
    for size in [1024, 4096, 16384, 65536] {
        group.throughput(Throughput::Bytes(size as u64));

        let realistic = generate_terminal_output(size);
        group.bench_function(format!("realistic_{size}"), |b| {
            b.iter(|| {
                let mut parser = AnsiParser::new();
                let mut count = 0usize;
                parser.feed(black_box(&realistic), |_action| {
                    count += 1;
                });
                count
            })
        });

        let escape_heavy = generate_escape_heavy(size);
        group.bench_function(format!("escape_heavy_{size}"), |b| {
            b.iter(|| {
                let mut parser = AnsiParser::new();
                let mut count = 0usize;
                parser.feed(black_box(&escape_heavy), |_action| {
                    count += 1;
                });
                count
            })
        });

        let plain = generate_plain_text(size);
        group.bench_function(format!("plain_text_{size}"), |b| {
            b.iter(|| {
                let mut parser = AnsiParser::new();
                let mut count = 0usize;
                parser.feed(black_box(&plain), |_action| {
                    count += 1;
                });
                count
            })
        });
    }

    group.finish();
}

fn bench_parser_specific(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_specific");

    // Cursor movement parsing
    let cursor_moves: Vec<u8> = (0..1000)
        .flat_map(|_| b"\x1b[5A\x1b[3B\x1b[10C\x1b[2D".iter().copied())
        .collect();
    group.bench_function("cursor_movement", |b| {
        b.iter(|| {
            let mut parser = AnsiParser::new();
            let mut count = 0usize;
            parser.feed(black_box(&cursor_moves), |_| count += 1);
            count
        })
    });

    // SGR parsing (colors and attributes)
    let sgr_sequences: Vec<u8> = (0..1000)
        .flat_map(|_| b"\x1b[1;4;31;44m\x1b[0m".iter().copied())
        .collect();
    group.bench_function("sgr_attributes", |b| {
        b.iter(|| {
            let mut parser = AnsiParser::new();
            let mut count = 0usize;
            parser.feed(black_box(&sgr_sequences), |_| count += 1);
            count
        })
    });

    // True color parsing
    let truecolor: Vec<u8> = (0..1000)
        .flat_map(|_| b"\x1b[38;2;100;150;200m".iter().copied())
        .collect();
    group.bench_function("truecolor", |b| {
        b.iter(|| {
            let mut parser = AnsiParser::new();
            let mut count = 0usize;
            parser.feed(black_box(&truecolor), |_| count += 1);
            count
        })
    });

    // Print characters (ASCII)
    let printable: Vec<u8> = (0..10000).map(|i| b'A' + (i % 26) as u8).collect();
    group.bench_function("printable_chars", |b| {
        b.iter(|| {
            let mut parser = AnsiParser::new();
            let mut count = 0usize;
            parser.feed(black_box(&printable), |_| count += 1);
            count
        })
    });

    group.finish();
}

fn bench_parser_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_reuse");

    let input = generate_terminal_output(4096);

    // Fresh parser each time
    group.bench_function("fresh_parser", |b| {
        b.iter(|| {
            let mut parser = AnsiParser::new();
            let mut count = 0usize;
            parser.feed(black_box(&input), |_| count += 1);
            count
        })
    });

    // Reused parser (simulates continuous stream)
    group.bench_function("reused_parser", |b| {
        let mut parser = AnsiParser::new();
        b.iter(|| {
            let mut count = 0usize;
            parser.feed(black_box(&input), |_| count += 1);
            count
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parser_throughput,
    bench_parser_specific,
    bench_parser_reuse
);
criterion_main!(benches);
