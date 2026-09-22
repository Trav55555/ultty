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
    TerminalMessage, TextAlign, TextContent, TextOverflow, TextWrap, WidgetContent, CAP_ALL_KNOWN,
    CAP_APPEND_CONTENT, CAP_LAYOUT_FLEXBOX, CAP_WIDGET_SCROLL, EVENT_FOCUS, EVENT_KEY, EVENT_MOUSE,
    EVENT_SCROLL, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};

use ultty::protocol::ProtocolServer;
use ultty::terminal_manager::TabId;

const TAB_A: TabId = TabId::new(0);
const TAB_B: TabId = TabId::new(1);

/// Per-test socket directory, removed with its contents on drop.
struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("ultty-test-{}-{}", id, ts)))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Server with one open, active tab (`TAB_A`).
///
/// Returns the server, that tab's socket path, and the directory guard,
/// which the test must keep alive (bind it as `_dir`).
fn start_server() -> (ProtocolServer, PathBuf, TestDir) {
    let dir = TestDir::new();
    let mut server = ProtocolServer::new(&dir.0).unwrap();
    let socket_path = server.open_tab(TAB_A).unwrap();
    server.set_active_tab(Some(TAB_A));
    (server, socket_path, dir)
}

#[test]
fn test_protocol_server_accepts_connections() {
    let (server, socket_path, _dir) = start_server();

    assert_eq!(server.session_count(), 0);
    assert!(socket_path.exists());

    let _client = UnixStream::connect(&socket_path).unwrap();

    thread::sleep(Duration::from_millis(10));

    drop(server);
    assert!(!socket_path.exists());
}

#[test]
fn test_client_hello_handshake() {
    let (mut server, socket_path, _dir) = start_server();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();
    assert_eq!(server.session_count(), 1);

    let hello = AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: CAP_LAYOUT_FLEXBOX,
    };
    let mut buf = BytesMut::new();
    hello.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

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
            assert_eq!(capabilities, CAP_LAYOUT_FLEXBOX);
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
fn test_future_client_version_negotiates_to_current_protocol() {
    let (mut server, socket_path, _dir) = start_server();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let requested = CAP_LAYOUT_FLEXBOX | CAP_WIDGET_SCROLL | CAP_APPEND_CONTENT;
    let hello = AppMessage::ClientHello {
        version: PROTOCOL_VERSION + 1,
        requested_capabilities: requested,
    };
    let mut buf = BytesMut::new();
    hello.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let mut response = vec![0u8; 1024];
    let n = client.read(&mut response).unwrap();
    let mut cursor = &response[..n];

    let msg1 = TerminalMessage::decode(&mut cursor).unwrap();
    match msg1 {
        TerminalMessage::ServerHello {
            version,
            capabilities,
        } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert_eq!(capabilities, requested);
        }
        other => panic!("Expected ServerHello, got {:?}", other),
    }
}

#[test]
fn test_unknown_capability_bits_are_masked_during_handshake() {
    let (mut server, socket_path, _dir) = start_server();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let unknown = 0x8000_0000;
    let requested = CAP_ALL_KNOWN | unknown;
    let hello = AppMessage::ClientHello {
        version: PROTOCOL_VERSION,
        requested_capabilities: requested,
    };
    let mut buf = BytesMut::new();
    hello.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let mut response = vec![0u8; 1024];
    let n = client.read(&mut response).unwrap();
    let mut cursor = &response[..n];

    let msg1 = TerminalMessage::decode(&mut cursor).unwrap();
    match msg1 {
        TerminalMessage::ServerHello { capabilities, .. } => {
            assert_eq!(capabilities, CAP_ALL_KNOWN);
            assert_eq!(capabilities & unknown, 0);
        }
        other => panic!("Expected ServerHello, got {:?}", other),
    }
}

