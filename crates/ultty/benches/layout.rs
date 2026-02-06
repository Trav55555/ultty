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

fn bench_layout_compute(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout_compute");

    for count in [10, 100, 500, 1000] {
        let tree = create_flat_tree(count);
        let mut engine = LayoutEngine::new();
        engine.sync(&tree).expect("layout sync failed");

        group.bench_function(format!("flat_{}_regions", count), |b| {
            b.iter(|| {
                engine
                    .compute(black_box(1920.0), black_box(1080.0))
                    .expect("layout compute failed");
            })
        });
    }

    for (rows, cols) in [(10, 10), (20, 20), (50, 10)] {
        let tree = create_flexbox_tree(rows, cols);
        let mut engine = LayoutEngine::new();
        engine.sync(&tree).expect("layout sync failed");
        let total = rows * cols + rows + 1;

        group.bench_function(
            format!("flexbox_{}x{}_{}_regions", rows, cols, total),
            |b| {
                b.iter(|| {
                    engine
                        .compute(black_box(1920.0), black_box(1080.0))
                        .expect("layout compute failed");
                })
            },
        );
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

criterion_group!(
    benches,
    bench_layout_sync,
    bench_layout_compute,
    bench_layout_full_cycle
);
criterion_main!(benches);
