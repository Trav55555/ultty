//! Stream text into a polite live region (ADR 0021 §3), for
//! `scripts/e2e/accessibility-live.sh`.
//!
//!     live-region [lines] [hold-seconds]
//!
//! Shows a full scene of one `Log` region labelled "Reply", marked
//! `Polite`, opens a stream on it and appends `lines` lines (12 by default)
//! a few characters every 20 ms, like a model's tokens, then ends the
//! stream and keeps the session open (answering pings) for
//! `hold-seconds`, 10 by default. Writes "streamed" to stderr when the
//! stream has ended. Exits 1 outside Ultty or when the terminal does not
//! offer semantics.

use std::time::{Duration, Instant};

use ultty_client::UlttyClient;
use ultty_protocol::{
    AppMessage, Constraints, Content, Dimension, Live, Role, Semantics, Style, TerminalMessage,
    TextContent, CAP_SEMANTICS,
};

/// Characters per append.
const CHUNK: usize = 5;

/// Time between appends.
const PACE: Duration = Duration::from_millis(20);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let lines: usize = args
        .next()
        .map(|arg| arg.parse())
        .transpose()?
        .unwrap_or(12);
    let hold = Duration::from_secs(
        args.next()
            .map(|arg| arg.parse())
            .transpose()?
            .unwrap_or(10),
    );

    let mut client = UlttyClient::connect()?;
    if !client.supports_capability(CAP_SEMANTICS) {
        eprintln!("live-region: the terminal does not offer semantics");
        std::process::exit(1);
    }
    let frame = client.begin_frame()?;
    let root = client.root_region();
    let log = client.create_region(
        root,
        Constraints {
            width: Dimension::Percent(100.0),
            height: Dimension::Percent(100.0),
            ..Constraints::default()
        },
        Style::default(),
    )?;
    client.set_content(log, Content::Text(TextContent::new("")))?;
    client.send(&AppMessage::SetSemantics {
        id: log,
        semantics: Semantics {
            role: Role::Log,
            label: Some("Reply".to_owned()),
            live: Live::Polite,
            ..Semantics::default()
        },
    })?;
    client.end_frame(frame)?;
    // Let a screen reader see the empty region before anything streams.
    pump(&mut client, Duration::from_millis(500))?;

    client.send(&AppMessage::StreamStart {
        id: log,
        stream_id: 1,
    })?;
    let text: String = (1..=lines)
        .map(|n| format!("Line {n} of the reply, streamed in small pieces.\n"))
        .collect();
    let chars: Vec<char> = text.chars().collect();
    for piece in chars.chunks(CHUNK) {
        client.append_content(log, piece.iter().collect::<String>())?;
        pump(&mut client, PACE)?;
    }
    client.send(&AppMessage::StreamEnd {
        id: log,
        stream_id: 1,
    })?;
    eprintln!("live-region: streamed");
    pump(&mut client, hold)?;
    Ok(())
}

/// Read and answer the terminal for `time`, reporting errors to stderr.
fn pump(client: &mut UlttyClient, time: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let until = Instant::now() + time;
    loop {
        for event in client.poll_events()? {
            if let TerminalMessage::Error { code, message } = event {
                eprintln!("live-region: error {code}: {message}");
            }
        }
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        std::thread::sleep((until - now).min(Duration::from_millis(10)));
    }
}
