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

/// Every case sets its own throughput: criterion keeps a group's last
/// `throughput` for all later benches, so one shared setting reports the
/// byte count of a different message.
fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol_encode");

    let app_cases = [
        ("simple_message", create_simple_message()),
        ("complex_message", create_complex_message()),
        ("rich_text_100_spans", create_rich_text_message()),
    ];
    for (name, message) in &app_cases {
        group.throughput(Throughput::Bytes(message.encode_to_bytes().len() as u64));
        group.bench_function(*name, |b| b.iter(|| black_box(message).encode_to_bytes()));
    }

    for count in [10, 100, 1000] {
        let message = create_layout_response(count);
        group.throughput(Throughput::Bytes(message.encode_to_bytes().len() as u64));
        group.bench_function(format!("layout_{count}_regions"), |b| {
            b.iter(|| black_box(&message).encode_to_bytes())
        });
    }

    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("protocol_decode");

    let app_cases = [
        ("simple_message", create_simple_message().encode_to_bytes()),
        (
            "complex_message",
            create_complex_message().encode_to_bytes(),
        ),
        (
            "rich_text_100_spans",
            create_rich_text_message().encode_to_bytes(),
        ),
    ];
    for (name, bytes) in &app_cases {
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(*name, |b| {
            b.iter(|| {
                let mut cursor = bytes.as_ref();
                AppMessage::decode(black_box(&mut cursor)).unwrap()
            })
        });
    }

    for count in [10, 100, 1000] {
        let bytes = create_layout_response(count).encode_to_bytes();
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_function(format!("layout_{count}_regions"), |b| {
            b.iter(|| {
                let mut cursor = bytes.as_ref();
                TerminalMessage::decode(black_box(&mut cursor)).unwrap()
            })
        });
    }

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
