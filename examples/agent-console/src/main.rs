//! `agent-console replay <file>|- [--fast] [--speed <f>]`
//!
//! Replays an agent event stream. In this slice it validates the stream and
//! prints one summary line per event; rendering comes later.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use anyhow::Context;

use agent_console::event::{Event, EventKind};
use agent_console::reader::EventReader;
use agent_console::replay::{Pace, Replay};

const USAGE: &str = "usage: agent-console replay <file>|- [--fast] [--speed <f>]";

/// What to replay and how fast.
struct Args {
    source: String,
    pace: Pace,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_console=info")
        .with_writer(io::stderr)
        .init();
    let args = parse_args(std::env::args().skip(1))?;
    let input: Box<dyn BufRead> = if args.source == "-" {
        Box::new(io::stdin().lock())
    } else {
        let file =
            File::open(&args.source).with_context(|| format!("cannot open {}", args.source))?;
        Box::new(BufReader::new(file))
    };

    let mut reader = EventReader::new(input);
    let mut out = io::stdout().lock();
    let mut count = 0usize;
    for event in Replay::new(reader.by_ref(), args.pace, std::thread::sleep) {
        writeln!(out, "{}", summary(&event?))?;
        out.flush()?;
        count += 1;
    }
    writeln!(
        out,
        "{count} events; {} malformed lines skipped; {} events of unknown type skipped",
        reader.malformed().len(),
        reader.unknown().len()
    )?;
    if let Some(first) = reader.malformed().first() {
        anyhow::bail!(
            "{} malformed lines, the first at {first}",
            reader.malformed().len()
        );
    }
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> anyhow::Result<Args> {
    anyhow::ensure!(args.next().as_deref() == Some("replay"), USAGE);
    let mut source = None;
    let mut pace = Pace::recorded(1.0)?;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fast" => pace = Pace::Fast,
            "--speed" => {
                let speed = args.next().context(USAGE)?;
                let speed: f64 = speed
                    .parse()
                    .with_context(|| format!("--speed {speed}: not a number"))?;
                pace = Pace::recorded(speed)?;
            }
            _ if source.is_none() && (arg == "-" || !arg.starts_with('-')) => source = Some(arg),
            _ => anyhow::bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Args {
        source: source.context(USAGE)?,
        pace,
    })
}

/// One line describing `event`, prefixed by its time offset.
fn summary(event: &Event) -> String {
    let detail = match &event.kind {
        EventKind::Prompt { text } => format!("{} chars", text.chars().count()),
        EventKind::TextDelta { turn, text } => {
            format!("turn {turn} +{} chars", text.chars().count())
        }
        EventKind::TurnEnd { turn } => format!("turn {turn}"),
        EventKind::ToolStart { call, tool, title } => format!("{call} {tool}: {title}"),
        EventKind::ToolOutput { call, stream, text } => {
            format!("{call} {stream:?} {} lines", text.lines().count())
        }
        EventKind::ToolEnd {
            call,
            status,
            exit_code,
            duration_ms,
        } => match exit_code {
            Some(code) => format!("{call} {status:?} exit {code} in {duration_ms} ms"),
            None => format!("{call} {status:?} in {duration_ms} ms"),
        },
        EventKind::Diff { path, diff } => format!("{path} {} lines", diff.lines().count()),
        EventKind::Approval {
            request,
            summary,
            choices,
        } => format!("{request} {summary} [{}]", choices.join(" | ")),
        EventKind::Status {
            model,
            input_tokens,
            output_tokens,
            state,
            elapsed_ms,
        } => format!("{model} in {input_tokens} out {output_tokens} {state} at {elapsed_ms} ms"),
    };
    format!(
        "{:>10.3}s {:<11} {detail}",
        event.t as f64 / 1000.0,
        event.kind.type_name()
    )
}
