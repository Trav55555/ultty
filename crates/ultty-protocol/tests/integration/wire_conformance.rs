//! Protocol v1 conformance against wire vectors maintained outside the codec.

use serde::Deserialize;

use ultty_protocol::{
    AppMessage, CodeWidget, Content, Decode, Encode, MarkdownWidget, ProtocolError, RegionId,
    TerminalMessage, TextContent, WidgetContent,
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
    Text {
        region: u32,
        text: String,
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
}

impl Rejection {
    fn matches(&self, error: &ProtocolError) -> bool {
        matches!(
            (self, error),
            (Self::UnexpectedEof, ProtocolError::UnexpectedEof { .. })
                | (Self::InvalidMagic, ProtocolError::InvalidMagic(_))
                | (Self::InvalidUtf8, ProtocolError::InvalidUtf8(_))
        )
    }
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
            ) => {
                assert_frame(
                    &bytes,
                    AppMessage::ClientHello {
                        version,
                        requested_capabilities,
                    },
                    &name,
                );
            }
            (
                Direction::Terminal,
                Expected::ServerHello {
                    version,
                    capabilities,
                },
            ) => {
                assert_frame(
                    &bytes,
                    TerminalMessage::ServerHello {
                        version,
                        capabilities,
                    },
                    &name,
                );
            }
            (
                Direction::Terminal,
                Expected::SessionReady {
                    session_id,
                    root_region,
                    width,
                    height,
                },
            ) => {
                assert_frame(
                    &bytes,
                    TerminalMessage::SessionReady {
                        session_id: session_id.parse().unwrap(),
                        root_region: RegionId::new(root_region),
                        width,
                        height,
                    },
                    &name,
                );
            }
            (Direction::App, Expected::Text { region, text }) => assert_frame(
                &bytes,
                AppMessage::SetContent {
                    id: RegionId::new(region),
                    content: Content::Text(TextContent::new(text)),
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
