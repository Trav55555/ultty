//! Parser throughput on the production entry point.
//!
//! The event loop calls `AnsiParser::feed_with_ascii_spans`, so that is what
//! these benches time, with one long-lived parser fed in 64 KiB chunks as
//! the PTY delivers them. Inputs are 8 MiB corpora from `ultty-bench`, large
//! enough not to sit in L2, and include non-ASCII text, which takes the
//! slow per-character path. Output goes to a `black_box` sink; grid work is
//! measured by `ultty-bench vt`.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use ultty::protocol::ansi::AnsiParser;
use ultty_bench::corpus::{self, Corpus};

const CHUNK: usize = 64 * 1024;
const CORPUS_BYTES: usize = 8 * 1024 * 1024;

fn parse_all(parser: &mut AnsiParser, data: &[u8]) -> usize {
    let mut outputs = 0usize;
    for chunk in data.chunks(CHUNK) {
        parser.feed_with_ascii_spans(black_box(chunk), |out| {
            black_box(&out);
            outputs += 1;
        });
    }
    outputs
}

fn bench_corpus_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_corpus");
    group.sample_size(10);
    for corpus in [
        Corpus::Ascii,
        Corpus::SgrDense,
        Corpus::UnicodeMixed,
        Corpus::Cjk,
        Corpus::EmojiZwj,
        Corpus::CursorRedraw,
        Corpus::AltScreen,
    ] {
        let data = corpus::generate(corpus, CORPUS_BYTES).bytes;
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_function(corpus.name(), |b| {
            let mut parser = AnsiParser::new();
            b.iter(|| parse_all(&mut parser, &data))
        });
    }
    group.finish();
}

/// Sequence-specific costs, each repeated to 1 MiB so per-call overhead
/// does not dominate.
fn bench_sequences(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_sequences");
    let cases: [(&str, &[u8]); 5] = [
        ("cursor_movement", b"\x1b[5A\x1b[3B\x1b[10C\x1b[2D"),
        ("sgr_attributes", b"\x1b[1;4;31;44mab\x1b[0m"),
        ("truecolor", b"\x1b[38;2;100;150;200mx"),
        ("osc_title", b"\x1b]0;user@host: ~/src\x07"),
        ("utf8_cjk", "日本語の文字列".as_bytes()),
    ];
    for (name, unit) in cases {
        let data: Vec<u8> = unit
            .iter()
            .copied()
            .cycle()
            .take((1024 * 1024 / unit.len()) * unit.len())
            .collect();
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_function(name, |b| {
            let mut parser = AnsiParser::new();
            b.iter(|| parse_all(&mut parser, &data))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_corpus_throughput, bench_sequences);
criterion_main!(benches);