#[test]
fn test_legacy_client_version_gets_error_without_panic() {
    let (mut server, socket_path, _dir) = start_server();

    let mut client = UnixStream::connect(&socket_path).unwrap();
    client.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let too_old = MIN_PROTOCOL_VERSION.saturating_sub(1);
    let hello = AppMessage::ClientHello {
        version: too_old,
        requested_capabilities: CAP_LAYOUT_FLEXBOX,
    };
    let mut buf = BytesMut::new();
    hello.encode(&mut buf);
    client.write_all(&buf).unwrap();

    server.poll(Some(Duration::from_millis(50))).unwrap();

    let mut response = vec![0u8; 1024];
    let n = client.read(&mut response).unwrap();
    let mut cursor = &response[..n];

    let msg = TerminalMessage::decode(&mut cursor).unwrap();
    match msg {
        TerminalMessage::Error { code, .. } => assert_eq!(code, 426),
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_create_region_and_layout() {
    let (mut server, socket_path, _dir) = start_server();
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
        requested_capabilities: CAP_LAYOUT_FLEXBOX,
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
    let (mut server, socket_path, _dir) = start_server();

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
        requested_capabilities: CAP_LAYOUT_FLEXBOX,
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
        Modifiers::NONE,
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();

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
    let (mut server, socket_path, _dir) = start_server();
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
    let (mut server, socket_path, _dir) = start_server();
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

// ========================================================================
// Input routing, clipboard gating, resize relayout, frame recovery
// ========================================================================

/// Read everything the server has queued for `client` (empty if nothing
/// arrives within the read timeout).
fn read_messages(client: &mut UnixStream) -> Vec<TerminalMessage> {
    let mut response = vec![0u8; 65536];
    let n = match client.read(&mut response) {
        Ok(n) => n,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            0
        }
        Err(e) => panic!("read failed: {}", e),
    };
    let mut cursor = &response[..n];
    let mut out = Vec::new();
    while !cursor.is_empty() {
        out.push(TerminalMessage::decode(&mut cursor).expect("valid message"));
    }
    out
}

fn subscribe_root_keys(client: &mut UnixStream, server: &mut ProtocolServer) {
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_KEY,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();
}

fn key_downs(messages: &[TerminalMessage]) -> Vec<Key> {
    messages
        .iter()
        .filter_map(|m| match m {
            TerminalMessage::Input(input) => match &input.event_type {
                InputType::KeyDown { key, .. } => Some(key.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn subscribe_root_mouse(client: &mut UnixStream, server: &mut ProtocolServer) {
    let mut buf = BytesMut::new();
    AppMessage::Subscribe {
        id: RegionId::ROOT,
        events: EVENT_MOUSE,
    }
    .encode(&mut buf);
    client.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();
}

fn short_read_timeout(client: &UnixStream) {
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
}

#[test]
fn test_keys_route_only_to_sessions_of_the_active_tab() {
    let (mut server, path_a, _dir) = start_server();
    let path_b = server.open_tab(TAB_B).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut on_a, _) = handshake(&mut server, &path_a);
    let (mut on_b, _) = handshake(&mut server, &path_b);
    subscribe_root_keys(&mut on_a, &mut server);
    subscribe_root_keys(&mut on_b, &mut server);
    short_read_timeout(&on_a);
    short_read_timeout(&on_b);

    // Tab A is active: only its session sees keystrokes, even though the
    // session on tab B connected more recently.
    assert_eq!(server.active_tab(), Some(TAB_A));
    assert!(server.has_key_subscribers());
    server.broadcast_key(Key::Char('a'), Modifiers::NONE);
    assert_eq!(key_downs(&read_messages(&mut on_a)), vec![Key::Char('a')]);
    assert!(
        key_downs(&read_messages(&mut on_b)).is_empty(),
        "a session in a background tab must not see keystrokes"
    );

    // Switch to tab B: keys, key-ups and text input go there instead.
    server.set_active_tab(Some(TAB_B));
    server.broadcast_key(Key::Char('b'), Modifiers::NONE);
    server.broadcast_key_up(Key::Char('b'), Modifiers::NONE);
    server.broadcast_text_input("txt".to_string());
    let b_msgs = read_messages(&mut on_b);
    assert_eq!(key_downs(&b_msgs), vec![Key::Char('b')]);
    assert!(b_msgs.iter().any(|m| matches!(
        m,
        TerminalMessage::Input(input) if matches!(&input.event_type, InputType::KeyUp { key, .. } if *key == Key::Char('b'))
    )));
    assert!(b_msgs.iter().any(|m| matches!(
        m,
        TerminalMessage::Input(input) if matches!(&input.event_type, InputType::TextInput { text } if text == "txt")
    )));
    assert!(read_messages(&mut on_a).is_empty());

    // Two sessions in the same tab both receive input (the user's choice).
    let (mut second_on_a, _) = handshake(&mut server, &path_a);
    subscribe_root_keys(&mut second_on_a, &mut server);
    short_read_timeout(&second_on_a);
    server.set_active_tab(Some(TAB_A));
    server.broadcast_key(Key::Char('c'), Modifiers::NONE);
    assert_eq!(key_downs(&read_messages(&mut on_a)), vec![Key::Char('c')]);
    assert_eq!(
        key_downs(&read_messages(&mut second_on_a)),
        vec![Key::Char('c')]
    );
    assert!(read_messages(&mut on_b).is_empty());

    // No active tab: nobody receives keys.
    server.set_active_tab(None);
    assert!(!server.has_key_subscribers());
    server.broadcast_key(Key::Char('d'), Modifiers::NONE);
    assert!(read_messages(&mut on_a).is_empty());
    assert!(read_messages(&mut second_on_a).is_empty());
    assert!(read_messages(&mut on_b).is_empty());
}

#[test]
fn test_mouse_routes_only_to_sessions_of_the_active_tab() {
    let (mut server, _path_a, _dir) = start_server();
    let path_b = server.open_tab(TAB_B).unwrap();
    server.set_viewport(80.0, 24.0);

    let (mut on_b, _) = handshake(&mut server, &path_b);
    subscribe_root_mouse(&mut on_b, &mut server);
    short_read_timeout(&on_b);

    let click = || InputType::MouseDown {
        button: MouseButton::Left,
    };
    server.broadcast_mouse(click(), 5.0, 3.0, Modifiers::NONE);
    assert!(
        read_messages(&mut on_b).is_empty(),
        "a click on the visible tab must not reach a hidden tab's app"
    );

    server.set_active_tab(Some(TAB_B));
    server.broadcast_mouse(click(), 5.0, 3.0, Modifiers::NONE);
    let msgs = read_messages(&mut on_b);
    assert!(msgs.iter().any(|m| matches!(
        m,
        TerminalMessage::Input(input) if matches!(input.event_type, InputType::MouseDown { .. })
    )));
}

#[test]
fn test_clipboard_get_is_denied_for_inactive_tab() {
    let (mut server, path_a, _dir) = start_server();
    let path_b = server.open_tab(TAB_B).unwrap();

    let (mut on_a, _) = handshake(&mut server, &path_a);
    let (mut on_b, _) = handshake(&mut server, &path_b);
    short_read_timeout(&on_a);
    short_read_timeout(&on_b);

    let mut buf = BytesMut::new();
    AppMessage::ClipboardGet.encode(&mut buf);
    on_a.write_all(&buf).unwrap();
    on_b.write_all(&buf).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();
    server.poll(Some(Duration::from_millis(10))).unwrap();

    let gets = server.pending_clipboard_gets();
    assert_eq!(gets.len(), 1, "only the active tab's request is honored");
    server.send_clipboard_content(gets[0], Some("secret".to_string()));

    let a_msgs = read_messages(&mut on_a);
    assert!(
        a_msgs.contains(&TerminalMessage::ClipboardContent {
            text: Some("secret".to_string())
        }),
        "active tab's session receives the clipboard: {:?}",
        a_msgs
    );
    let b_msgs = read_messages(&mut on_b);
    assert!(
        b_msgs.contains(&TerminalMessage::ClipboardContent { text: None }),
        "inactive tab's session is answered with an empty clipboard: {:?}",
        b_msgs
    );
    assert!(!b_msgs
        .iter()
        .any(|m| matches!(m, TerminalMessage::ClipboardContent { text: Some(_) })));
}

#[test]
fn test_closing_a_tab_unlinks_its_socket_and_disconnects_its_sessions() {
    let (mut server, path_a, _dir) = start_server();
    let path_b = server.open_tab(TAB_B).unwrap();

    let (mut on_a, _) = handshake(&mut server, &path_a);
    let (mut on_b, _) = handshake(&mut server, &path_b);
    short_read_timeout(&on_a);
    short_read_timeout(&on_b);
    assert_eq!(server.session_count(), 2);

    server.close_tab(TAB_B);

    assert!(!path_b.exists(), "closed tab's socket file is unlinked");
    assert!(
        UnixStream::connect(&path_b).is_err(),
        "nothing listens for the closed tab any more"
    );
    assert_eq!(server.session_count(), 1);
    assert!(server.sessions().all(|s| s.tab_id() == TAB_A));

    // The closed tab's client sees EOF; the other tab's client is untouched.
    let mut buf = [0u8; 16];
    assert_eq!(
        on_b.read(&mut buf).unwrap(),
        0,
        "session on closed tab is hung up"
    );
    assert!(path_a.exists());
    server.set_viewport(80.0, 24.0);
    subscribe_root_keys(&mut on_a, &mut server);
    server.broadcast_key(Key::Char('x'), Modifiers::NONE);
    assert_eq!(key_downs(&read_messages(&mut on_a)), vec![Key::Char('x')]);
}

#[test]
fn test_resize_sends_layout_computed_to_sessions_with_scene() {
    let (mut server, socket_path, _dir) = start_server();
    server.set_viewport(80.0, 24.0);

    let (mut with_scene, root) = handshake(&mut server, &socket_path);
    let (mut empty, _) = handshake(&mut server, &socket_path);
    with_scene
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    empty
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();

    send_frame(
        &mut with_scene,
        &mut server,
        1,
        vec![AppMessage::CreateRegion {
            id: RegionId::new(1),
            parent: root,
            insert_index: -1,
            constraints: Constraints {
                width: Dimension::Percent(100.0),
                height: Dimension::Cells(3.0),
                ..Default::default()
            },
            style: Style::default(),
        }],
    );
    // Drain the frame's LayoutComputed.
    let _ = read_messages(&mut with_scene);

    server.broadcast_resize(120, 40);

    let msgs = read_messages(&mut with_scene);
    let resize_index = msgs
        .iter()
        .position(|m| matches!(m, TerminalMessage::Resize(_)))
        .expect("resize event");
    let layout_index = msgs
        .iter()
        .position(|m| matches!(m, TerminalMessage::LayoutComputed { .. }))
        .expect("layout after resize");
    assert!(
        resize_index < layout_index,
        "Resize precedes LayoutComputed"
    );
    match &msgs[layout_index] {
        TerminalMessage::LayoutComputed {
            frame_id,
            trigger,
            regions,
        } => {
            assert_eq!(*frame_id, None);
            assert_eq!(*trigger, ultty_protocol::LayoutTrigger::Resize);
            let root_layout = regions.iter().find(|r| r.id == root).unwrap();
            assert_eq!((root_layout.width, root_layout.height), (120.0, 40.0));
            let child = regions.iter().find(|r| r.id == RegionId::new(1)).unwrap();
            assert_eq!(child.width, 120.0);
        }
        other => panic!("unexpected {:?}", other),
    }

    let empty_msgs = read_messages(&mut empty);
    assert!(empty_msgs
        .iter()
        .any(|m| matches!(m, TerminalMessage::Resize(_))));
    assert!(
        !empty_msgs
            .iter()
            .any(|m| matches!(m, TerminalMessage::LayoutComputed { .. })),
        "a session without a scene gets no layout"
    );
}

#[test]
fn test_end_frame_mismatch_then_next_frame_renders() {
    let (mut server, socket_path, _dir) = start_server();
    server.set_viewport(80.0, 24.0);

    let (mut client, root) = handshake(&mut server, &socket_path);
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();

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
    let msgs = read_messages(&mut client);
    assert!(msgs
        .iter()
        .any(|m| matches!(m, TerminalMessage::Error { code: 400, .. })));

    send_frame(
        &mut client,
        &mut server,
        3,
        vec![
            AppMessage::CreateRegion {
                id: RegionId::new(1),
                parent: root,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::SetContent {
                id: RegionId::new(1),
                content: Content::Text(TextContent::new("after mismatch")),
            },
        ],
    );
    let msgs = read_messages(&mut client);
    assert!(
        !msgs
            .iter()
            .any(|m| matches!(m, TerminalMessage::Error { .. })),
        "second frame must be clean: {:?}",
        msgs
    );
    assert!(msgs.iter().any(|m| matches!(
        m,
        TerminalMessage::LayoutComputed {
            frame_id: Some(FrameId(3)),
            ..
        }
    )));

    // Content-only follow-up frame is answered too.
    send_frame(
        &mut client,
        &mut server,
        4,
        vec![AppMessage::SetContent {
            id: RegionId::new(1),
            content: Content::Text(TextContent::new("content only")),
        }],
    );
    let msgs = read_messages(&mut client);
    assert!(msgs.iter().any(|m| matches!(
        m,
        TerminalMessage::LayoutComputed {
            frame_id: Some(FrameId(4)),
            trigger: ultty_protocol::LayoutTrigger::ContentChange,
            ..
        }
    )));
}
