//! Open one inline surface (ADR 0020) and hold it, for
//! `scripts/e2e/inline-surface.sh`.
//!
//!     inline-surface [hold-seconds]
//!
//! Writes the surface's anchor and three lines of fallback text to stdout,
//! sends `CreateSurface` with an opaque magenta card over the socket, and
//! keeps the session open (answering pings) for `hold-seconds`, 60 by
//! default. Surface events go to stderr, one per line. Exits 1 outside
//! Ultty or when the terminal does not offer inline surfaces.

use std::io::Write;
use std::time::{Duration, Instant};

use ultty_client::UlttyClient;
use ultty_protocol::{
    AppMessage, Color, Constraints, Content, Dimension, RegionId, Style, TerminalMessage,
    TextContent, CAP_INLINE_SURFACES,
};

/// The surface root's id; well clear of the ids the client hands out.
const SURFACE: RegionId = RegionId::new(9000);

/// The card's height, and the lines of fallback text under it.
const FALLBACK: [&str; 3] = ["FALLBACK 1", "FALLBACK 2", "FALLBACK 3"];

/// The card's background, which the e2e script looks for in screenshots.
const CARD_BG: Color = Color::Rgb(200, 40, 160);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hold = std::env::args()
        .nth(1)
        .map(|arg| arg.parse::<u64>())
        .transpose()?
        .map_or(Duration::from_secs(60), Duration::from_secs);

    let mut client = UlttyClient::connect()?;
    if !client.supports_capability(CAP_INLINE_SURFACES) {
        eprintln!("inline-surface: the terminal does not offer inline surfaces");
        std::process::exit(1);
    }
    let rows = FALLBACK.len() as u16;

    // The anchor, then exactly `rows` lines of fallback text.
    let mut stdout = std::io::stdout().lock();
    write!(
        stdout,
        "\x1b_u{};{};{}\x1b\\",
        client.session_id(),
        SURFACE.0,
        rows
    )?;
    for line in FALLBACK {
        write!(stdout, "{line}\r\n")?;
    }
    stdout.flush()?;
    drop(stdout);

    let frame = client.begin_frame()?;
    client.send(&AppMessage::CreateSurface {
        id: SURFACE,
        rows,
        constraints: Constraints::default(),
        style: Style {
            bg: Some(CARD_BG),
            ..Style::default()
        },
    })?;
    let label = client.create_region(
        SURFACE,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Cells(1.0),
            ..Constraints::default()
        },
        Style {
            fg: Some(Color::Rgb(255, 255, 255)),
            ..Style::default()
        },
    )?;
    client.set_content(label, Content::Text(TextContent::new("INLINE CARD")))?;
    client.end_frame(frame)?;

    let until = Instant::now() + hold;
    while Instant::now() < until {
        for event in client.poll_events()? {
            match event {
                TerminalMessage::SurfaceEvent { id, kind } => {
                    eprintln!("inline-surface: surface {} {:?}", id.0, kind);
                }
                TerminalMessage::Error { code, message } => {
                    eprintln!("inline-surface: error {code}: {message}");
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
