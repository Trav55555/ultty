//! Reads events from JSON Lines, skipping lines it cannot use.
//!
//! A malformed line and an event of an unknown type are both skipped with a
//! warning and remembered with their line numbers, so one bad line does not
//! end a session and a newer producer can add event types.

use std::io::{self, BufRead};

use serde::de::Error as _;
use serde_json::Value;
use thiserror::Error;

use crate::event::{Event, EventKind};

/// A line that is not a valid event.
#[derive(Debug, Error)]
#[error("line {line}: {error}")]
pub struct MalformedLine {
    /// The 1-based line number.
    pub line: usize,
    /// Why the line did not parse.
    #[source]
    pub error: serde_json::Error,
}

/// A well-formed event whose type this reader does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownType {
    /// The 1-based line number.
    pub line: usize,
    /// The event's `type` tag.
    pub type_name: String,
}

/// Reading the input failed; the stream cannot continue.
#[derive(Debug, Error)]
#[error("line {line}: read failed")]
pub struct ReadError {
    /// The 1-based number of the line being read.
    pub line: usize,
    /// The underlying I/O error.
    #[source]
    pub source: io::Error,
}

/// An iterator over the events in a JSON Lines stream.
///
/// Blank lines are ignored. Malformed lines and unknown event types are
/// skipped and listed by [`EventReader::malformed`] and
/// [`EventReader::unknown`]; only an I/O error ends the stream early.
pub struct EventReader<R> {
    input: R,
    line: usize,
    buf: String,
    malformed: Vec<MalformedLine>,
    unknown: Vec<UnknownType>,
}

impl<R: BufRead> EventReader<R> {
    /// A reader over `input`, which may be a file or stdin.
    pub fn new(input: R) -> Self {
        Self {
            input,
            line: 0,
            buf: String::new(),
            malformed: Vec::new(),
            unknown: Vec::new(),
        }
    }

    /// The lines skipped so far because they were not valid events.
    pub fn malformed(&self) -> &[MalformedLine] {
        &self.malformed
    }

    /// The events skipped so far because their type is unknown.
    pub fn unknown(&self) -> &[UnknownType] {
        &self.unknown
    }

    /// The event on the current line, or `None` after recording why the
    /// line was skipped.
    fn parse(&mut self) -> Option<Event> {
        let text = self.buf.trim();
        if text.is_empty() {
            return None;
        }
        let line = self.line;
        match classify(text) {
            Ok(Line::Event(event)) => Some(event),
            Ok(Line::Unknown(type_name)) => {
                tracing::warn!(line, %type_name, "skipping event of unknown type");
                self.unknown.push(UnknownType { line, type_name });
                None
            }
            Err(error) => {
                let malformed = MalformedLine { line, error };
                tracing::warn!(%malformed, "skipping malformed line");
                self.malformed.push(malformed);
                None
            }
        }
    }
}

/// A well-formed line: an event, or the tag of an event type this reader
/// does not know.
enum Line {
    Event(Event),
    Unknown(String),
}

fn classify(text: &str) -> Result<Line, serde_json::Error> {
    let value: Value = serde_json::from_str(text)?;
    let type_name = value
        .get("type")
        .ok_or_else(|| serde_json::Error::missing_field("type"))?
        .as_str()
        .ok_or_else(|| serde_json::Error::custom("`type` is not a string"))?;
    if EventKind::TYPES.contains(&type_name) {
        serde_json::from_value(value).map(Line::Event)
    } else {
        Ok(Line::Unknown(type_name.to_owned()))
    }
}

impl<R: BufRead> Iterator for EventReader<R> {
    type Item = Result<Event, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.buf.clear();
            self.line += 1;
            match self.input.read_line(&mut self.buf) {
                Ok(0) => return None,
                Ok(_) => {
                    if let Some(event) = self.parse() {
                        return Some(Ok(event));
                    }
                }
                Err(source) => {
                    return Some(Err(ReadError {
                        line: self.line,
                        source,
                    }))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;

    fn read_all(input: &str) -> (Vec<Event>, EventReader<&[u8]>) {
        let mut reader = EventReader::new(input.as_bytes());
        let events = reader.by_ref().map(Result::unwrap).collect();
        (events, reader)
    }

    #[test]
    fn reads_events_and_ignores_blank_lines() {
        let (events, reader) = read_all(
            "{\"t\":0,\"type\":\"prompt\",\"text\":\"hi\"}\n\n   \n{\"t\":5,\"type\":\"turn_end\",\"turn\":1}",
        );
        assert_eq!(
            events,
            [
                Event::new(0, EventKind::Prompt { text: "hi".into() }),
                Event::new(5, EventKind::TurnEnd { turn: 1 }),
            ]
        );
        assert!(reader.malformed().is_empty());
        assert!(reader.unknown().is_empty());
    }

    #[test]
    fn unknown_event_types_are_skipped_and_counted() {
        let (events, reader) = read_all(concat!(
            "{\"t\":0,\"type\":\"thinking\",\"text\":\"hmm\"}\n",
            "{\"t\":1,\"type\":\"turn_end\",\"turn\":1}\n",
            "{\"t\":2,\"type\":\"thinking\"}\n",
        ));
        assert_eq!(events, [Event::new(1, EventKind::TurnEnd { turn: 1 })]);
        assert_eq!(
            reader.unknown(),
            [
                UnknownType {
                    line: 1,
                    type_name: "thinking".into()
                },
                UnknownType {
                    line: 3,
                    type_name: "thinking".into()
                },
            ]
        );
        assert!(reader.malformed().is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped_with_their_line_numbers() {
        let (events, reader) = read_all(concat!(
            "{\"t\":0,\"type\":\"turn_end\",\"turn\":1}\n",
            "{not json\n",
            "{\"t\":2,\"text\":\"no type\"}\n",
            "{\"t\":3,\"type\":\"turn_end\"}\n",
            "{\"t\":4,\"type\":7}\n",
            "{\"t\":5,\"type\":\"turn_end\",\"turn\":2}\n",
        ));
        assert_eq!(
            events,
            [
                Event::new(0, EventKind::TurnEnd { turn: 1 }),
                Event::new(5, EventKind::TurnEnd { turn: 2 }),
            ]
        );
        let lines: Vec<_> = reader.malformed().iter().map(|m| m.line).collect();
        assert_eq!(lines, [2, 3, 4, 5]);
        let shown = reader.malformed()[2].to_string();
        assert!(shown.starts_with("line 4: "), "{shown}");
        assert!(shown.contains("turn"), "{shown}");
    }
}
