//! Headless frame render time.
//!
//! Each frame renders into one offscreen target that is created once, as a
//! swapchain image would be, and waits for the GPU, so a sample is CPU
//! encode plus GPU execution of one frame. Present and compositor time are
//! not included.
//!
//! The adapter is printed at startup and is part of every group name
//! (`@gpu-vulkan`, `@swrast-gl`, ...): numbers from a software rasterizer such as
//! lavapipe must never be compared with real-GPU numbers, and criterion
//! baselines must not mix them.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ultty::grid::{Dimensions, Terminal, TerminalHandler};
use ultty::protocol::ansi::{AnsiOutput, AnsiParser};
use ultty::renderer::{OverlayState, Renderer};
use ultty_bench::corpus::{self, Corpus};

const SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (200, 60)];

fn create_filled_terminal(cols: u16, rows: u16) -> Terminal {
    let mut term = Terminal::new(Dimensions::new(cols, rows));
    let grid = term.buffer_mut().grid_mut();
    for row in 0..rows {
        grid.cursor_mut().position.row = row;
        grid.cursor_mut().position.col = 0;
        for _ in 0..cols {
            grid.write_char('X');
        }
    }
    term
}

fn create_mixed_terminal(cols: u16, rows: u16) -> Terminal {
    let mut term = Terminal::new(Dimensions::new(cols, rows));
    let grid = term.buffer_mut().grid_mut();
    for row in 0..rows {
        grid.cursor_mut().position.row = row;
        grid.cursor_mut().position.col = 0;
        let line = match row % 3 {
            0 => "Hello, World! This is a test line with some content.",
            1 => "$ cargo build --release",
            _ => "",
        };
        for c in line.chars() {
            grid.write_char(c);
        }
    }
    term
}

/// A screen left by real output: the tail of a corpus fed through the
/// production parser, so colors, attributes and wide glyphs appear in the
/// proportions the corpus has.
fn create_corpus_terminal(cols: u16, rows: u16, corpus: Corpus) -> Terminal {
    let data = corpus::generate(corpus, 256 * 1024).bytes;
    let mut term = Terminal::new(Dimensions::new(cols, rows));
    let mut parser = AnsiParser::new();
    let mut handler = TerminalHandler::new(&mut term);
    parser.feed_with_ascii_spans(&data, |out| match out {
        AnsiOutput::Action(action) => handler.handle(action),
        AnsiOutput::PrintAscii(bytes) => handler.handle_ascii_span(bytes),
    });
    term
}

/// A screen of punctuation-heavy source code, where ligature run shaping
/// does the most work.
fn create_code_terminal(cols: u16, rows: u16) -> Terminal {
    let lines = [
        "    if a != b && c >= d => { return x->y; } // ok",
        "    let total = items.iter().map(|x| x * 2).sum::<u32>();",
        "    match v { Some(x) => x <= 10, None => false } !== ===",
        "    #[derive(Debug)] fn f<'a>(s: &'a str) -> Result<(), Error>",
    ];
    let mut terminal = Terminal::new(Dimensions::new(cols, rows));
    let grid = terminal.grid_mut();
    for row in 0..rows {
        grid.set_cursor_position(0, row);
        for c in lines[row as usize % lines.len()]
            .chars()
            .take(cols as usize)
        {
            grid.write_char(c);
        }
    }
    terminal
}

/// "gpu-<backend>" or "swrast-<backend>" for the adapter the headless
/// renderer picked. The GL backend reports Mesa's llvmpipe as an ordinary
/// device on some drivers, so the name is checked too.
fn adapter_class(info: &wgpu::AdapterInfo) -> String {
    let name = info.name.to_lowercase();
    let software = info.device_type == wgpu::DeviceType::Cpu
        || ["llvmpipe", "lavapipe", "softpipe", "swiftshader"]
            .iter()
            .any(|s| name.contains(s));
    let backend = format!("{:?}", info.backend).to_lowercase();
    format!("{}-{backend}", if software { "swrast" } else { "gpu" })
}

struct Bench {
    rt: tokio::runtime::Runtime,
    class: String,
}

impl Bench {
    fn new() -> Self {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let probe = rt
            .block_on(Renderer::new_headless(64, 64))
            .expect("headless renderer");
        let info = probe.adapter_info();
        eprintln!(
            "render bench adapter: {} ({:?}, {:?}, driver {} {})",
            info.name, info.backend, info.device_type, info.driver, info.driver_info
        );
        let class = adapter_class(info);
        Bench { rt, class }
    }

    fn renderer(&self, cols: u16, rows: u16) -> Renderer {
        self.rt
            .block_on(Renderer::new_headless(cols as u32 * 10, rows as u32 * 20))
            .expect("headless renderer")
    }

    fn run(&self, c: &mut Criterion, group: &str, cases: &[(String, u16, u16, Terminal)]) {
        let mut group = c.benchmark_group(format!("{group}@{}", self.class));
        if cases.iter().any(|(_, cols, _, _)| *cols >= 300) {
            group.sample_size(20);
        }
        for (name, cols, rows, terminal) in cases {
            let mut renderer = self.renderer(*cols, *rows);
            let target = renderer.create_offscreen_target();
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let overlay = OverlayState::default();
            group.bench_function(name, |b| {
                b.iter(|| {
                    renderer
                        .render_to_view(&view, black_box(terminal), black_box(&overlay))
                        .expect("render failed");
                })
            });
        }
        group.finish();
    }
}

fn sized(make: fn(u16, u16) -> Terminal) -> Vec<(String, u16, u16, Terminal)> {
    SIZES
        .iter()
        .map(|&(cols, rows)| (format!("{cols}x{rows}"), cols, rows, make(cols, rows)))
        .collect()
}

fn bench_render(c: &mut Criterion) {
    let bench = Bench::new();
    bench.run(
        c,
        "render_empty",
        &sized(|c, r| Terminal::new(Dimensions::new(c, r))),
    );
    bench.run(c, "render_filled", &sized(create_filled_terminal));
    bench.run(c, "render_mixed", &sized(create_mixed_terminal));
    bench.run(
        c,
        "render_corpus",
        &[
            Corpus::SgrDense,
            Corpus::UnicodeMixed,
            Corpus::Cjk,
            Corpus::EmojiZwj,
        ]
        .map(|corpus| {
            (
                format!("{}/120x40", corpus.name()),
                120,
                40,
                create_corpus_terminal(120, 40, corpus),
            )
        }),
    );
    bench.run(
        c,
        "render_large",
        &[(
            "300x100_filled".to_string(),
            300,
            100,
            create_filled_terminal(300, 100),
        )],
    );
    bench.run(
        c,
        "render_code",
        &[(
            "300x100_code".to_string(),
            300,
            100,
            create_code_terminal(300, 100),
        )],
    );
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
