//! Integration tests for native protocol.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use bytes::BytesMut;
use ultty_protocol::{
    AppMessage, Constraints, Content, Decode, Dimension, Display, Encode, FrameId, InputType, Key,
    Modifiers, ProgressWidget, RegionId, Style, TerminalMessage, TextAlign, TextContent,
    TextOverflow, TextWrap, WidgetContent, EVENT_KEY, PROTOCOL_VERSION,
};

fn temp_socket_path() -> PathBuf {
    let id = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    PathBuf::from(format!("/tmp/ultty-test-{}-{}.sock", id, ts))
}

#[test]
fn test_protocol_server_accepts_connections() {
    let socket_path = temp_socket_path();
    let server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();

    assert_eq!(server.session_count(), 0);
    assert!(socket_path.exists());

    let _client = UnixStream::connect(&socket_path).unwrap();

    thread::sleep(Duration::from_millis(10));

    drop(server);
    assert!(!socket_path.exists());
}

#[test]
fn test_client_hello_handshake() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();
    assert_eq!(server.session_count(), 1);

    let hello = AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: 0x0001,
    };
    let mut buf = BytesMut::new();
    hello.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 1024];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected response from server");

    let mut cursor = &response[..n];
    let msg1 = TerminalMessage::decode(&mut cursor).unwrap();

    match msg1 {
        TerminalMessage::ServerHello {
            version,
            capabilities,
        } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert!(capabilities > 0);
        }
        other => panic!("Expected ServerHello, got {:?}", other),
    }

    let msg2 = TerminalMessage::decode(&mut cursor).unwrap();
    match msg2 {
        TerminalMessage::SessionReady {
            session_id,
            root_region,
            width,
            height,
        } => {
            assert!(session_id > 0);
            assert_eq!(root_region, RegionId::ROOT);
            assert!(width > 0);
            assert!(height > 0);
        }
        other => panic!("Expected SessionReady, got {:?}", other),
    }
}

#[test]
fn test_create_region_and_layout() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut buf = BytesMut::new();

    AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: 0x0001,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    buf.clear();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 1024];
    let _ = client.read(&mut response).unwrap();

    AppMessage::BeginFrame {
        frame_id: FrameId(1),
    }
    .encode(&mut buf);

    AppMessage::SetConstraints {
        id: RegionId::ROOT,
        constraints: Constraints {
            display: Display::Flex,
            flex_direction: ultty_protocol::FlexDirection::Column,
            width: Dimension::Cells(80.0),
            height: Dimension::Cells(24.0),
            ..Default::default()
        },
    }
    .encode(&mut buf);

    AppMessage::CreateRegion {
        id: RegionId::new(1),
        parent: RegionId::ROOT,
        insert_index: -1,
        constraints: Constraints {
            display: Display::Flex,
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(3.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        style: Style::default(),
    }
    .encode(&mut buf);

    AppMessage::CreateRegion {
        id: RegionId::new(2),
        parent: RegionId::ROOT,
        insert_index: -1,
        constraints: Constraints {
            display: Display::Flex,
            flex_grow: 1.0,
            ..Default::default()
        },
        style: Style::default(),
    }
    .encode(&mut buf);

    AppMessage::EndFrame {
        frame_id: FrameId(1),
    }
    .encode(&mut buf);

    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected LayoutComputed response");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();

    match msg {
        TerminalMessage::LayoutComputed {
            frame_id,
            trigger: _,
            regions,
        } => {
            assert_eq!(frame_id, Some(FrameId(1)));
            assert!(regions.len() >= 2, "Expected at least 2 region layouts");

            let header = regions.iter().find(|r| r.id == RegionId::new(1)).unwrap();
            assert!(
                (header.height - 3.0).abs() < 0.01,
                "Header height should be 3 cells"
            );

            let content = regions.iter().find(|r| r.id == RegionId::new(2)).unwrap();
            assert!(
                content.height > header.height,
                "Content should be taller than header"
            );
        }
        other => panic!("Expected LayoutComputed, got {:?}", other),
    }
}

#[test]
fn test_disconnect() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();
    assert_eq!(server.session_count(), 1);

    let mut buf = BytesMut::new();
    AppMessage::Disconnect.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();
    assert_eq!(server.session_count(), 0);
}

