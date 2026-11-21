//! Native mode against a recording [`Cards`]: which cards open with how
//! many rows, what they show, when they close, and what is printed as text
//! between them, for the short fixture.

use agent_console::native::{CardKind, Cards, NativeScreen};
use agent_console::screen::Result;
use agent_console::style::Line;

use crate::common::{assert_prose_counts, assert_segments_in_order, load, prose_segments, run};

/// One call the console made.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Print(String),
    Open {
        card: u32,
        rows: u16,
        kind: CardKind,
        text: Vec<String>,
    },
    Update {
        card: u32,
        text: Vec<String>,
    },
    Close(u32),
}

#[derive(Default)]
struct Recorder {
    calls: Vec<Call>,
    next: u32,
    open: Vec<u32>,
}

fn text(lines: &[Line]) -> Vec<String> {
    lines.iter().map(Line::text).collect()
}

impl Cards for Recorder {
    type Card = u32;

    fn width(&self) -> u16 {
        80
    }

    fn print(&mut self, text: &str) -> Result<()> {
        self.calls.push(Call::Print(text.to_string()));
        Ok(())
    }

    fn open(&mut self, rows: u16, lines: &[Line], kind: CardKind) -> Result<u32> {
        assert!((1..=128).contains(&rows), "{rows} rows");
        assert!(
            lines.len() <= usize::from(rows),
            "{} lines in {rows} rows",
            lines.len()
        );
        self.next += 1;
        self.open.push(self.next);
        self.calls.push(Call::Open {
            card: self.next,
            rows,
            kind,
            text: text(lines),
        });
        Ok(self.next)
    }

    fn update(&mut self, card: &u32, lines: &[Line], _kind: CardKind) -> Result<()> {
        assert!(self.open.contains(card), "update of card {card}, not open");
        self.calls.push(Call::Update {
            card: *card,
            text: text(lines),
        });
        Ok(())
    }

    fn close(&mut self, card: u32) -> Result<()> {
        assert!(self.open.contains(&card), "close of card {card}, not open");
        self.open.retain(|&c| c != card);
        self.calls.push(Call::Close(card));
        Ok(())
    }

    fn take_input(&mut self) -> Vec<u8> {
        Vec::new()
    }

    fn drain_events(&mut self) -> Result<()> {
        Ok(())
    }
}

/// `text` without SGR sequences and without the live card's retirement.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.next() == Some('[') {
                while chars.next().is_some_and(|c| !c.is_ascii_alphabetic()) {}
            }
        } else if ch != '\r' {
            out.push(ch);
        }
    }
    out
}

fn record() -> Recorder {
    let events = load("short.jsonl");
    let recorder = run(NativeScreen::new(Recorder::default()), &events).into_inner();
    assert!(
        recorder.open.is_empty(),
        "cards left open: {:?}",
        recorder.open
    );
    recorder
}

fn opened(calls: &[Call], kind: CardKind) -> Vec<(u32, u16, Vec<String>)> {
    calls
        .iter()
        .filter_map(|c| match c {
            Call::Open {
                card,
                rows,
                kind: k,
                text,
            } if *k == kind => Some((*card, *rows, text.clone())),
            _ => None,
        })
        .collect()
}

/// The last view card `card` showed before it closed.
fn final_view(calls: &[Call], card: u32) -> Vec<String> {
    let close = calls.iter().position(|c| *c == Call::Close(card)).unwrap();
    calls[..close]
        .iter()
        .rev()
        .find_map(|c| match c {
            Call::Update { card: id, text } | Call::Open { card: id, text, .. } if *id == card => {
                Some(text.clone())
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn test_blocks_are_cards_of_fixed_height_frozen_with_their_final_view() {
    let calls = record().calls;
    let blocks = opened(&calls, CardKind::Block);
    let shape: Vec<(u16, &str)> = blocks
        .iter()
        .map(|(_, rows, text)| (*rows, text[0].as_str()))
        .collect();
    assert_eq!(
        shape,
        [
            (8, "[bash] cargo test -p pager parse:: (running, 0.0s)"),
            (21, "[diff] src/parse.rs"),
            (4, "[approve] cargo test -p pager"),
            (8, "[bash] cargo test -p pager (running, 0.0s)"),
        ]
    );
    let (tool, _, _) = &blocks[0];
    let updates = calls
        .iter()
        .filter(|c| matches!(c, Call::Update { card, .. } if card == tool))
        .count();
    assert!(updates >= 4, "the tool card was updated {updates} times");
    let last = final_view(&calls, *tool);
    assert_eq!(last.len(), 8);
    assert_eq!(
        last[0],
        "[bash] cargo test -p pager parse:: (failed, exit 101, 10 lines, 3.9s)"
    );
    assert_eq!(
        last[7],
        "test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
    );
    assert_eq!(
        final_view(&calls, blocks[2].0),
        [
            "[approve] cargo test -p pager (Allow once)",
            "> 1. Allow once",
            "  2. Always allow `cargo test`",
            "  3. Deny"
        ]
    );
    for (card, _, _) in &blocks {
        let closes = calls.iter().filter(|c| **c == Call::Close(*card)).count();
        assert_eq!(closes, 1, "card {card} closed {closes} times");
        let close = calls.iter().position(|c| *c == Call::Close(*card)).unwrap();
        assert!(
            !calls[close..]
                .iter()
                .any(|c| matches!(c, Call::Update { card: id, .. } if id == card)),
            "card {card} updated after it closed"
        );
    }
}

#[test]
fn test_the_live_card_sits_at_the_bottom_and_is_erased_before_anything_follows() {
    let calls = record().calls;
    let live = opened(&calls, CardKind::Live);
    assert!(!live.is_empty(), "no live card");
    for (card, rows, text) in &live {
        assert_eq!(*rows, 2);
        assert_eq!(text.len(), 2);
        let close = calls
            .iter()
            .position(|c| *c == Call::Close(*card))
            .expect("every live card is closed");
        let Some(Call::Print(next)) = calls.get(close + 1) else {
            panic!("live card {card} closed without its rows being erased");
        };
        assert!(
            next.starts_with("\x1b[2A\r\x1b[2K\x1b[J"),
            "after live card {card}: {next:?}"
        );
        let open = calls
            .iter()
            .position(|c| matches!(c, Call::Open { card: id, .. } if id == card))
            .unwrap();
        assert!(
            !calls[open + 1..close]
                .iter()
                .any(|c| matches!(c, Call::Print(_) | Call::Open { .. })),
            "something was printed or opened under live card {card}"
        );
    }
    assert!(
        live.iter()
            .any(|(_, _, text)| text[1] == "example-model-1 | in 921 | out 254 | editing | 4.4s"),
        "the status never showed in the live card"
    );
}

#[test]
fn test_prose_is_printed_as_text_in_order_once() {
    let events = load("short.jsonl");
    let calls = record().calls;
    let printed: String = calls
        .iter()
        .filter_map(|c| match c {
            Call::Print(text) => Some(plain(text)),
            _ => None,
        })
        .collect();
    let lines: Vec<String> = printed.lines().map(String::from).collect();
    let segments = prose_segments(&events);
    assert_segments_in_order(&lines, &segments);
    assert_prose_counts(&lines, &segments);
    assert_eq!(
        lines.last().unwrap(),
        "example-model-1 | in 921 | out 424 | idle | 9.6s"
    );
    for line in &lines {
        assert_eq!(line.trim_end(), line, "printed with trailing spaces");
    }
}
