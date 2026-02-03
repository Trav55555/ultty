use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use ultty_protocol::{
    AppMessage, CodeWidget, Color, Constraints, Content, Decode, Dimension, Display, Encode,
    FlexDirection, FrameId, MarkdownWidget, NamedColor, ProgressWidget, RegionId, SpinnerWidget,
    Style, TerminalMessage, TextAlign, TextContent, TextOverflow, TextWrap, WidgetContent,
    FRAME_MAGIC, PROTOCOL_VERSION,
};

struct NativeClient {
    stream: UnixStream,
    read_buf: BytesMut,
    session_id: u64,
    root_region: RegionId,
    viewport_width: u16,
    viewport_height: u16,
    next_region_id: u32,
    frame_id: u32,
}

impl NativeClient {
    fn connect() -> anyhow::Result<Self> {
        let socket_path = std::env::var("ULTTY_SOCKET")
            .map_err(|_| anyhow::anyhow!("ULTTY_SOCKET not set - run inside ultty terminal"))?;

        tracing::info!("Connecting to {}", socket_path);
        let stream = UnixStream::connect(&socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let mut client = Self {
            stream,
            read_buf: BytesMut::with_capacity(4096),
            session_id: 0,
            root_region: RegionId::ROOT,
            viewport_width: 80,
            viewport_height: 24,
            next_region_id: 1,
            frame_id: 0,
        };

        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> anyhow::Result<()> {
        self.send(&AppMessage::ClientHello {
            version: PROTOCOL_VERSION,
            requested_capabilities: 0,
        })?;

        loop {
            self.read_available()?;
            while let Some(msg) = self.try_decode()? {
                match msg {
                    TerminalMessage::ServerHello { version, .. } => {
                        tracing::info!("Server version: {}", version);
                    }
                    TerminalMessage::SessionReady {
                        session_id,
                        root_region,
                        width,
                        height,
                    } => {
                        self.session_id = session_id;
                        self.root_region = root_region;
                        self.viewport_width = width;
                        self.viewport_height = height;
                        tracing::info!(
                            "Session ready: id={}, viewport={}x{}",
                            session_id,
                            width,
                            height
                        );
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }

    fn send(&mut self, msg: &AppMessage) -> anyhow::Result<()> {
        let bytes = msg.encode_to_bytes();
        self.stream.write_all(&bytes)?;
        Ok(())
    }

    fn read_available(&mut self) -> anyhow::Result<()> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => anyhow::bail!("Connection closed"),
            Ok(n) => self.read_buf.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    fn try_decode(&mut self) -> anyhow::Result<Option<TerminalMessage>> {
        if self.read_buf.len() < 8 {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([self.read_buf[0], self.read_buf[1]]);
        if magic != u16::from_le_bytes(FRAME_MAGIC) {
            anyhow::bail!("Invalid magic");
        }

        let payload_len = u32::from_le_bytes([
            self.read_buf[4],
            self.read_buf[5],
            self.read_buf[6],
            self.read_buf[7],
        ]) as usize;

        let total = 8 + payload_len;
        if self.read_buf.len() < total {
            return Ok(None);
        }

        let msg = TerminalMessage::decode(&mut self.read_buf.as_ref())?;
        self.read_buf.advance(total);
        Ok(Some(msg))
    }

    fn alloc_region(&mut self) -> RegionId {
        let id = RegionId::new(self.next_region_id);
        self.next_region_id += 1;
        id
    }

    fn begin_frame(&mut self) -> anyhow::Result<FrameId> {
        self.frame_id += 1;
        let frame_id = FrameId(self.frame_id);
        self.send(&AppMessage::BeginFrame { frame_id })?;
        Ok(frame_id)
    }

    fn end_frame(&mut self, frame_id: FrameId) -> anyhow::Result<()> {
        self.send(&AppMessage::EndFrame { frame_id })?;
        self.stream.flush()?;

        self.stream.set_nonblocking(false)?;
        loop {
            self.read_available()?;
            while let Some(msg) = self.try_decode()? {
                if let TerminalMessage::LayoutComputed { regions, .. } = msg {
                    tracing::info!("Layout computed for {} regions", regions.len());
                    for r in &regions {
                        tracing::debug!(
                            "  Region {:?}: ({}, {}) {}x{}",
                            r.id,
                            r.x,
                            r.y,
                            r.width,
                            r.height
                        );
                    }
                    self.stream.set_nonblocking(true)?;
                    return Ok(());
                }
            }
        }
    }

    fn create_region(
        &mut self,
        parent: RegionId,
        constraints: Constraints,
        style: Style,
    ) -> anyhow::Result<RegionId> {
        let id = self.alloc_region();
        self.send(&AppMessage::CreateRegion {
            id,
            parent,
            insert_index: -1,
            constraints,
            style,
        })?;
        Ok(id)
    }

    fn set_content(&mut self, id: RegionId, content: Content) -> anyhow::Result<()> {
        self.send(&AppMessage::SetContent { id, content })
    }

    fn poll_events(&mut self) -> anyhow::Result<Vec<TerminalMessage>> {
        let mut events = Vec::new();
        let _ = self.read_available();
        while let Some(msg) = self.try_decode()? {
            events.push(msg);
        }
        Ok(events)
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("hello_native=debug,info")
        .init();

    tracing::info!("Hello Native - Ultty Protocol Demo");

    let mut client = NativeClient::connect()?;

    let frame = client.begin_frame()?;

    let container = client.create_region(
        client.root_region,
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
