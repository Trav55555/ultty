//! Checks the committed fixtures: they match the generator, parse cleanly,
//! form consistent sessions, and hold the text the copy-fidelity and resize
//! measurements need.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use agent_console::event::{Event, EventKind};
use agent_console::generate::{
    to_jsonl, FIXTURES, LONG_OUTPUT_LINES, TWO_HOURS_MS, TWO_HOURS_TURNS,
};
use agent_console::reader::EventReader;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// The fixture's events, asserting that no line was skipped.
fn load(name: &str) -> Vec<Event> {
    let file = File::open(fixture_path(name)).unwrap();
    let mut reader = EventReader::new(BufReader::new(file));
    let events: Vec<Event> = reader.by_ref().map(Result::unwrap).collect();
    assert!(
        reader.malformed().is_empty(),
        "{name}: {:?}",
        reader.malformed()
    );
    assert!(
        reader.unknown().is_empty(),
        "{name}: {:?}",
        reader.unknown()
    );
    events
}

fn counts(events: &[Event]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for event in events {
        *counts.entry(event.kind.type_name()).or_default() += 1;
    }
    counts
}

fn text_fields(kind: &EventKind) -> Vec<&str> {
    match kind {
        EventKind::Prompt { text }
        | EventKind::TextDelta { text, .. }
        | EventKind::ToolOutput { text, .. } => vec![text],
        EventKind::TurnEnd { .. } | EventKind::ToolEnd { .. } => vec![],
        EventKind::ToolStart { call, tool, title } => vec![call, tool, title],
        EventKind::Diff { path, diff } => vec![path, diff],
        EventKind::Approval {
            request,
            summary,
            choices,
        } => std::iter::once(request.as_str())
            .chain(std::iter::once(summary.as_str()))
            .chain(choices.iter().map(String::as_str))
            .collect(),
        EventKind::Status { model, state, .. } => vec![model, state],
    }
}

/// Every turn's assembled prose and every call's assembled output, keyed by
/// turn or call, as a reader of the transcript would see them.
fn assembled(events: &[Event]) -> Vec<String> {
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    for event in events {
        match &event.kind {
            EventKind::TextDelta { turn, text } => texts
                .entry(format!("turn {turn}"))
                .or_default()
                .push_str(text),
            EventKind::ToolOutput { call, text, .. } => {
                texts.entry(call.clone()).or_default().push_str(text)
            }
            _ => {}
        }
    }
    texts.into_values().collect()
}

fn has_trailing_whitespace(text: &str) -> bool {
    text.contains('\r') || text.split('\n').any(|line| line.ends_with([' ', '\t']))
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}')
}

fn is_emoji(c: char) -> bool {
    matches!(c, '\u{1f300}'..='\u{1faff}' | '\u{2705}')
}

/// Whether some fenced code block in `markdown` holds an indented line.
fn has_indented_code(markdown: &str) -> bool {
    let mut in_code = false;
    markdown.lines().any(|line| {
        if line.starts_with("```") {
            in_code = !in_code;
            return false;
        }
        in_code && line.starts_with("    ") && line.trim_start().len() < line.len()
    })
}

/// Asserts the fixture is a consistent session: time never runs backwards,
/// prose and turn ends belong to the open turn, and tool output and ends
/// belong to a call that is running.
fn assert_consistent(name: &str, events: &[Event]) {
    let mut t = 0;
    let mut turn = 0;
    let mut running = HashSet::new();
    for (i, event) in events.iter().enumerate() {
        assert!(event.t >= t, "{name}: event {i} goes back in time");
        t = event.t;
        match &event.kind {
            EventKind::Prompt { .. } => turn += 1,
            EventKind::TextDelta { turn: n, .. } | EventKind::TurnEnd { turn: n } => {
                assert_eq!(*n, turn, "{name}: event {i} names the wrong turn")
            }
            EventKind::ToolStart { call, .. } => {
                assert!(running.insert(call.clone()), "{name}: {call} started twice")
            }
            EventKind::ToolOutput { call, .. } => {
                assert!(running.contains(call), "{name}: output for idle {call}")
            }
            EventKind::ToolEnd { call, .. } => {
                assert!(running.remove(call), "{name}: {call} ended but never ran")
            }
            _ => {}
        }
    }
    assert!(running.is_empty(), "{name}: calls never ended: {running:?}");
    let turn_ends = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::TurnEnd { .. }))
        .count();
    assert_eq!(turn_ends, turn as usize, "{name}: a turn never ended");
}

