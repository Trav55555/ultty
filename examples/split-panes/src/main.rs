use std::time::Duration;

use ultty_client::UlttyClient;
use ultty_protocol::{
    BorderStyle, CodeWidget, Color, Constraints, Content, Dimension, Display, FlexDirection,
    InputType, Key, NamedColor, RegionId, Style, TerminalMessage, TextAlign, TextContent,
    TextOverflow, TextWrap, WidgetContent, EVENT_KEY,
};

/// Which pane currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPane {
    Left,
    Main,
    Right,
}

/// Border color for an unfocused pane.
fn dim_border_color() -> Color {
    Color::Named(NamedColor::BrightBlack)
}

/// Border color for the focused pane.
fn bright_border_color() -> Color {
    Color::Named(NamedColor::BrightCyan)
}

/// Build the style for a pane given its border type and whether it is focused.
fn pane_style(border: BorderStyle, focused: bool) -> Style {
    Style {
        border,
        border_color: Some(if focused {
            bright_border_color()
        } else {
            dim_border_color()
        }),
        ..Default::default()
    }
}

/// Re-apply styles to the three pane regions based on current focus.
fn update_focus(
    client: &mut UlttyClient,
    focused: FocusedPane,
    left_region: RegionId,
    main_region: RegionId,
    right_region: RegionId,
    status_region: RegionId,
) -> anyhow::Result<()> {
    let frame = client.begin_frame()?;

    client.set_style(
        left_region,
        pane_style(BorderStyle::thin(), focused == FocusedPane::Left),
    )?;
    client.set_style(
        main_region,
        pane_style(BorderStyle::rounded(), focused == FocusedPane::Main),
    )?;
    client.set_style(
        right_region,
        pane_style(BorderStyle::thin(), focused == FocusedPane::Right),
    )?;

    // Update status bar to reflect current focus
    let label = match focused {
        FocusedPane::Left => "Files",
        FocusedPane::Main => "Editor",
        FocusedPane::Right => "Outline",
    };
    client.set_content(
        status_region,
        Content::Text(TextContent {
            text: format!(" [{}]  Press 1/2/3 to focus pane | q to quit", label),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    client.end_frame(frame)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("split_panes=debug,info")
        .init();

    tracing::info!("Split Panes - Flex Layout Demo");

    let mut client = UlttyClient::connect()?;

    // Subscribe to keyboard events on the root region
    client.subscribe(client.root_region(), EVENT_KEY)?;

    let frame = client.begin_frame()?;

    // ── Outermost container: column (body row + status bar) ──────────
    let root_col = client.create_region(
        client.root_region(),
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            ..Default::default()
        },
        Style::default(),
    )?;

    // ── Panes row: left | main | right ──────────────────────────────
    let panes_row = client.create_region(
        root_col,
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style::default(),
    )?;

    // ── Left sidebar (file tree) ────────────────────────────────────
    let left_region = client.create_region(
        panes_row,
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Cells(25.0),
            height: Dimension::Percent(100.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        pane_style(BorderStyle::thin(), true), // starts focused
    )?;

    // Title for the left sidebar
    let left_title = client.create_region(
        left_region,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::BrightYellow)),
            bold: true,
            ..Default::default()
        },
    )?;
    client.set_content(
        left_title,
        Content::Text(TextContent {
            text: " FILES".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    // File listing
    let left_body = client.create_region(
        left_region,
        Constraints {
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::White)),
            ..Default::default()
        },
    )?;
    client.set_content(
        left_body,
        Content::Text(TextContent {
            text: [
                "  src/",
                "    main.rs",
                "    lib.rs",
                "    config.rs",
                "    utils/",
                "      helpers.rs",
                "      math.rs",
                "  tests/",
                "    integration.rs",
                "  Cargo.toml",
                "  README.md",
                "  .gitignore",
            ]
            .join("\n"),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    // ── Main editor area ────────────────────────────────────────────
    let main_region = client.create_region(
        panes_row,
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            height: Dimension::Percent(100.0),
            ..Default::default()
        },
        pane_style(BorderStyle::rounded(), false),
    )?;

    // Editor title bar
    let main_title = client.create_region(
        main_region,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::BrightGreen)),
            bold: true,
            ..Default::default()
        },
    )?;
    client.set_content(
        main_title,
        Content::Text(TextContent {
            text: " EDITOR  src/main.rs".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    // Code content
    let main_body = client.create_region(
        main_region,
        Constraints {
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style::default(),
    )?;
    client.set_content(
        main_body,
        Content::Widget(WidgetContent::Code(CodeWidget {
            source: r#"use std::collections::HashMap;

/// Application configuration loaded from disk.
#[derive(Debug, Clone)]
pub struct Config {
    pub name: String,
    pub version: String,
    pub settings: HashMap<String, String>,
}

impl Config {
    /// Load configuration from the default path.
    pub fn load() -> anyhow::Result<Self> {
        let path = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("no config dir"))?
            .join("myapp/config.toml");

        let content = std::fs::read_to_string(&path)?;
        let settings: HashMap<String, String> =
            toml::from_str(&content)?;

        Ok(Self {
            name: "myapp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            settings,
        })
    }
}

fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    println!("Loaded config: {:?}", config);
    Ok(())
}"#
            .to_string(),
            language: "rust".to_string(),
            line_numbers: true,
        })),
    )?;

    // ── Right sidebar (outline) ─────────────────────────────────────
    let right_region = client.create_region(
        panes_row,
        Constraints {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            width: Dimension::Cells(20.0),
            height: Dimension::Percent(100.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        pane_style(BorderStyle::thin(), false),
    )?;

    // Outline title
    let right_title = client.create_region(
        right_region,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::BrightMagenta)),
            bold: true,
            ..Default::default()
        },
    )?;
    client.set_content(
        right_title,
        Content::Text(TextContent {
            text: " OUTLINE".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    // Outline listing
    let right_body = client.create_region(
        right_region,
        Constraints {
            width: Dimension::Percent(100.0),
            flex_grow: 1.0,
            ..Default::default()
        },
        Style {
            fg: Some(Color::Named(NamedColor::White)),
            dim: true,
            ..Default::default()
        },
    )?;
    client.set_content(
        right_body,
        Content::Text(TextContent {
            text: [
                "  struct Config",
                "    name",
                "    version",
                "    settings",
                "  impl Config",
                "    fn load()",
                "  fn main()",
            ]
            .join("\n"),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    // ── Status bar ──────────────────────────────────────────────────
    let status_region = client.create_region(
        root_col,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        Style {
            bg: Some(Color::Named(NamedColor::Blue)),
            fg: Some(Color::Named(NamedColor::BrightWhite)),
            bold: true,
            ..Default::default()
        },
    )?;
    client.set_content(
        status_region,
        Content::Text(TextContent {
            text: " [Files]  Press 1/2/3 to focus pane | q to quit".to_string(),
            wrap: TextWrap::None,
            align: TextAlign::Left,
            overflow: TextOverflow::Clip,
        }),
    )?;

    client.end_frame(frame)?;

    tracing::info!("Layout rendered. Waiting for key events...");

    let mut focused = FocusedPane::Left;

    loop {
        std::thread::sleep(Duration::from_millis(50));
        for event in client.poll_events()? {
            match event {
                TerminalMessage::Input(input) => {
                    if let InputType::KeyDown { key, .. } = &input.event_type {
                        let new_focus = match key {
                            Key::Char('1') => Some(FocusedPane::Left),
                            Key::Char('2') => Some(FocusedPane::Main),
                            Key::Char('3') => Some(FocusedPane::Right),
                            Key::Char('q') | Key::Escape => {
                                tracing::info!("Quitting...");
                                client.disconnect()?;
                                return Ok(());
                            }
                            _ => None,
                        };

                        if let Some(new) = new_focus {
                            if new != focused {
                                focused = new;
                                tracing::info!("Focus changed to {:?}", focused);
                                update_focus(
                                    &mut client,
                                    focused,
                                    left_region,
                                    main_region,
                                    right_region,
                                    status_region,
                                )?;
                            }
                        }
                    }
                }
                TerminalMessage::Resize(resize) => {
                    tracing::info!("Resize: {}x{}", resize.width, resize.height);
                }
                _ => {}
            }
        }
    }
}
