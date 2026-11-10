//! Benchmarks for grid operations.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ultty::grid::{Dimensions, Grid};

fn create_grid(cols: u16, rows: u16) -> Grid {
    Grid::new(Dimensions::new(cols, rows))
}

fn bench_grid_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_write");

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        group.bench_function(format!("write_cell_{}x{}", cols, rows), |b| {
            let mut grid = create_grid(cols, rows);
            b.iter(|| {
                for row in 0..rows {
                    for col in 0..cols {
                        grid.update_cell(black_box(col), black_box(row), |cell| cell.set_char('A'));
                    }
                }
            })
        });
    }

    group.finish();
}

fn bench_grid_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_read");

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        let mut grid = create_grid(cols, rows);
        // Fill with data
        for row in 0..rows {
            for col in 0..cols {
                grid.update_cell(col, row, |cell| cell.set_char('X'));
            }
        }

        group.bench_function(format!("read_cell_{}x{}", cols, rows), |b| {
            b.iter(|| {
                let mut count = 0u32;
                for row in 0..rows {
                    for col in 0..cols {
                        if let Some(cell) = grid.cell(black_box(col), black_box(row)) {
                            if cell.char() == 'X' {
                                count += 1;
                            }
                        }
                    }
                }
                count
            })
        });
    }

    group.finish();
}

fn bench_grid_row_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_row_access");

    for (cols, rows) in [(80, 24), (120, 40)] {
        let mut grid = create_grid(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                grid.update_cell(col, row, |cell| cell.set_char('R'));
            }
        }

        group.bench_function(format!("row_iter_{}x{}", cols, rows), |b| {
            b.iter(|| {
                let mut count = 0u32;
                for row_idx in 0..rows {
                    if let Some(row) = grid.row(black_box(row_idx)) {
                        for col in 0..cols {
                            if let Some(cell) = row.cell(col) {
                                if cell.char() != '\0' {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                count
            })
        });
    }

    group.finish();
}

fn bench_grid_scroll(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_scroll");

    let (cols, rows) = (80, 24);

    group.bench_function("scroll_up_1", |b| {
        let mut grid = create_grid(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                grid.update_cell(col, row, |cell| {
                    cell.set_char((b'A' + (row % 26) as u8) as char)
                });
            }
        }

        b.iter(|| {
            grid.scroll_up(black_box(1));
        })
    });

    group.bench_function("scroll_down_1", |b| {
        let mut grid = create_grid(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                grid.update_cell(col, row, |cell| {
                    cell.set_char((b'A' + (row % 26) as u8) as char)
                });
            }
        }

        b.iter(|| {
            grid.scroll_down(black_box(1));
        })
    });

    group.bench_function("scroll_up_10", |b| {
        let mut grid = create_grid(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                grid.update_cell(col, row, |cell| cell.set_char('S'));
            }
        }

        b.iter(|| {
            grid.scroll_up(black_box(10));
        })
    });

    group.finish();
}

fn bench_grid_clear(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_clear");

    for (cols, rows) in [(80, 24), (120, 40), (200, 60)] {
        group.bench_function(format!("clear_{}x{}", cols, rows), |b| {
            let mut grid = create_grid(cols, rows);
            // Fill first
            for row in 0..rows {
                for col in 0..cols {
                    grid.update_cell(col, row, |cell| cell.set_char('C'));
                }
            }

            b.iter(|| {
                grid.clear();
            })
        });
    }

    group.finish();
}

fn bench_grid_resize(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid_resize");

    group.bench_function("resize_grow", |b| {
        b.iter(|| {
            let mut grid = create_grid(80, 24);
            grid.resize(Dimensions::new(black_box(120), black_box(40)));
        })
    });

    group.bench_function("resize_shrink", |b| {
        b.iter(|| {
            let mut grid = create_grid(120, 40);
            grid.resize(Dimensions::new(black_box(80), black_box(24)));
        })
    });

    group.bench_function("resize_same", |b| {
        b.iter(|| {
            let mut grid = create_grid(80, 24);
            grid.resize(Dimensions::new(black_box(80), black_box(24)));
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_grid_write,
    bench_grid_read,
    bench_grid_row_access,
    bench_grid_scroll,
    bench_grid_clear,
    bench_grid_resize
);
criterion_main!(benches);