#[test]
fn committed_fixtures_match_the_generator() {
    for fixture in FIXTURES {
        let committed = std::fs::read_to_string(fixture_path(fixture.name)).unwrap();
        assert!(
            committed == to_jsonl(&(fixture.events)()),
            "{} is stale: run `cargo run -p agent-console --bin gen-fixtures`",
            fixture.name
        );
    }
}

#[test]
fn short_session_has_one_of_everything() {
    let events = load("short.jsonl");
    assert_eq!(
        counts(&events),
        BTreeMap::from([
            ("approval", 1),
            ("diff", 1),
            ("prompt", 1),
            ("status", 3),
            ("text_delta", 57),
            ("tool_end", 2),
            ("tool_output", 5),
            ("tool_start", 2),
            ("turn_end", 1),
        ])
    );
    assert!(events.len() < 100);
}

#[test]
fn long_tool_output_streams_twenty_thousand_lines_from_one_call() {
    let events = load("long-tool-output.jsonl");
    let c = counts(&events);
    assert_eq!(c["tool_start"], 1);
    assert_eq!(c["tool_end"], 1);
    assert_eq!(c["tool_output"], 1034);
    assert_eq!(c["text_delta"], 21);
    let lines: usize = events
        .iter()
        .map(|e| match &e.kind {
            EventKind::ToolOutput { text, .. } => text.lines().count(),
            _ => 0,
        })
        .sum();
    assert_eq!(lines, LONG_OUTPUT_LINES);
}

#[test]
fn two_hours_spans_two_hours_of_turns() {
    let events = load("two-hours.jsonl");
    assert_eq!(
        counts(&events),
        BTreeMap::from([
            ("approval", 17),
            ("diff", 77),
            ("prompt", 200),
            ("status", 549),
            ("text_delta", 3779),
            ("tool_end", 149),
            ("tool_output", 1913),
            ("tool_start", 149),
            ("turn_end", 200),
        ])
    );
    assert_eq!(counts(&events)["turn_end"], TWO_HOURS_TURNS as usize);
    let end = events.last().unwrap().t;
    assert!(
        (TWO_HOURS_MS * 95 / 100..=TWO_HOURS_MS * 105 / 100).contains(&end),
        "ends at {end} ms"
    );
}

#[test]
fn fixtures_are_consistent_sessions() {
    for fixture in FIXTURES {
        assert_consistent(fixture.name, &load(fixture.name));
    }
}

#[test]
fn no_text_has_trailing_whitespace() {
    for fixture in FIXTURES {
        let events = load(fixture.name);
        for event in &events {
            for text in text_fields(&event.kind) {
                assert!(
                    !has_trailing_whitespace(text),
                    "{} at t={}: {text:?}",
                    fixture.name,
                    event.t
                );
            }
        }
        for text in assembled(&events) {
            assert!(
                !has_trailing_whitespace(&text),
                "{}: {text:?}",
                fixture.name
            );
        }
    }
}

#[test]
fn fixtures_hold_long_lines_wide_characters_and_indented_code() {
    for fixture in FIXTURES {
        let texts = assembled(&load(fixture.name));
        let all = texts.join("\n");
        assert!(
            all.lines().any(|line| line.chars().count() > 80),
            "{}: no line wraps at 80 columns",
            fixture.name
        );
        assert!(all.chars().any(is_cjk), "{}: no CJK", fixture.name);
        assert!(all.chars().any(is_emoji), "{}: no emoji", fixture.name);
        assert!(
            fixture.name == "long-tool-output.jsonl" || texts.iter().any(|t| has_indented_code(t)),
            "{}: no indented code block",
            fixture.name
        );
    }
}
