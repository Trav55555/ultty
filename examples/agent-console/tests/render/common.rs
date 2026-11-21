//! Helpers the rendering tests share: fixtures, the source text they hold,
//! and a transcript run without waiting.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use agent_console::app::{App, Approver};
use agent_console::event::{Event, EventKind};
use agent_console::reader::EventReader;
use agent_console::screen::Screen;

/// The events of fixture `name`.
pub fn load(name: &str) -> Vec<Event> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    EventReader::new(BufReader::new(File::open(path).unwrap()))
        .map(Result::unwrap)
        .collect()
}

/// The events of `events` up to and including the end of turn `turns`.
pub fn first_turns(events: Vec<Event>, turns: u32) -> Vec<Event> {
    let end = events
        .iter()
        .position(|e| matches!(e.kind, EventKind::TurnEnd { turn } if turn == turns))
        .expect("the fixture has that many turns");
    events.into_iter().take(end + 1).collect()
}

/// The prose the agent wrote, in order: each run of text deltas between
/// other events that are not status updates, split into lines.
pub fn prose_segments(events: &[Event]) -> Vec<Vec<String>> {
    let mut segments = Vec::new();
    let mut text = String::new();
    for event in events {
        match &event.kind {
            EventKind::TextDelta { text: delta, .. } => text.push_str(delta),
            EventKind::Status { .. } | EventKind::ToolOutput { .. } => {}
            _ => {
                if !text.is_empty() {
                    segments.push(
                        std::mem::take(&mut text)
                            .split('\n')
                            .map(String::from)
                            .collect(),
                    );
                }
            }
        }
    }
    if !text.is_empty() {
        segments.push(text.split('\n').map(String::from).collect());
    }
    segments
}

/// Every prompt, as the transcript shows its first line.
pub fn prompts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Prompt { text } => Some(format!("> {}", text.lines().next().unwrap())),
            _ => None,
        })
        .collect()
}

/// Every tool call's title line up to its status, in order.
pub fn tool_titles(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolStart { tool, title, .. } => Some(format!("[{tool}] {title} (")),
            _ => None,
        })
        .collect()
}

/// An approver that picks the first choice at once.
pub fn approve_first() -> Approver {
    Approver::Auto {
        choice: "1".into(),
        delay: Duration::ZERO,
    }
}

/// Show `events` on `screen` as a replay at recorded pace would, without
/// waiting: idle before every event, so live parts are drawn throughout.
pub fn run<S: Screen>(screen: S, events: &[Event]) -> S {
    let mut app = App::new(screen, approve_first(), |_| {});
    for event in events {
        app.idle(Duration::ZERO).unwrap();
        app.handle(event).unwrap();
    }
    app.finish().unwrap();
    app.into_screen()
}

/// Assert that each segment appears in `lines` as a contiguous run, in
/// order, and return where each started.
pub fn assert_segments_in_order(lines: &[String], segments: &[Vec<String>]) -> Vec<usize> {
    let mut at = 0;
    let mut starts = Vec::new();
    for segment in segments {
        let found = (at..=lines.len().saturating_sub(segment.len()))
            .find(|&i| lines[i..i + segment.len()] == segment[..]);
        let Some(start) = found else {
            panic!(
                "segment not found in order after line {at}:\n{}\n--- transcript from there:\n{}",
                segment.join("\n"),
                lines[at..].join("\n")
            );
        };
        starts.push(start);
        at = start + segment.len();
    }
    starts
}

/// Assert that each of `wanted` is the start of a line, in order.
pub fn assert_line_starts_in_order(lines: &[String], wanted: &[String]) {
    let mut at = 0;
    for want in wanted {
        let found = lines[at..]
            .iter()
            .position(|l| l.starts_with(want.as_str()));
        let Some(offset) = found else {
            panic!("{want:?} not found after line {at}");
        };
        at += offset + 1;
    }
}

/// Assert that every prose line occurs in `lines` exactly as often as in
/// the source: nothing doubled by a redraw, nothing lost.
pub fn assert_prose_counts(lines: &[String], segments: &[Vec<String>]) {
    let mut wanted: std::collections::BTreeMap<&str, usize> = Default::default();
    for line in segments.iter().flatten().filter(|l| !l.is_empty()) {
        *wanted.entry(line).or_default() += 1;
    }
    for (line, count) in wanted {
        let shown = lines.iter().filter(|l| l.as_str() == line).count();
        assert_eq!(
            shown, count,
            "{line:?} is shown {shown} times, written {count}"
        );
    }
}
