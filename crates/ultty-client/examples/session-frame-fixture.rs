//! Controlled native frames for the headless attachment smoke gate.

use std::time::{Duration, Instant};

use ultty_client::UlttyClient;
use ultty_protocol::{AppMessage, Constraints, Content, RegionId, Style, TextContent};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let release = std::env::args().nth(1).ok_or("missing release path")?;
    let mut client = UlttyClient::connect()?;
    let region = RegionId::new(42);
    let frame = client.begin_frame()?;
    client.send(&AppMessage::CreateRegion {
        id: region,
        parent: RegionId::ROOT,
        insert_index: -1,
        constraints: Constraints::default(),
        style: Style::default(),
    })?;
    client.send(&AppMessage::SetContent {
        id: region,
        content: Content::Text(TextContent::new("FRAME_COMMITTED")),
    })?;
    client.end_frame(frame)?;
    let frame = client.begin_frame()?;
    client.send(&AppMessage::SetContent {
        id: region,
        content: Content::Text(TextContent::new("FRAME_PENDING")),
    })?;
    std::fs::write(format!("{release}.ready"), b"ready")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !std::path::Path::new(&release).exists() {
        let _ = client.poll_events()?;
        std::thread::sleep(Duration::from_millis(20));
    }
    client.end_frame(frame)?;
    while Instant::now() < deadline {
        let _ = client.poll_events()?;
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
