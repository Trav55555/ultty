//! An agent transcript with inline cards (ADR 0020, plan item B3).
//!
//!     agent-transcript [--hold <path>]
//!
//! Prints a short, made-up agent session the way an agent CLI would: prose,
//! a tool-call card that streams `cargo`-like output into itself, more
//! prose, a diff card, and a closing line. In Ultty the cards are inline
//! surfaces drawn natively in the scrollback; anywhere else (tmux, another
//! terminal, a pipe) they are printed as ANSI text. The output is the same
//! on every run.
//!
//! With `--hold <path>` the cards stay open (live) after the transcript
//! until a file exists at `path`, so a test can print shell output under
//! them first. What the terminal reports about the cards goes to stderr,
//! one line each.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ulttui::inline::{Card, Console, Mode, SurfaceEventKind};
use ulttui::prelude::*;

/// The tool card: a title line and the last lines of output.
const TOOL_ROWS: u16 = 8;
const TOOL_BG: Color = Color::Rgb(24, 52, 96);

/// The diff card: a title line and the hunk.
const DIFF_ROWS: u16 = 6;
const DIFF_BG: Color = Color::Rgb(28, 72, 40);

/// What the fake `cargo test` prints, one line per tick.
const CARGO_OUTPUT: [&str; 10] = [
    "   Compiling ultty-vt v0.1.0",
    "   Compiling ultty-protocol v0.1.0",
    "   Compiling ultty-client v0.1.0",
    "   Compiling ulttui v0.1.0",
    "    Finished test profile in 3.21s",
    "     Running unittests src/lib.rs",
    "running 42 tests",
    "test parse::tabs_expand ... ok",
    "test parse::wide_chars ... ok",
    "test result: ok. 42 passed; 0 failed",
];

/// How long each line of fake output takes.
const TICK: Duration = Duration::from_millis(120);

fn main() -> anyhow::Result<()> {
    let hold = parse_args()?;
    let mut console = Console::connect();
    eprintln!("agent-transcript: {:?} mode", console.mode());

    console.println("> Make the parser tests pass.")?;
    console.println("")?;
    console.println("I'll build the workspace and run the tests first.")?;

    // The card opens queued; its fallback text says so. Natively the rows
    // end up holding what the card last showed instead.
    let tool = console.open(TOOL_ROWS, &tool_card("queued", &[]))?;
    for shown in 1..=CARGO_OUTPUT.len() {
        std::thread::sleep(TICK);
        console.update(&tool, &tool_card("running", &CARGO_OUTPUT[..shown]))?;
        report(&mut console)?;
    }
    console.update(&tool, &tool_card("passed in 3.2s", &CARGO_OUTPUT))?;

    console.println("All 42 tests pass. One assertion was off by a tab stop;")?;
    console.println("here is the fix in src/parse.rs:")?;
    let diff = console.open(DIFF_ROWS, &diff_card())?;
    console.println("Applied. The suite is green.")?;
    console.println("End of transcript.")?;

    if let Some(path) = hold {
        while !path.exists() {
            report(&mut console)?;
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    close(&mut console, [tool, diff])
}

fn parse_args() -> anyhow::Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (None, _) => Ok(None),
        (Some("--hold"), Some(path)) => Ok(Some(PathBuf::from(path))),
        _ => anyhow::bail!("usage: agent-transcript [--hold <path>]"),
    }
}

fn tool_card(status: &str, output: &[&str]) -> Column {
    let tail = output.len().saturating_sub(usize::from(TOOL_ROWS) - 1);
    let mut card = Column::new().bg(TOOL_BG).child(Text::new(format!(
        "[tool] cargo test --workspace ({status})"
    )));
    for line in &output[tail..] {
        card = card.child(Text::new(*line));
    }
    card
}

fn diff_card() -> Column {
    Column::new()
        .bg(DIFF_BG)
        .child(Text::new("[diff] src/parse.rs"))
        .child(DiffView::new(vec![
            DiffLine::new(DiffKind::Context, "fn expand_tabs(line: &str) -> String {").old_line(41),
            DiffLine::new(DiffKind::Removed, "    let stop = 4;").old_line(42),
            DiffLine::new(DiffKind::Added, "    let stop = 8;").new_line(42),
            DiffLine::new(DiffKind::Context, "    let mut out = String::new();").old_line(43),
            DiffLine::new(DiffKind::Context, "    // ...").old_line(44),
        ]))
}

/// Print what the terminal said about the cards to stderr.
fn report(console: &mut Console) -> anyhow::Result<()> {
    for event in console.poll_events()? {
        eprintln!("agent-transcript: card {} {:?}", event.card.0, event.kind);
    }
    Ok(())
}

/// Close the cards and, natively, wait (up to 2 s) until the terminal has
/// frozen them, reporting as it goes.
fn close(console: &mut Console, cards: [Card; 2]) -> anyhow::Result<()> {
    for card in cards {
        console.close(card)?;
    }
    let until = Instant::now() + Duration::from_secs(2);
    let mut closed = 0;
    while console.mode() == Mode::Native && closed < 2 && Instant::now() < until {
        for event in console.poll_events()? {
            eprintln!("agent-transcript: card {} {:?}", event.card.0, event.kind);
            if event.kind == SurfaceEventKind::Closed {
                closed += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
