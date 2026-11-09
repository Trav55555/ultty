//! Benchmarks for Taffy layout computation.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ultty::scene::layout::LayoutEngine;
use ultty::scene::{Region, SceneTree};
use ultty_protocol::{Constraints, Dimension, Display, FlexDirection, RegionId};

fn create_flat_tree(count: usize) -> SceneTree {
    let mut tree = SceneTree::new();

    for i in 1..=count {
        let mut region = Region::new(RegionId::new(i as u32));
        region.parent = Some(RegionId::ROOT);
        region.constraints = Constraints {
            display: Display::Block,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            ..Default::default()
        };
        tree.insert(region);
    }

    tree
}

fn create_nested_tree(depth: usize, branching: usize) -> SceneTree {
    let mut tree = SceneTree::new();
    let mut next_id = 1u32;

    fn add_children(
        tree: &mut SceneTree,
        parent_id: RegionId,
        depth: usize,
        branching: usize,
        next_id: &mut u32,
    ) {
        if depth == 0 {
            return;
        }

        for _ in 0..branching {
            let id = RegionId::new(*next_id);
            *next_id += 1;

            let mut region = Region::new(id);
            region.parent = Some(parent_id);
            region.constraints = Constraints {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                ..Default::default()
            };
            tree.insert(region);

            add_children(tree, id, depth - 1, branching, next_id);
        }
    }

    add_children(&mut tree, RegionId::ROOT, depth, branching, &mut next_id);
    tree
}

fn create_flexbox_tree(rows: usize, cols: usize) -> SceneTree {
    let mut tree = SceneTree::new();
    let mut next_id = 1u32;

    if let Some(root) = tree.get_mut(RegionId::ROOT) {
        root.constraints = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            ..Default::default()
        };
    }

    for _ in 0..rows {
        let row_id = RegionId::new(next_id);
        next_id += 1;

        let mut row = Region::new(row_id);
        row.parent = Some(RegionId::ROOT);
        row.constraints = Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        };
        tree.insert(row);

        for _ in 0..cols {
            let cell_id = RegionId::new(next_id);
            next_id += 1;

            let mut cell = Region::new(cell_id);
            cell.parent = Some(row_id);
            cell.constraints = Constraints {
                display: Display::Block,
                flex_grow: 1.0,
                ..Default::default()
            };
            tree.insert(cell);
        }
    }

    tree
}

fn bench_layout_sync(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout_sync");

    for count in [10, 100, 500, 1000] {
        let tree = create_flat_tree(count);
        group.bench_function(format!("flat_{}_regions", count), |b| {
            let mut engine = LayoutEngine::new();
            b.iter(|| {
                engine.sync(black_box(&tree)).expect("layout sync failed");
            })
        });
    }

    for (depth, branching, count) in [(3, 3, 40), (4, 3, 121), (5, 2, 63)] {
        let tree = create_nested_tree(depth, branching);
        group.bench_function(
            format!("nested_depth{}_branch{}_{}", depth, branching, count),
            |b| {
                let mut engine = LayoutEngine::new();
                b.iter(|| {
                    engine.sync(black_box(&tree)).expect("layout sync failed");
                })
            },
        );
    }

    group.finish();
}

/// Relayout after something changed. Re-running `compute` on an unchanged
/// tree at the same size mostly measures Taffy's cache, which is not what a
/// frame pays, so every case here invalidates first:
///
/// - `relayout_dirty_leaf`: one leaf's constraints change (a `SetConstraints`
///   diff), then sync and compute, as the protocol server does.
/// - `relayout_resize`: the window size alternates between two widths, so
///   every node's available space changes.
/// - `cached_same_size`: the old measurement, kept as the floor for
///   comparison.
fn bench_layout_compute(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout_compute");

    let mut trees: Vec<(String, SceneTree, RegionId)> = Vec::new();
    for count in [10, 100, 500, 1000] {
        trees.push((
            format!("flat_{count}_regions"),
            create_flat_tree(count),
            RegionId::new(count as u32 / 2),
        ));
    }
    for (rows, cols) in [(10, 10), (20, 20), (50, 10)] {
        let total = rows * cols + rows + 1;
        // The middle cell of the middle row.
        let leaf = (rows / 2) * (cols + 1) + cols / 2 + 2;
        trees.push((
            format!("flexbox_{rows}x{cols}_{total}_regions"),
            create_flexbox_tree(rows, cols),
            RegionId::new(leaf as u32),
        ));
    }

    for (name, tree, leaf) in &mut trees {
        assert!(tree.get(*leaf).is_some(), "{name}: leaf {leaf:?} missing");
        let mut engine = LayoutEngine::new();
        engine.sync(tree).expect("layout sync failed");
        engine
            .compute(1920.0, 1080.0)
            .expect("layout compute failed");

        group.bench_function(format!("relayout_dirty_leaf/{name}"), |b| {
            let mut tall = false;
            b.iter(|| {
                tall = !tall;
                if let Some(region) = tree.get_mut(*leaf) {
                    region.constraints.height = Dimension::Cells(if tall { 3.0 } else { 1.0 });
                }
                engine.sync(black_box(tree)).expect("layout sync failed");
                engine
                    .compute(black_box(1920.0), black_box(1080.0))
                    .expect("layout compute failed");
            })
        });

        group.bench_function(format!("relayout_resize/{name}"), |b| {
            let mut wide = false;
            b.iter(|| {
                wide = !wide;
                let width = if wide { 1920.0 } else { 1600.0 };
                engine
                    .compute(black_box(width), black_box(1080.0))
                    .expect("layout compute failed");
            })
        });

        group.bench_function(format!("cached_same_size/{name}"), |b| {
            b.iter(|| {
                engine
                    .compute(black_box(1920.0), black_box(1080.0))
                    .expect("layout compute failed");
            })
        });
    }

    group.finish();
}