/// Helper: connect a client, complete the handshake, and return (client, root_region).
fn handshake(
    server: &mut ultty::protocol::ProtocolServer,
    socket_path: &std::path::Path,
) -> (UnixStream, RegionId) {
    let mut client = UnixStream::connect(socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut buf = BytesMut::new();
    AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: 0x0001,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    let mut cursor = &response[..n];

    // ServerHello
    let _hello = TerminalMessage::decode(&mut cursor).unwrap();
    // SessionReady
    let ready = TerminalMessage::decode(&mut cursor).unwrap();
    let root_region = match ready {
        TerminalMessage::SessionReady { root_region, .. } => root_region,
        other => panic!("Expected SessionReady, got {:?}", other),
    };

    (client, root_region)
}

/// Helper: send a frame with the given messages, read LayoutComputed response.
fn send_frame(
    client: &mut UnixStream,
    server: &mut ultty::protocol::ProtocolServer,
    frame_id: u32,
    messages: Vec<AppMessage>,
) {
    let mut buf = BytesMut::new();
    AppMessage::BeginFrame {
        frame_id: FrameId(frame_id),
    }
    .encode(&mut buf);
    for msg in messages {
        msg.encode(&mut buf);
    }
    AppMessage::EndFrame {
        frame_id: FrameId(frame_id),
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();
}

#[test]
fn test_set_content_round_trip() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Text(TextContent {
        text: "Hello round-trip".to_string(),
        wrap: TextWrap::None,
        align: TextAlign::Left,
        overflow: TextOverflow::Clip,
    });

    send_frame(
        &mut client,
        &mut server,
        1,
        vec![
            AppMessage::CreateRegion {
                id: region_id,
                parent: root,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Flex,
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(3.0),
                    ..Default::default()
                },
                style: Style::default(),
            },
            AppMessage::SetContent {
                id: region_id,
                content: content.clone(),
            },
        ],
    );

    // Drain the LayoutComputed response
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Verify content in the scene tree
    let session = server.sessions().next().expect("should have a session");
    let region = session.scene().get(region_id).expect("region should exist");
    assert_eq!(region.content, content);
}

#[test]
fn test_set_widget_content_round_trip() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Progress(ProgressWidget {
        value: 0.75,
        label: Some("Loading...".to_string()),
        animated: false,
        indeterminate: false,
    }));

    send_frame(
        &mut client,
        &mut server,
        1,
        vec![
            AppMessage::CreateRegion {
                id: region_id,
                parent: root,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Flex,
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(1.0),
                    ..Default::default()
                },
                style: Style::default(),
            },
            AppMessage::SetContent {
                id: region_id,
                content: content.clone(),
            },
        ],
    );

    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    let session = server.sessions().next().expect("should have a session");
    let region = session.scene().get(region_id).expect("region should exist");
    assert_eq!(region.content, content);
}

#[test]
fn test_key_event_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Subscribe ROOT to key events
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();

    // Server processes the subscribe
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast a key event (auto-flushes)
    server.broadcast_key(Key::Char('a'), Modifiers::NONE);

    // Client reads the input event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected input event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Input(input) => {
            assert_eq!(input.region_id, RegionId::ROOT);
            match input.event_type {
                InputType::KeyDown { key, modifiers } => {
                    assert_eq!(key, Key::Char('a'));
                    assert_eq!(modifiers, Modifiers::NONE);
                }
                other => panic!("Expected KeyDown, got {:?}", other),
            }
        }
        other => panic!("Expected Input, got {:?}", other),
    }
}

#[test]
fn test_key_up_event_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Subscribe ROOT to key events
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();

    // Server processes the subscribe
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast a key-up event (auto-flushes)
    server.broadcast_key_up(Key::Char('z'), Modifiers::SHIFT);

    // Client reads the input event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected input event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Input(input) => {
            assert_eq!(input.region_id, RegionId::ROOT);
            match input.event_type {
                InputType::KeyUp { key, modifiers } => {
                    assert_eq!(key, Key::Char('z'));
                    assert_eq!(modifiers, Modifiers::SHIFT);
                }
                other => panic!("Expected KeyUp, got {:?}", other),
            }
        }
        other => panic!("Expected Input, got {:?}", other),
    }
}

#[test]
fn test_resize_event_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Broadcast a resize (auto-flushes)
    server.broadcast_resize(120, 40);

    // Client reads the resize event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected resize event from server");

    // There may be a LayoutComputed message before the Resize, so decode all
    let mut cursor = &response[..n];
    let mut found_resize = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Resize(resize)) => {
                assert_eq!(resize.width, 120);
                assert_eq!(resize.height, 40);
                found_resize = true;
            }
            Ok(_) => {} // skip other messages (e.g. LayoutComputed)
            Err(_) => break,
        }
    }
    assert!(found_resize, "Expected Resize event");
}

#[test]
fn test_error_for_unknown_region() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    let unknown_id = RegionId::new(9999);

    // Send SetContent for a non-existent region inside a frame
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::SetContent {
            id: unknown_id,
            content: Content::Text(TextContent {
                text: "ghost".to_string(),
                wrap: TextWrap::None,
                align: TextAlign::Left,
                overflow: TextOverflow::Clip,
            }),
        }],
    );

    // Read response — expect an Error message
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected response from server");

    let mut cursor = &response[..n];
    let mut found_error = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Error { code, message }) => {
                assert_eq!(code, 404);
                assert!(
                    message.contains("9999"),
                    "Error message should reference the region id"
                );
                found_error = true;
            }
            Ok(_) => {} // skip other messages
            Err(_) => break,
        }
    }
    assert!(found_error, "Expected Error message for unknown region");
}
