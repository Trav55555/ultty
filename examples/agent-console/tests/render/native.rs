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

    fn live_row_limit(&self) -> u16 {
        23
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

#[test]
fn test_native_blocks_preview_live_and_print_their_final_source() {
    let calls = record().calls;
    let blocks = opened(&calls, CardKind::Block);

    assert!(blocks
        .iter()
        .any(|(_, _, text)| text[0] == "[bash] cargo test -p pager parse:: (running, 0.0s)"));
    assert!(blocks
        .iter()
        .any(|(_, _, text)| text[0] == "[approve] cargo test -p pager"));
    assert!(calls.iter().any(|call| matches!(
        call,
        Call::Update { text, .. } if text.iter().any(|line| line.contains("running 4 tests"))
    )));

    let printed: String = calls
        .iter()
        .filter_map(|call| match call {
            Call::Print(text) => Some(plain(text)),
            _ => None,
        })
        .collect();
    for line in [
        "[bash] cargo test -p pager parse:: (failed, exit 101, 10 lines, 3.9s)",
        "test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
        "+    let mut column = 0;",
        "[approve] cargo test -p pager (Allow once)",
        "  3. Deny",
    ] {
        assert_eq!(printed.lines().filter(|printed| *printed == line).count(), 1, "{line}");
    }
}

#[test]
fn test_a_preview_is_erased_before_anything_is_printed_under_it() {
    let calls = record().calls;
    for kind in [CardKind::Block, CardKind::Live] {
        for (card, rows, _) in opened(&calls, kind) {
            let close = calls
                .iter()
                .position(|call| *call == Call::Close(card))
                .expect("every preview is closed");
            let Some(Call::Print(next)) = calls.get(close + 1) else {
                panic!("preview {card} closed without its rows being erased");
            };
            assert_eq!(next, &format!("\x1b[{rows}A\r\x1b[2K\x1b[J\r"));
            let open = calls
                .iter()
                .position(|call| matches!(call, Call::Open { card: id, .. } if *id == card))
                .unwrap();
            assert!(!calls[open + 1..close]
                .iter()
                .any(|call| matches!(call, Call::Print(_) | Call::Open { .. })));
        }
    }
    assert!(opened(&calls, CardKind::Live)
        .iter()
        .any(|(_, _, text)| text[1] == "example-model-1 | in 921 | out 254 | editing | 4.4s"));
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
