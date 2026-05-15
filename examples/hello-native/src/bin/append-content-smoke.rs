use std::time::Duration;

use ultty_client::UlttyClient;
use ultty_protocol::{
    Color, Constraints, Content, Dimension, Display, FlexDirection, NamedColor, Style, TextAlign,
    TextContent, TextOverflow, TextWrap,
};

fn text(value: impl Into<String>) -> Content {
    Content::Text(TextContent {
        text: value.into(),
        wrap: TextWrap::None,
        align: TextAlign::Left,
        overflow: TextOverflow::Clip,
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("append_content_smoke=debug,info")
        .init();

    let mut client = UlttyClient::connect()?;
    let frame = client.begin_frame()?;

    let container = client.create_region(
        client.root_region(),
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Black)),
            ..Default::default()
        },
    )?;

    let title = client.create_region(
        container,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(2.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Green)),
            fg: Some(Color::Named(NamedColor::Black)),
            ..Default::default()
        },
    )?;
    client.set_content(title, text("  AppendContent native smoke"))?;

    let append_region = client.create_region(
        container,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(3.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::White)),
            ..Default::default()
        },
    )?;
    client.set_content(append_region, text("append-content: before"))?;

    let footer = client.create_region(
        container,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::Cyan)),
            ..Default::default()
        },
    )?;
    client.set_content(footer, text("waiting for append..."))?;

    client.end_frame(frame)?;
    tracing::info!("append smoke initial frame rendered");

    std::thread::sleep(Duration::from_millis(1500));
    client.append_content(append_region, " + after")?;
    tracing::info!("append smoke append sent");

    loop {
        std::thread::sleep(Duration::from_millis(250));
        let _ = client.poll_events();
    }
}
