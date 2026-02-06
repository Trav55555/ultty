//! Integration tests for native protocol.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use bytes::BytesMut;
use ultty_protocol::{
    AppMessage, BorderStyle, BorderType, CodeWidget, Constraints, Content, Decode, Dimension,
    Display, Encode, FlexDirection, FrameId, InputType, InputWidget, Key, MarkdownWidget,
    Modifiers, MouseButton, ProgressWidget, RegionId, RichText, Span, SpinnerWidget, Style,
    TerminalMessage, TextAlign, TextContent, TextOverflow, TextWrap, WidgetContent, EVENT_FOCUS,
    EVENT_KEY, EVENT_MOUSE, EVENT_SCROLL, PROTOCOL_VERSION,
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

#[test]
fn test_error_for_end_frame_mismatch() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    let mut buf = BytesMut::new();
    AppMessage::BeginFrame {
        frame_id: FrameId(1),
    }
    .encode(&mut buf);
    AppMessage::EndFrame {
        frame_id: FrameId(2),
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected response from server");

    let mut cursor = &response[..n];
    let mut found_error = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Error { code, message }) => {
                assert_eq!(code, 400);
                assert!(
                    message.contains("end_frame mismatch"),
                    "Error should mention mismatch"
                );
                found_error = true;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        found_error,
        "Expected Error message for mismatched EndFrame"
    );
}

#[test]
fn test_error_for_decode_failure() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Build a valid message frame then corrupt type code to force decode failure.
    let mut bad = BytesMut::new();
    AppMessage::Pong.encode(&mut bad);
    bad[2] = 0xff;
    bad[3] = 0xff;
    client.write_all(&bad).unwrap();

    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected response from server");

    let mut cursor = &response[..n];
    let mut found_error = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Error { code, message }) => {
                assert_eq!(code, 400);
                assert!(
                    message.contains("decode error"),
                    "Error should mention decode failure"
                );
                found_error = true;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(found_error, "Expected Error message for decode failure");
}

#[test]
fn test_focus_event_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create a child region
    let child_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: child_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(3.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Subscribe child to focus events
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: child_id,
        events: EVENT_FOCUS,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Focus the child region
    let mut buf = BytesMut::new();
    AppMessage::Focus { id: child_id }.encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Client should receive a FocusEvent
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected focus event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Focus(focus) => {
            assert_eq!(focus.region_id, child_id);
            assert!(focus.focused, "Expected focused=true");
        }
        other => panic!("Expected Focus, got {:?}", other),
    }

    // Now blur — send Blur message
    let mut buf = BytesMut::new();
    AppMessage::Blur.encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected blur event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Focus(focus) => {
            assert_eq!(focus.region_id, child_id);
            assert!(!focus.focused, "Expected focused=false");
        }
        other => panic!("Expected Focus(blur), got {:?}", other),
    }
}

#[test]
fn test_focus_key_routing() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create a child region
    let child_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: child_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(3.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Subscribe the child to key events (NOT root)
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: child_id,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    // Focus the child
    AppMessage::Focus { id: child_id }.encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast a key — should be delivered to child_id (focused + subscribed)
    server.broadcast_key(Key::Char('x'), Modifiers::NONE);

    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected input event");

    let mut cursor = &response[..n];
    // Skip any focus events
    let mut found_key = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Input(input)) => {
                assert_eq!(
                    input.region_id, child_id,
                    "Key should be routed to focused region"
                );
                match input.event_type {
                    InputType::KeyDown { key, .. } => assert_eq!(key, Key::Char('x')),
                    other => panic!("Expected KeyDown, got {:?}", other),
                }
                found_key = true;
            }
            Ok(_) => {} // skip Focus events etc
            Err(_) => break,
        }
    }
    assert!(found_key, "Expected key event routed to focused region");
}

