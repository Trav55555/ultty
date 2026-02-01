//! Integration tests for native protocol.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use bytes::BytesMut;
use ultty_protocol::{
    AppMessage, Constraints, Decode, Dimension, Display, Encode, FrameId, RegionId, Style,
    TerminalMessage, PROTOCOL_VERSION,
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
