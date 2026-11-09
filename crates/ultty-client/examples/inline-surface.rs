//! Open one inline surface (ADR 0020) and hold it, for
//! `scripts/e2e/inline-surface.sh`.
//!
//!     inline-surface [hold-seconds] [close-when]
//!
//! Writes the surface's anchor and three lines of fallback text to stdout,
//! sends `CreateSurface` with an opaque magenta card over the socket, and
//! keeps the session open (answering pings) for `hold-seconds`, 60 by
//! default. Once a file exists at the `close-when` path, it closes the
//! surface (`CloseSurface`) and exits when the terminal says it is closed.
//! Surface events go to stderr, one per line. Exits 1 outside Ultty or when
//! the terminal does not offer inline surfaces.

use std::time::{Duration, Instant};

use ultty_client::UlttyClient;
use ultty_protocol::{
    AppMessage, Color, Constraints, Content, Dimension, RegionId, Style, SurfaceEventKind,
    TerminalMessage, TextContent, CAP_INLINE_SURFACES,
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
    let close_when = std::env::args().nth(2).map(std::path::PathBuf::from);

    let mut client = UlttyClient::connect()?;
    if !client.supports_capability(CAP_INLINE_SURFACES) {
        eprintln!("inline-surface: the terminal does not offer inline surfaces");
        std::process::exit(1);
    }
    // The anchor and the three lines of fallback text, then one frame with
    // `CreateSurface` and the card's label.
    let label = RegionId::new(SURFACE.0 + 1);
    client.open_surface(
        SURFACE,
        FALLBACK.len() as u16,
        &FALLBACK,
        Constraints::default(),
        Style {
            bg: Some(CARD_BG),
            ..Style::default()
        },
        vec![
            AppMessage::CreateRegion {
                id: label,
                parent: SURFACE,
                insert_index: -1,
                constraints: Constraints {
                    width: Dimension::Percent(100.0),
                    height: Dimension::Cells(1.0),
                    ..Constraints::default()
                },
                style: Style {
                    fg: Some(Color::Rgb(255, 255, 255)),
                    ..Style::default()
                },
            },
            AppMessage::SetContent {
                id: label,
                content: Content::Text(TextContent::new("INLINE CARD")),
            },
        ],
    )?;

    let until = Instant::now() + hold;
    let mut closing = false;
    while Instant::now() < until {
        if !closing && close_when.as_deref().is_some_and(|path| path.exists()) {
            client.close_surface(SURFACE)?;
            closing = true;
        }
        for event in client.poll_events()? {
            match event {
                TerminalMessage::SurfaceEvent { id, kind } => {
                    eprintln!("inline-surface: surface {} {:?}", id.0, kind);
                    if kind == SurfaceEventKind::Closed {
                        return Ok(());
                    }
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
