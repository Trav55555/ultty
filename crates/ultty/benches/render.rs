//! Benchmarks for GPU rendering performance.
//!
//! Measures frame render time against the <5ms target.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use ultty::grid::{Dimensions, Terminal};
use ultty::renderer::{OverlayState, Renderer};

fn create_empty_terminal(cols: u16, rows: u16) -> Terminal {
    Terminal::new(Dimensions::new(cols, rows))
}

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

        if row % 3 == 0 {
            for c in "Hello, World! This is a test line with some content.".chars() {
                grid.write_char(c);
            }
        } else if row % 3 == 1 {
            for c in "$ cargo build --release".chars() {
                grid.write_char(c);
            }
        }
    }

    term
}

fn bench_render_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_empty");

    let rt = tokio::runtime::Runtime::new().unwrap();

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        let mut renderer = rt.block_on(async {
            Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
                .await
                .unwrap()
        });
        let terminal = create_empty_terminal(cols, rows);
        let overlay = OverlayState::default();

        group.bench_with_input(
            BenchmarkId::new("terminal", format!("{}x{}", cols, rows)),
            &(cols, rows),
            |b, _| {
                b.iter(|| {
                    renderer
                        .render_to_texture(black_box(&terminal), black_box(&overlay))
                        .unwrap();
                })
            },
        );
    }

    group.finish();
}

fn bench_render_filled(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_filled");

    let rt = tokio::runtime::Runtime::new().unwrap();

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        let mut renderer = rt.block_on(async {
            Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
                .await
                .unwrap()
        });
        let terminal = create_filled_terminal(cols, rows);
        let overlay = OverlayState::default();

        group.bench_with_input(
            BenchmarkId::new("terminal", format!("{}x{}", cols, rows)),
            &(cols, rows),
            |b, _| {
                b.iter(|| {
                    renderer
                        .render_to_texture(black_box(&terminal), black_box(&overlay))
                        .unwrap();
                })
            },
        );
    }

    group.finish();
}

fn bench_render_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_mixed");

    let rt = tokio::runtime::Runtime::new().unwrap();

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        let mut renderer = rt.block_on(async {
            Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
                .await
                .unwrap()
        });
        let terminal = create_mixed_terminal(cols, rows);
        let overlay = OverlayState::default();

        group.bench_with_input(
            BenchmarkId::new("terminal", format!("{}x{}", cols, rows)),
            &(cols, rows),
            |b, _| {
                b.iter(|| {
                    renderer
                        .render_to_texture(black_box(&terminal), black_box(&overlay))
                        .unwrap();
                })
            },
        );
    }

    group.finish();
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

fn bench_render_code(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_code");
    group.sample_size(30);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (cols, rows) = (300u16, 100u16);
    let mut renderer = rt.block_on(async {
        Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
            .await
            .unwrap()
    });
    let terminal = create_code_terminal(cols, rows);
    let overlay = OverlayState::default();
    group.bench_function("300x100_code", |b| {
        b.iter(|| {
            renderer
                .render_to_texture(black_box(&terminal), black_box(&overlay))
                .unwrap();
        })
    });
    group.finish();
}

fn bench_render_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_large");
    group.sample_size(20);

    let rt = tokio::runtime::Runtime::new().unwrap();

    let cols = 300u16;
    let rows = 100u16;
    let mut renderer = rt.block_on(async {
        Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
            .await
            .unwrap()
    });
    let terminal = create_filled_terminal(cols, rows);
    let overlay = OverlayState::default();

    group.bench_function("300x100_filled", |b| {
        b.iter(|| {
            renderer
                .render_to_texture(black_box(&terminal), black_box(&overlay))
                .unwrap();
        })
    });

    group.finish();
}

/// A full screen of code where one row changes between frames, as when a
/// shell echoes a keystroke or a status line ticks: the frame should cost
/// about one row of text work, not a whole screen.
fn bench_render_one_dirty_row(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_one_dirty_row");
    group.sample_size(30);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (cols, rows) = (300u16, 75u16);
    let mut renderer = rt.block_on(async {
        Renderer::new_headless(cols as u32 * 10, rows as u32 * 20)
            .await
            .unwrap()
    });
    let mut terminal = create_code_terminal(cols, rows);
    let overlay = OverlayState::default();
    let mut frame = 0u16;
    group.bench_function("300x75_code", |b| {
        b.iter(|| {
            frame = frame.wrapping_add(1);
            let grid = terminal.grid_mut();
            grid.set_cursor_position(frame % cols, frame % rows);
            grid.write_char(char::from(b'a' + (frame % 26) as u8));
            renderer
                .render_to_texture(black_box(&terminal), black_box(&overlay))
                .unwrap();
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_render_empty,
    bench_render_filled,
    bench_render_mixed,
    bench_render_large,
    bench_render_code,
    bench_render_one_dirty_row
);
criterion_main!(benches);