#[test]
fn test_scroll_immediate() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create a region
    let region_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: region_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(10.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Send immediate scroll
    let mut buf = BytesMut::new();
    AppMessage::Scroll {
        id: region_id,
        x: 0.0,
        y: 50.0,
        animate: false,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Verify scroll offset is set immediately
    let session = server.sessions().next().unwrap();
    let (sx, sy) = session.scroll_offset(region_id);
    assert_eq!(sx, 0.0);
    assert_eq!(sy, 50.0);
}

#[test]
fn test_scroll_animated_converges() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create a region
    let region_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: region_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(10.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Send animated scroll
    let mut buf = BytesMut::new();
    AppMessage::Scroll {
        id: region_id,
        x: 0.0,
        y: 100.0,
        animate: true,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Initially should NOT be at target yet
    let session = server.sessions().next().unwrap();
    let (_, sy) = session.scroll_offset(region_id);
    assert!(sy < 100.0, "Should not immediately reach target");

    // Run animation frames until convergence
    let mut iterations = 0;
    loop {
        let still_animating = server.update_animations();
        iterations += 1;
        if !still_animating || iterations > 200 {
            break;
        }
    }

    let session = server.sessions().next().unwrap();
    let (sx, sy) = session.scroll_offset(region_id);
    assert_eq!(sx, 0.0);
    assert_eq!(sy, 100.0);
    assert!(
        iterations > 1,
        "Should take multiple iterations to converge"
    );
}

#[test]
fn test_reparent_region() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create two container regions and a child
    let container_a = RegionId::new(1);
    let container_b = RegionId::new(2);
    let child = RegionId::new(3);

    send_frame(
        &mut client,
        &mut server,
        1,
        vec![
            AppMessage::CreateRegion {
                id: container_a,
                parent: root,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    width: Dimension::Percent(50.0),
                    height: Dimension::Percent(100.0),
                    ..Default::default()
                },
                style: Style::default(),
            },
            AppMessage::CreateRegion {
                id: container_b,
                parent: root,
                insert_index: -1,
                constraints: Constraints {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    width: Dimension::Percent(50.0),
                    height: Dimension::Percent(100.0),
                    ..Default::default()
                },
                style: Style::default(),
            },
            AppMessage::CreateRegion {
                id: child,
                parent: container_a,
                insert_index: -1,
                constraints: Constraints {
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(3.0),
                    ..Default::default()
                },
                style: Style::default(),
            },
        ],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Verify child is under container_a
    let session = server.sessions().next().unwrap();
    let a = session.scene().get(container_a).unwrap();
    assert!(a.children.contains(&child));
    let b = session.scene().get(container_b).unwrap();
    assert!(!b.children.contains(&child));

    // Reparent child from container_a to container_b
    send_frame(
        &mut client,
        &mut server,
        2,
        vec![AppMessage::ReparentRegion {
            id: child,
            new_parent: container_b,
            insert_index: -1,
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Verify child moved to container_b
    let session = server.sessions().next().unwrap();
    let a = session.scene().get(container_a).unwrap();
    assert!(
        !a.children.contains(&child),
        "Child should no longer be under container_a"
    );
    let b = session.scene().get(container_b).unwrap();
    assert!(
        b.children.contains(&child),
        "Child should now be under container_b"
    );
    let c = session.scene().get(child).unwrap();
    assert_eq!(c.parent, Some(container_b));
}

#[test]
fn test_widget_content_snapshot_progress() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Progress(ProgressWidget {
        value: 0.5,
        label: Some("Halfway there".to_string()),
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

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("progress_widget", &region.content);
}

#[test]
fn test_widget_content_snapshot_spinner() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Spinner(SpinnerWidget {
        label: Some("Loading...".to_string()),
        ..Default::default()
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

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("spinner_widget", &region.content);
}

#[test]
fn test_widget_content_snapshot_markdown() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Markdown(MarkdownWidget {
        source: "# Title\n\n**Bold** and *italic*\n\n- item 1\n- item 2".to_string(),
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
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(6.0),
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

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("markdown_widget", &region.content);
}

#[test]
fn test_widget_content_snapshot_code() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Code(CodeWidget {
        source: "fn main() {\n    println!(\"hello\");\n}".to_string(),
        language: "rust".to_string(),
        line_numbers: true,
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
                    width: Dimension::Percent(100.0),
                    flex_grow: 1.0,
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

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("code_widget", &region.content);
}

// ========================================================================
// Step 1: Integration tests for existing-but-untested protocol paths
// ========================================================================

#[test]
fn test_mouse_event_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Subscribe ROOT to mouse events
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_MOUSE,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast a mouse event
    server.broadcast_mouse(
        InputType::MouseDown {
            button: MouseButton::Left,
        },
        5.0,
        3.0,
    );

    // Client reads the input event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected mouse input event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Input(input) => {
            match input.event_type {
                InputType::MouseDown { button } => {
                    assert_eq!(button, MouseButton::Left);
                }
                other => panic!("Expected MouseDown, got {:?}", other),
            }
            // Hit-test should return ROOT with local coordinates
            assert_eq!(input.region_id, RegionId::ROOT);
            assert_eq!(input.x, 5.0);
            assert_eq!(input.y, 3.0);
        }
        other => panic!("Expected Input, got {:?}", other),
    }
}

#[test]
fn test_unsubscribe_stops_events() {
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
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Verify key events arrive
    server.broadcast_key(Key::Char('a'), Modifiers::NONE);
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected key event before unsubscribe");

    // Unsubscribe from key events
    let mut buf = BytesMut::new();
    AppMessage::Unsubscribe {
        id: RegionId::ROOT,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast another key — should NOT arrive
    server.broadcast_key(Key::Char('b'), Modifiers::NONE);

    // Set short read timeout and expect no data
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let mut response = vec![0u8; 4096];
    match client.read(&mut response) {
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Expected: no data after unsubscribe
        }
        Ok(0) => {
            // Also acceptable: no data
        }
        Ok(n) => panic!("Expected no data after unsubscribe, got {} bytes", n),
        Err(e) => panic!("Unexpected error: {}", e),
    }
}

#[test]
fn test_rich_text_round_trip() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::RichText(RichText {
        spans: vec![
            Span {
                text: "Hello ".to_string(),
                style: Style::default(),
            },
            Span {
                text: "bold".to_string(),
                style: Style {
                    bold: true,
                    ..Default::default()
                },
            },
            Span {
                text: " world".to_string(),
                style: Style::default(),
            },
        ],
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

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    let session = server.sessions().next().expect("should have a session");
    let region = session.scene().get(region_id).expect("region should exist");
    assert_eq!(region.content, content);
}

#[test]
fn test_input_widget_round_trip() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Input(InputWidget {
        value: "hello".to_string(),
        placeholder: "Type here...".to_string(),
        multiline: false,
        password: false,
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
                    height: Dimension::Cells(2.0),
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

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    let session = server.sessions().next().expect("should have a session");
    let region = session.scene().get(region_id).expect("region should exist");
    assert_eq!(region.content, content);
}

#[test]
fn test_rich_text_snapshot() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::RichText(RichText {
        spans: vec![
            Span {
                text: "Normal ".to_string(),
                style: Style::default(),
            },
            Span {
                text: "bold".to_string(),
                style: Style {
                    bold: true,
                    ..Default::default()
                },
            },
            Span {
                text: " italic".to_string(),
                style: Style {
                    italic: true,
                    ..Default::default()
                },
            },
        ],
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
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(2.0),
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

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("rich_text_widget", &region.content);
}

#[test]
fn test_input_widget_snapshot() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    let content = Content::Widget(WidgetContent::Input(InputWidget {
        value: "user input".to_string(),
        placeholder: "Enter text...".to_string(),
        multiline: true,
        password: false,
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

    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    insta::assert_debug_snapshot!("input_widget", &region.content);
}

// ========================================================================
// Step 2: TextInput event forwarding test
// ========================================================================

#[test]
fn test_text_input_delivery() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Subscribe ROOT to key events (TextInput is keyboard-adjacent)
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Broadcast a text input event
    server.broadcast_text_input("hello".to_string());

    // Client reads the input event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected text input event from server");

    let mut cursor = &response[..n];
    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Input(input) => {
            assert_eq!(input.region_id, RegionId::ROOT);
            match input.event_type {
                InputType::TextInput { text } => {
                    assert_eq!(text, "hello");
                }
                other => panic!("Expected TextInput, got {:?}", other),
            }
        }
        other => panic!("Expected Input, got {:?}", other),
    }
}

// ========================================================================
// Step 3: ScrollEvent delivery test
// ========================================================================

#[test]
fn test_scroll_event_notifies_subscriber() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    // Create a region
    let region_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: region_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(10.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Subscribe the region to scroll events
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: region_id,
        events: EVENT_SCROLL,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Send an immediate scroll
    let mut buf = BytesMut::new();
    AppMessage::Scroll {
        id: region_id,
        x: 10.0,
        y: 20.0,
        animate: false,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Client should receive a Scroll event
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected scroll event from server");

    let mut cursor = &response[..n];
    let mut found_scroll = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::Scroll(scroll)) => {
                assert_eq!(scroll.region_id, region_id);
                assert_eq!(scroll.scroll_x, 10.0);
                assert_eq!(scroll.scroll_y, 20.0);
                found_scroll = true;
            }
            Ok(_) => {} // skip other messages
            Err(_) => break,
        }
    }
    assert!(found_scroll, "Expected Scroll event notification");
}

#[test]
fn test_clipboard_set_and_get() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();

    // Handshake
    let (mut client, _root_id) = handshake(&mut server, &socket_path);

    // --- Test ClipboardSet ---
    // Send ClipboardSet from client
    let mut buf = BytesMut::new();
    AppMessage::ClipboardSet {
        text: "hello clipboard".to_string(),
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Server should have a pending clipboard set
    let sets = server.pending_clipboard_sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].1, "hello clipboard");

    // --- Test ClipboardGet ---
    // Send ClipboardGet from client
    let mut buf = BytesMut::new();
    AppMessage::ClipboardGet.encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    // Server should have a pending clipboard get
    let gets = server.pending_clipboard_gets();
    assert_eq!(gets.len(), 1);
    let session_key = gets[0];

    // Simulate the app layer sending clipboard content back
    server.send_clipboard_content(session_key, Some("pasted text".to_string()));

    // Client should receive ClipboardContent
    thread::sleep(Duration::from_millis(10));
    let mut response = vec![0u8; 4096];
    let n = client.read(&mut response).unwrap();
    assert!(n > 0, "Expected clipboard content response");

    let mut cursor = &response[..n];
    let mut found_clipboard = false;
    while !cursor.is_empty() {
        match TerminalMessage::decode(&mut cursor) {
            Ok(TerminalMessage::ClipboardContent { text }) => {
                assert_eq!(text, Some("pasted text".to_string()));
                found_clipboard = true;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(found_clipboard, "Expected ClipboardContent message");
}

// ========================================================================
// Border rendering integration test
// ========================================================================

#[test]
fn test_border_style_rendered() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);

    let region_id = RegionId::new(1);
    send_frame(
        &mut client,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: region_id,
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                display: Display::Flex,
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(5.0),
                ..Default::default()
            },
            style: Style {
                border: BorderStyle::thin(),
                ..Default::default()
            },
        }],
    );

    // Drain LayoutComputed
    let mut response = vec![0u8; 4096];
    let _ = client.read(&mut response);

    // Verify the region has a border style set in the scene
    let session = server.sessions().next().unwrap();
    let region = session.scene().get(region_id).unwrap();
    assert_eq!(region.style.border.style, BorderType::Thin);
    assert!(region.style.border.edges.top);
    assert!(region.style.border.edges.bottom);
    assert!(region.style.border.edges.left);
    assert!(region.style.border.edges.right);
}

// ========================================================================
// Protocol resilience: Ping/Pong keepalive integration test
// ========================================================================

#[test]
fn test_keepalive_ping_pong() {
    let socket_path = temp_socket_path();
    let mut server = ultty::protocol::ProtocolServer::new(&socket_path).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut client, _root) = handshake(&mut server, &socket_path);

    // Server sends a Ping to the client via check_keepalives
    // (which normally triggers after 30s idle, but we can test the Pong path)

    // Manually send a Pong from the client (simulating a response to a Ping)
    let mut buf = BytesMut::new();
    AppMessage::Pong.encode(&mut buf);
    client.write_all(&buf).unwrap();

    // Server should process the Pong without error and session should remain active
    server.poll(Some(Duration::from_millis(10))).unwrap();
    assert_eq!(
        server.session_count(),
        1,
        "Session should remain active after Pong"
    );

    // Verify Ping can be encoded and decoded (roundtrip via the server sending Ping)
    // We test this by encoding a Ping and verifying the client can decode it
    let mut ping_buf = BytesMut::new();
    TerminalMessage::Ping.encode(&mut ping_buf);

    // Simulate writing ping to client and decoding
    let mut cursor = &ping_buf[..];
    let decoded = TerminalMessage::decode(&mut cursor).unwrap();
    assert!(
        matches!(decoded, TerminalMessage::Ping),
        "Expected Ping message"
    );
}