fn bench_layout_full_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout_full_cycle");

    for (rows, cols) in [(10, 10), (20, 10)] {
        let tree = create_flexbox_tree(rows, cols);
        let total = rows * cols + rows + 1;

        group.bench_function(
            format!("flexbox_{}x{}_{}_regions", rows, cols, total),
            |b| {
                b.iter(|| {
                    let mut engine = LayoutEngine::new();
                    engine.sync(black_box(&tree)).expect("layout sync failed");
                    engine
                        .compute(black_box(1920.0), black_box(1080.0))
                        .expect("layout compute failed");
                })
            },
        );
    }

    group.finish();
}

/// One streaming frame of an 8 x 120 inline surface (ADR 0020): a bordered
/// card with a title and six lines of tool output, rasterised into cells
/// and written into its rows, on the screen and in scrollback.
fn bench_surface_raster(c: &mut Criterion) {
    use ultty::grid::{Dimensions, Terminal};
    use ultty::scene::raster::rasterise_surface;
    use ultty_protocol::{BorderStyle, Content, Edges, Style, TextContent};

    const COLS: u16 = 120;
    const ROWS: u16 = 8;
    let card = RegionId::new(100);
    let log = RegionId::new(102);
    let mut tree = SceneTree::new();
    let mut root = Region::new(card);
    root.style = Style {
        border: BorderStyle::thin(),
        ..Style::default()
    };
    root.constraints = Constraints {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        padding: Edges {
            top: Dimension::Cells(1.0),
            right: Dimension::Cells(1.0),
            bottom: Dimension::Cells(1.0),
            left: Dimension::Cells(1.0),
        },
        ..Default::default()
    };
    tree.try_insert_surface(root, ROWS).expect("surface");
    for (id, height, text) in [(101, 1.0, "cargo test --workspace"), (102, 5.0, "")] {
        let mut region = Region::new(RegionId::new(id));
        region.parent = Some(card);
        region.constraints = Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(height),
            ..Default::default()
        };
        region.content = Content::Text(TextContent::new(text));
        tree.insert(region);
    }
    let mut engine = LayoutEngine::new();
    engine
        .compute_scene_layout(&mut tree, f32::from(COLS), 40.0)
        .expect("layout");

    // Each frame appends a line of output and shows the last five.
    let output: Vec<String> = (0..64)
        .map(|n| {
            format!("test scene::raster::tests::case_{n:03} ... ok (streamed output line {n})")
        })
        .collect();
    let mut frame = 0usize;
    let mut next_frame = |tree: &mut SceneTree| {
        frame = (frame + 1) % (output.len() - 5);
        if let Some(region) = tree.get_mut(log) {
            region.content = Content::Text(TextContent::new(output[frame..frame + 5].join("\n")));
        }
    };

    let mut group = c.benchmark_group("surface_raster");
    group.bench_function("rasterise_8x120", |b| {
        b.iter(|| {
            next_frame(&mut tree);
            black_box(rasterise_surface(&tree, card, COLS, ROWS, |_| (0.0, 0.0)))
        })
    });

    let mut terminal = Terminal::new(Dimensions::new(COLS, 40));
    let history = 1000;
    for _ in 0..history {
        terminal.buffer_mut().newline();
    }
    let on_screen = terminal.main_buffer().history_len() + 10;
    for (name, first) in [("screen", on_screen), ("scrollback", 10)] {
        group.bench_function(format!("rasterise_and_write_8x120/{name}"), |b| {
            b.iter(|| {
                next_frame(&mut tree);
                let rows = rasterise_surface(&tree, card, COLS, ROWS, |_| (0.0, 0.0));
                let buffer = terminal.main_buffer_mut();
                for (offset, row) in rows.iter().enumerate() {
                    buffer.replace_row(first + offset, row);
                }
            })
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_layout_sync,
    bench_layout_compute,
    bench_layout_full_cycle,
    bench_surface_raster
);
criterion_main!(benches);
