use std::time::Duration;

use ultty_client::UlttyClient;
use ultty_protocol::{
    CodeWidget, Color, Constraints, Content, Dimension, Display, FlexDirection, MarkdownWidget,
    NamedColor, ProgressWidget, SpinnerWidget, Style, TerminalMessage, TextAlign, TextContent,
    TextOverflow, TextWrap, WidgetContent, EVENT_KEY,
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("hello_native=debug,info")
        .init();

    tracing::info!("Hello Native - Ultty Protocol Demo");

    let mut client = UlttyClient::connect()?;

    // Subscribe to key events on the root region so we receive keyboard input
    client.subscribe(client.root_region(), EVENT_KEY)?;

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
            bg: Some(Color::Named(NamedColor::Blue)),
            opacity: 0.3,
            ..Default::default()
        },
    )?;

    let header = client.create_region(
        container,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(3.0),
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Green)),
            fg: Some(Color::Named(NamedColor::Black)),
            ..Default::default()
        },
    )?;

    client.set_content(
        header,
        Content::Text(TextContent {
            text: "  Hello from Native Protocol!".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    let body = client.create_region(
        container,
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style::default(),
    )?;

    // --- Progress bar ---
    let progress_region = client.create_region(
        body,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(2.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Red)),
            opacity: 0.3,
            ..Default::default()
        },
    )?;

    client.set_content(
        progress_region,
        Content::Widget(WidgetContent::Progress(ProgressWidget {
            value: 0.65,
            label: Some("Downloading... 65%".to_string()),
            animated: true,
            indeterminate: false,
        })),
    )?;

    // --- Spinner ---
    let spinner_region = client.create_region(
        body,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(2.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Cyan)),
            opacity: 0.3,
            ..Default::default()
        },
    )?;

    client.set_content(
        spinner_region,
        Content::Widget(WidgetContent::Spinner(SpinnerWidget {
            label: Some("Loading resources...".to_string()),
            ..Default::default()
        })),
    )?;

    // --- Markdown ---
    let markdown_region = client.create_region(
        body,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(6.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style::default(),
    )?;

    client.set_content(
        markdown_region,
        Content::Widget(WidgetContent::Markdown(MarkdownWidget {
            source: "# Hello Markdown\n\nThis is **bold** and *italic* text rendered via the native protocol.\n\n- Item one\n- Item two".to_string(),
        })),
    )?;

    // --- Code block ---
    let code_region = client.create_region(
        body,
        Constraints {
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style::default(),
    )?;

    client.set_content(
        code_region,
        Content::Widget(WidgetContent::Code(CodeWidget {
            source: "fn main() {\n    println!(\"Hello from native protocol!\");\n}".to_string(),
            language: "rust".to_string(),
            line_numbers: true,
        })),
    )?;

    let footer = client.create_region(
        container,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::White)),
            fg: Some(Color::Named(NamedColor::Black)),
            ..Default::default()
        },
    )?;

    client.set_content(
        footer,
        Content::Text(TextContent {
            text: "Press Ctrl+C to exit".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Center,
            overflow: TextOverflow::Clip,
        }),
    )?;

    client.end_frame(frame)?;

    tracing::info!("UI rendered. Waiting for events...");

    loop {
        std::thread::sleep(Duration::from_millis(100));
        for event in client.poll_events()? {
            match event {
                TerminalMessage::Input(input) => {
                    tracing::info!("Input: {:?}", input);
                }
                TerminalMessage::Resize(resize) => {
                    tracing::info!("Resize: {}x{}", resize.width, resize.height);
                }
                _ => {}
            }
        }
    }
}
