//! Protocol v1 conformance against wire vectors maintained outside the codec.

use serde::Deserialize;

use ultty_protocol::{
    AppMessage, BorderStyle, CodeWidget, Color, ComputedRegionLayout, Constraints, Content, Decode,
    Dimension, Display, Encode, FlexDirection, FrameId, InputEvent, InputType, Key, LayoutTrigger,
    Live, MarkdownWidget, Modifiers, NamedColor, Overflow, OverflowAxis, ProtocolError, RegionId,
    ResizeEvent, RichText, Role, Semantics, Span, Style, SurfaceEventKind, TerminalMessage,
    TextContent, WidgetContent,
};

#[derive(Deserialize)]
struct Vector {
    name: String,
    direction: Direction,
    hex: String,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    App,
    Terminal,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Expected {
    ClientHello {
        version: u16,
        requested_capabilities: u32,
    },
    ServerHello {
        version: u16,
        capabilities: u32,
    },
    SessionReady {
        session_id: String,
        root_region: u32,
        width: u16,
        height: u16,
    },
    BeginFrame {
        frame_id: u32,
    },
    EndFrame {
        frame_id: u32,
    },
    CreateRegion {
        id: u32,
        parent: u32,
        insert_index: i32,
        flex_grow: f32,
        width_percent: f32,
        fg_rgb: [u8; 3],
        bold: bool,
    },
    DeleteRegion {
        region: u32,
    },
    Text {
        region: u32,
        text: String,
    },
    RichText {
        region: u32,
        spans: Vec<String>,
    },
    AppendContent {
        region: u32,
        text: String,
    },
    SetRegionMetadata {
        region: u32,
        metadata: Vec<(String, String)>,
    },
    Markdown {
        region: u32,
        source: String,
    },
    Code {
        region: u32,
        source: String,
        language: String,
        line_numbers: bool,
    },
    CreateSurface {
        region: u32,
        rows: u16,
    },
    CloseSurface {
        region: u32,
    },
    SurfaceEvent {
        region: u32,
        kind: String,
    },
    SetSemantics {
        region: u32,
        role: String,
        label: String,
        live: String,
        busy: bool,
    },
    Scroll {
        region: u32,
        x: f32,
        y: f32,
        animate: bool,
    },
    LayoutComputed {
        frame_id: u32,
        trigger: String,
        region: u32,
        width: f32,
        height: f32,
        content_width: f32,
        content_height: f32,
        visible: bool,
    },
    InputKeyDown {
        region: u32,
        char: String,
        modifiers: u8,
    },
    Resize {
        width: u16,
        height: u16,
    },
    Error {
        code: u16,
        message: String,
    },
    Disconnect,
    Ping,
    Pong,
    Reject {
        error: Rejection,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Rejection {
    UnexpectedEof,
    InvalidMagic,
    InvalidUtf8,
    UnknownMessageType,
    InvalidFloat,
    InvalidData,
}

impl Rejection {
    fn matches(&self, error: &ProtocolError) -> bool {
        matches!(
            (self, error),
            (Self::UnexpectedEof, ProtocolError::UnexpectedEof { .. })
                | (Self::InvalidMagic, ProtocolError::InvalidMagic(_))
                | (Self::InvalidUtf8, ProtocolError::InvalidUtf8(_))
                | (
                    Self::UnknownMessageType,
                    ProtocolError::UnknownMessageType(_)
                )
                | (Self::InvalidFloat, ProtocolError::InvalidFloat(_))
                | (Self::InvalidData, ProtocolError::InvalidData(_))
        )
    }
}

fn modifiers_from_bits(bits: u8) -> Modifiers {
    let mut mods = Modifiers::NONE;
    if bits & 0x01 != 0 {
        mods = mods.union(Modifiers::SHIFT);
    }
    if bits & 0x02 != 0 {
        mods = mods.union(Modifiers::CTRL);
    }
    if bits & 0x04 != 0 {
        mods = mods.union(Modifiers::ALT);
    }
    if bits & 0x08 != 0 {
        mods = mods.union(Modifiers::META);
    }
    mods
}

fn assert_frame<M: Decode + Encode + std::fmt::Debug + PartialEq>(
    mut wire: &[u8],
    expected: M,
    name: &str,
) {
    assert_eq!(expected.encode_to_bytes().as_ref(), wire, "{name}");
    assert_eq!(M::decode(&mut wire).unwrap(), expected, "{name}");
    assert!(wire.is_empty(), "{name}: decoder left unread bytes");
}

fn hex(s: &str) -> Vec<u8> {
    assert!(s.is_ascii() && s.len().is_multiple_of(2));
    (0..s.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&s[offset..offset + 2], 16).unwrap())
        .collect()
}

#[test]
fn test_v1_wire_vectors() {
    let vectors: Vec<Vector> =
        serde_json::from_str(include_str!("../conformance/v1.json")).unwrap();
    assert!(vectors.len() >= 32, "expected full v1 conformance corpus");

    for Vector {
        name,
        direction,
        hex: wire,
        expected,
    } in vectors
    {
        let bytes = hex(&wire);
        match (direction, expected) {
            (
                Direction::App,
                Expected::ClientHello {
                    version,
                    requested_capabilities,
                },
            ) => assert_frame(
                &bytes,
                AppMessage::ClientHello {
                    version,
                    requested_capabilities,
                },
                &name,
            ),
            (
                Direction::Terminal,
                Expected::ServerHello {
                    version,
                    capabilities,
                },
            ) => assert_frame(
                &bytes,
                TerminalMessage::ServerHello {
                    version,
                    capabilities,
                },
                &name,
            ),
            (
                Direction::Terminal,
                Expected::SessionReady {
                    session_id,
                    root_region,
                    width,
                    height,
                },
            ) => assert_frame(
                &bytes,
                TerminalMessage::SessionReady {
                    session_id: session_id.parse().unwrap(),
                    root_region: RegionId::new(root_region),
                    width,
                    height,
                },
                &name,
            ),
            (Direction::App, Expected::BeginFrame { frame_id }) => assert_frame(
                &bytes,
                AppMessage::BeginFrame {
                    frame_id: FrameId(frame_id),
                },
                &name,
            ),
            (Direction::App, Expected::EndFrame { frame_id }) => assert_frame(
                &bytes,
                AppMessage::EndFrame {
                    frame_id: FrameId(frame_id),
                },
                &name,
            ),
            (
                Direction::App,
                Expected::CreateRegion {
                    id,
                    parent,
                    insert_index,
                    flex_grow,
                    width_percent,
                    fg_rgb,
                    bold,
                },
            ) => assert_frame(
                &bytes,
                AppMessage::CreateRegion {
                    id: RegionId::new(id),
                    parent: RegionId::new(parent),
                    insert_index,
                    constraints: Constraints {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        flex_grow,
                        width: Dimension::Percent(width_percent),
                        overflow: Overflow {
                            x: OverflowAxis::Hidden,
                            y: OverflowAxis::Scroll,
                        },
                        ..Constraints::default()
                    },
                    style: Style {
                        fg: Some(Color::Rgb(fg_rgb[0], fg_rgb[1], fg_rgb[2])),
                        bg: Some(Color::Named(NamedColor::Background)),
                        border: BorderStyle::rounded(),
                        bold,
                        ..Style::default()
                    },
                },
                &name,
            ),
            (Direction::App, Expected::DeleteRegion { region }) => assert_frame(
                &bytes,
                AppMessage::DeleteRegion {
                    id: RegionId::new(region),
                },
                &name,
            ),
            (Direction::App, Expected::Text { region, text }) => assert_frame(
                &bytes,
                AppMessage::SetContent {
                    id: RegionId::new(region),
                    content: Content::Text(TextContent::new(text)),
                },
                &name,
            ),
            (Direction::App, Expected::RichText { region, spans }) => {
                assert_eq!(spans.len(), 2, "{name}");
                assert_frame(
                    &bytes,
                    AppMessage::SetContent {
                        id: RegionId::new(region),
                        content: Content::RichText(RichText {
                            spans: vec![
                                Span {
                                    text: spans[0].clone(),
                                    style: Style::new().fg(Color::CYAN).bold(),
                                },
                                Span {
                                    text: spans[1].clone(),
                                    style: Style::default(),
                                },
                            ],
                        }),
                    },
                    &name,
                );
            }
            (Direction::App, Expected::AppendContent { region, text }) => assert_frame(
                &bytes,
                AppMessage::AppendContent {
                    id: RegionId::new(region),
                    text,
                },
                &name,
            ),
            (Direction::App, Expected::SetRegionMetadata { region, metadata }) => assert_frame(
                &bytes,
                AppMessage::SetRegionMetadata {
                    id: RegionId::new(region),
                    metadata,
                },
                &name,
            ),
            (Direction::App, Expected::Markdown { region, source }) => assert_frame(
                &bytes,
                AppMessage::SetContent {
                    id: RegionId::new(region),
                    content: Content::Widget(WidgetContent::Markdown(MarkdownWidget { source })),
                },
                &name,
            ),
            (
                Direction::App,
                Expected::Code {
                    region,
                    source,
                    language,
                    line_numbers,
                },
            ) => assert_frame(
                &bytes,
                AppMessage::SetContent {
                    id: RegionId::new(region),
                    content: Content::Widget(WidgetContent::Code(CodeWidget {
                        source,
                        language,
                        line_numbers,
                    })),
                },
                &name,
            ),
            (Direction::App, Expected::CreateSurface { region, rows }) => assert_frame(
                &bytes,
                AppMessage::CreateSurface {
                    id: RegionId::new(region),
                    rows,
                    constraints: Constraints::default(),
                    style: Style::default(),
                },
                &name,
            ),
            (Direction::App, Expected::CloseSurface { region }) => assert_frame(
                &bytes,
                AppMessage::CloseSurface {
                    id: RegionId::new(region),
                },
                &name,
            ),
            (Direction::Terminal, Expected::SurfaceEvent { region, kind }) => {
                let kind = match kind.as_str() {
                    "placed" => SurfaceEventKind::Placed,
                    "closed" => SurfaceEventKind::Closed,
                    "evicted" => SurfaceEventKind::Evicted,
                    "hidden" => SurfaceEventKind::Hidden,
                    "shown" => SurfaceEventKind::Shown,
                    other => panic!("{name}: unknown surface event kind {other}"),
                };
                assert_frame(
                    &bytes,
                    TerminalMessage::SurfaceEvent {
                        id: RegionId::new(region),
                        kind,
                    },
                    &name,
                );
            }
            (
                Direction::App,
                Expected::SetSemantics {
                    region,
                    role,
                    label,
                    live,
                    busy,
                },
            ) => {
                let role = match role.as_str() {
                    "log" => Role::Log,
                    other => panic!("{name}: unhandled role {other}"),
                };
                let live = match live.as_str() {
                    "polite" => Live::Polite,
                    "assertive" => Live::Assertive,
                    "off" => Live::Off,
                    other => panic!("{name}: unhandled live {other}"),
                };
                assert_frame(
                    &bytes,
                    AppMessage::SetSemantics {
                        id: RegionId::new(region),
                        semantics: Semantics {
                            role,
                            label: Some(label),
                            live,
                            busy,
                            ..Semantics::default()
                        },
                    },
                    &name,
                );
            }
            (
                Direction::App,
                Expected::Scroll {
                    region,
                    x,
                    y,
                    animate,
                },
            ) => assert_frame(
                &bytes,
                AppMessage::Scroll {
                    id: RegionId::new(region),
                    x,
                    y,
                    animate,
                },
                &name,
            ),
            (
                Direction::Terminal,
                Expected::LayoutComputed {
                    frame_id,
                    trigger,
                    region,
                    width,
                    height,
                    content_width,
                    content_height,
                    visible,
                },
            ) => {
                let trigger = match trigger.as_str() {
                    "resize" => LayoutTrigger::Resize,
                    "constraint_change" => LayoutTrigger::ConstraintChange,
                    "content_change" => LayoutTrigger::ContentChange,
                    "tree_change" => LayoutTrigger::TreeChange,
                    other => panic!("{name}: unknown layout trigger {other}"),
                };
                assert_frame(
                    &bytes,
                    TerminalMessage::LayoutComputed {
                        frame_id: Some(FrameId(frame_id)),
                        trigger,
                        regions: vec![ComputedRegionLayout {
                            id: RegionId::new(region),
                            x: 0.0,
                            y: 0.0,
                            width,
                            height,
                            content_x: 1.0,
                            content_y: 1.0,
                            content_width,
                            content_height,
                            visible,
                        }],
                    },
                    &name,
                );
            }
            (
                Direction::Terminal,
                Expected::InputKeyDown {
                    region,
                    char: ch,
                    modifiers,
                },
            ) => {
                let c = ch.chars().next().unwrap();
                let mods = modifiers_from_bits(modifiers);
                assert_frame(
                    &bytes,
                    TerminalMessage::Input(InputEvent {
                        region_id: RegionId::new(region),
                        event_type: InputType::KeyDown {
                            key: Key::Char(c),
                            modifiers: mods,
                        },
                        x: 0.0,
                        y: 0.0,
                        modifiers: mods,
                    }),
                    &name,
                );
            }
            (Direction::Terminal, Expected::Resize { width, height }) => assert_frame(
                &bytes,
                TerminalMessage::Resize(ResizeEvent { width, height }),
                &name,
            ),
            (Direction::Terminal, Expected::Error { code, message }) => {
                assert_frame(&bytes, TerminalMessage::Error { code, message }, &name)
            }
            (Direction::App, Expected::Disconnect) => {
                assert_frame(&bytes, AppMessage::Disconnect, &name)
            }
            (Direction::Terminal, Expected::Ping) => {
                assert_frame(&bytes, TerminalMessage::Ping, &name)
            }
            (Direction::App, Expected::Pong) => assert_frame(&bytes, AppMessage::Pong, &name),
            (direction, Expected::Reject { error }) => {
                let mut wire = bytes.as_slice();
                let actual = match direction {
                    Direction::App => AppMessage::decode(&mut wire).unwrap_err(),
                    Direction::Terminal => TerminalMessage::decode(&mut wire).unwrap_err(),
                };
                assert!(error.matches(&actual), "{name}: {actual}");
            }
            _ => panic!("{name}: the message is in the wrong namespace"),
        }
    }
}
