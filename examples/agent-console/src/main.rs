//! `agent-console replay <file>|- [--fast | --speed <f>] [--auto-approve <choice>]`
//! `agent-console events <file>|- [--fast | --speed <f>]`
//! `agent-console live <codex|claude> --cli-version <version> [--prompt <text>]`
//!
//! `replay` renders an agent event stream as the console: natively with
//! inline cards inside Ultty, as ANSI text anywhere else (and with
//! `ULTTY_IN_BAND=0` when Ultty is reached in-band). The mode goes to
//! stderr. An approval is answered by `--auto-approve` (a choice's text or
//! number, after a short pause), else by keys when stdin is a terminal,
//! else with its first choice.
//!
//! `events` prints one summary line per event instead, and a total.
//! `live` renders events from a supported agent CLI as they arrive on stdin.

mod live;

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::time::Duration;

use anyhow::Context;
use ulttui::inline::{Console, Mode};

use agent_console::ansi::AnsiScreen;
use agent_console::app::{App, Approver};
use agent_console::event::{Event, EventKind};
use agent_console::keys::{InBandKeys, TerminalKeys};
use agent_console::native::NativeScreen;
use agent_console::reader::EventReader;
use agent_console::replay::{Pace, Replay};
use agent_console::screen::Screen;

const USAGE: &str = "usage: agent-console replay <file>|- [--fast | --speed <f>] [--auto-approve <choice>]\n       agent-console events <file>|- [--fast | --speed <f>]\n       agent-console live <codex|claude> --cli-version <version> [--prompt <text>]";

/// How long an approval answered by `--auto-approve` stays on screen at
/// recorded pace (divided by the speed; none with `--fast`).
const AUTO_APPROVE_DELAY: Duration = Duration::from_millis(800);

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Replay,
    Events,
}

/// What to replay and how.
struct Args {
    command: Command,
    source: String,
    pace: Pace,
    auto_approve: Option<String>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_console=info")
        .with_writer(io::stderr)
        .init();
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == "live") {
        args.next();
        let args = live::parse_args(args)?;
        let mut reader = live::Reader::new(io::stdin().lock(), args);
        render(&mut reader, None)?;
        return reader.finish();
    }
    let args = parse_args(args)?;
    let input: Box<dyn BufRead> = if args.source == "-" {
        Box::new(io::stdin().lock())
    } else {
        let file =
            File::open(&args.source).with_context(|| format!("cannot open {}", args.source))?;
        Box::new(BufReader::new(file))
    };

    let mut reader = EventReader::new(input);
    match args.command {
        Command::Events => events(&mut reader, args.pace)?,
        Command::Replay => render(reader.by_ref().map(|event| Ok(event?)), Some(&args))?,
    }
    if let Some(first) = reader.malformed().first() {
        anyhow::bail!(
            "{} malformed lines, the first at {first}",
            reader.malformed().len()
        );
    }
    Ok(())
}

fn events(reader: &mut EventReader<Box<dyn BufRead>>, pace: Pace) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    let mut count = 0usize;
    for event in Replay::new(reader.by_ref(), pace, std::thread::sleep) {
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
    Ok(())
}

fn render(
    events: impl Iterator<Item = anyhow::Result<Event>>,
    replay_args: Option<&Args>,
) -> anyhow::Result<()> {
    let console = Console::connect();
    eprintln!("agent-console: {:?} mode", console.mode());
    let native = console.mode() == Mode::Native;
    let in_band = native && ultty_client::detect::socket_path().is_none();
    let pace = replay_args.map_or(Pace::Fast, |args| args.pace);
    let follow = replay_args.is_none();
    let can_ask = replay_args.is_some_and(|args| args.source != "-") && io::stdin().is_terminal();
    let approver = match replay_args.and_then(|args| args.auto_approve.as_ref()) {
        Some(choice) => Approver::Auto {
            choice: choice.clone(),
            delay: match pace {
                Pace::Fast => Duration::ZERO,
                Pace::Recorded { speed } => AUTO_APPROVE_DELAY.div_f64(speed),
            },
        },
        None if !follow && in_band => Approver::Ask(Box::new(InBandKeys::default())),
        None if can_ask => Approver::Ask(Box::new(TerminalKeys::default())),
        None => Approver::First,
    };
    if native {
        let app = App::new(NativeScreen::new(console), approver, std::thread::sleep);
        return play(app, events, pace, follow);
    }
    drop(console);
    let stdout = io::stdout();
    let screen = if stdout.is_terminal() {
        AnsiScreen::terminal(stdout.lock())
    } else {
        AnsiScreen::pipe(stdout.lock())
    };
    play(
        App::new(screen, approver, std::thread::sleep),
        events,
        pace,
        follow,
    )
}

/// Show every event at `pace`; the app is idle (and draws its live parts)
/// whenever the replay waits.
fn play<S: Screen>(
    mut app: App<S>,
    events: impl Iterator<Item = anyhow::Result<Event>>,
    pace: Pace,
    follow: bool,
) -> anyhow::Result<()> {
    let wait = Cell::new(None);
    for event in Replay::new(events, pace, |gap| wait.set(Some(gap))) {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                app.finish()?;
                return Err(error);
            }
        };
        if let Some(gap) = wait.take() {
            app.idle(gap)?;
        }
        app.handle(&event)?;
        if follow {
            app.refresh()?;
        }
    }
    app.finish()?;
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> anyhow::Result<Args> {
    let command = match args.next().as_deref() {
        Some("replay") => Command::Replay,
        Some("events") => Command::Events,
        _ => anyhow::bail!(USAGE),
    };
    let mut source = None;
    let mut pace = Pace::recorded(1.0)?;
    let mut auto_approve = None;
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
            "--auto-approve" if command == Command::Replay => {
                auto_approve = Some(args.next().context(USAGE)?);
            }
            _ if source.is_none() && (arg == "-" || !arg.starts_with('-')) => source = Some(arg),
            _ => anyhow::bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Args {
        command,
        source: source.context(USAGE)?,
        pace,
        auto_approve,
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
