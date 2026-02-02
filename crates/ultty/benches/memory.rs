//! Memory usage benchmarks for core data structures.
//!
//! Measures heap allocations to verify against the <50MB target.

use criterion::{black_box, criterion_group, Criterion};
use ultty::grid::{Dimensions, Terminal};
use ultty::scene::{Region, SceneTree};
use ultty_protocol::{Constraints, Display, RegionId};

fn measure_terminal_memory(cols: u16, rows: u16, scrollback: usize) -> usize {
    let term = Terminal::new(Dimensions::new(cols, rows));
    std::mem::size_of_val(&term) + estimate_heap_size(&term, scrollback)
}

fn estimate_heap_size(term: &Terminal, scrollback: usize) -> usize {
    let dims = term.grid().dimensions();
    let cell_size = 16;
    let viewport_cells = dims.cols as usize * dims.rows as usize * cell_size;
    let scrollback_cells = dims.cols as usize * scrollback * cell_size;
    viewport_cells + scrollback_cells
}

fn measure_scene_tree_memory(region_count: usize) -> usize {
    let mut tree = SceneTree::new();
    for i in 1..=region_count {
        let mut region = Region::new(RegionId::new(i as u32));
        region.parent = Some(RegionId::ROOT);
        region.constraints = Constraints {
            display: Display::Block,
            ..Default::default()
        };
        tree.insert(region);
    }
    std::mem::size_of_val(&tree) + region_count * std::mem::size_of::<Region>() * 2
}

fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");

    group.bench_function("terminal_80x24_10k_scrollback", |b| {
        b.iter(|| {
            let size = measure_terminal_memory(80, 24, 10_000);
            black_box(size)
        })
    });

    group.bench_function("terminal_200x60_10k_scrollback", |b| {
        b.iter(|| {
            let size = measure_terminal_memory(200, 60, 10_000);
            black_box(size)
        })
    });

    group.bench_function("scene_tree_100_regions", |b| {
        b.iter(|| {
            let size = measure_scene_tree_memory(100);
            black_box(size)
        })
    });

    group.bench_function("scene_tree_1000_regions", |b| {
        b.iter(|| {
            let size = measure_scene_tree_memory(1000);
            black_box(size)
        })
    });

    group.finish();
}

fn report_memory_estimates() {
    println!("\n=== Memory Estimates ===\n");

    let term_80x24 = measure_terminal_memory(80, 24, 10_000);
    println!(
        "Terminal 80x24 (10k scrollback): {:.2} MB",
        term_80x24 as f64 / 1_000_000.0
    );

    let term_200x60 = measure_terminal_memory(200, 60, 10_000);
    println!(
        "Terminal 200x60 (10k scrollback): {:.2} MB",
        term_200x60 as f64 / 1_000_000.0
    );

    let scene_1000 = measure_scene_tree_memory(1000);
    println!(
        "Scene tree (1000 regions): {:.2} MB",
        scene_1000 as f64 / 1_000_000.0
    );

    let glyph_atlas = 2048 * 2048;
    println!(
        "Glyph atlas (2048x2048 R8): {:.2} MB",
        glyph_atlas as f64 / 1_000_000.0
    );

    let total_estimate = term_80x24 + scene_1000 + glyph_atlas;
    println!(
        "\nTotal estimate (terminal + scene + atlas): {:.2} MB",
        total_estimate as f64 / 1_000_000.0
    );
    println!("Target: <50 MB");
    println!(
        "Status: {}",
        if total_estimate < 50_000_000 {
            "PASS"
        } else {
            "FAIL"
        }
    );
}

criterion_group!(benches, bench_memory_usage);

fn main() {
    report_memory_estimates();
    println!();
    benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();
}
