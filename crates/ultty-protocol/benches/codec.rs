//! Benchmarks for protocol encode/decode.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use ultty_protocol::{
    AppMessage, Color, ComputedRegionLayout, Constraints, Content, Decode, Dimension, Display,
    Encode, FlexDirection, FrameId, LayoutTrigger, NamedColor, RegionId, RichText, Span, Style,
    TerminalMessage, TextAlign, TextContent, TextOverflow, TextWrap,
};

fn create_simple_message() -> AppMessage {
    AppMessage::SetContent {
        id: RegionId::new(1),
        content: Content::Text(TextContent {
            text: "Hello, World!".to_string(),
            wrap: TextWrap::Word,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    }
}

fn create_complex_message() -> AppMessage {
    AppMessage::CreateRegion {
        id: RegionId::new(42),
        parent: RegionId::ROOT,
        insert_index: -1,
        constraints: Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(24.0),
            ..Default::default()
        },
        style: Style {
            fg: Some(Color::Named(NamedColor::White)),
            bg: Some(Color::Rgb(30, 30, 46)),
            bold: true,
            opacity: 0.95,
            ..Default::default()
        },
    }
}

fn create_rich_text_message() -> AppMessage {
    let spans: Vec<Span> = (0..100)
        .map(|i| Span {
            text: format!("Span {} with some text content. ", i),
            style: Style {
                fg: Some(Color::Indexed((i % 256) as u8)),
                bold: i % 2 == 0,
                italic: i % 3 == 0,
                underline: i % 5 == 0,
                ..Default::default()
            },
        })
        .collect();

    AppMessage::SetContent {
        id: RegionId::new(1),
        content: Content::RichText(RichText { spans }),
    }
}

fn create_layout_response(region_count: usize) -> TerminalMessage {
    let regions: Vec<ComputedRegionLayout> = (0..region_count)
        .map(|i| ComputedRegionLayout {
            id: RegionId::new(i as u32),
            x: (i % 10) as f32 * 10.0,
            y: (i / 10) as f32 * 20.0,
            width: 100.0,
            height: 50.0,
            content_x: 5.0,
            content_y: 5.0,
            content_width: 90.0,
            content_height: 40.0,
            visible: true,
        })
        .collect();

    TerminalMessage::LayoutComputed {
        frame_id: Some(FrameId(1)),
        trigger: LayoutTrigger::ConstraintChange,
        regions,
    }
}

fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol_encode");

    let simple = create_simple_message();
    group.bench_function("simple_message", |b| {
        b.iter(|| black_box(&simple).encode_to_bytes())
    });

    let complex = create_complex_message();
    group.bench_function("complex_message", |b| {
        b.iter(|| black_box(&complex).encode_to_bytes())
    });

    let rich = create_rich_text_message();
    let rich_bytes = rich.encode_to_bytes();
    group.throughput(Throughput::Bytes(rich_bytes.len() as u64));
    group.bench_function("rich_text_100_spans", |b| {
        b.iter(|| black_box(&rich).encode_to_bytes())
    });

    let layout_10 = create_layout_response(10);
    group.bench_function("layout_10_regions", |b| {
        b.iter(|| black_box(&layout_10).encode_to_bytes())
    });

    let layout_100 = create_layout_response(100);
    group.bench_function("layout_100_regions", |b| {
        b.iter(|| black_box(&layout_100).encode_to_bytes())
    });

    let layout_1000 = create_layout_response(1000);
    group.bench_function("layout_1000_regions", |b| {
        b.iter(|| black_box(&layout_1000).encode_to_bytes())
    });

    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol_decode");

    let simple_bytes = create_simple_message().encode_to_bytes();
    group.bench_function("simple_message", |b| {
        b.iter(|| {
            let mut cursor = simple_bytes.as_ref();
            AppMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    let complex_bytes = create_complex_message().encode_to_bytes();
    group.bench_function("complex_message", |b| {
        b.iter(|| {
            let mut cursor = complex_bytes.as_ref();
            AppMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    let rich_bytes = create_rich_text_message().encode_to_bytes();
    group.throughput(Throughput::Bytes(rich_bytes.len() as u64));
    group.bench_function("rich_text_100_spans", |b| {
        b.iter(|| {
            let mut cursor = rich_bytes.as_ref();
            AppMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    let layout_10_bytes = create_layout_response(10).encode_to_bytes();
    group.bench_function("layout_10_regions", |b| {
        b.iter(|| {
            let mut cursor = layout_10_bytes.as_ref();
            TerminalMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    let layout_100_bytes = create_layout_response(100).encode_to_bytes();
    group.bench_function("layout_100_regions", |b| {
        b.iter(|| {
            let mut cursor = layout_100_bytes.as_ref();
            TerminalMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    let layout_1000_bytes = create_layout_response(1000).encode_to_bytes();
    group.bench_function("layout_1000_regions", |b| {
        b.iter(|| {
            let mut cursor = layout_1000_bytes.as_ref();
            TerminalMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol_roundtrip");

    let complex = create_complex_message();
    group.bench_function("complex_message", |b| {
        b.iter(|| {
            let bytes = complex.encode_to_bytes();
            let mut cursor = bytes.as_ref();
            AppMessage::decode(black_box(&mut cursor)).unwrap()
        })
    });

    group.finish();
}

criterion_group!(benches, bench_encode, bench_decode, bench_roundtrip);
criterion_main!(benches);
