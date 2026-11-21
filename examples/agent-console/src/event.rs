//! The agent event schema: one JSON object per line, documented in
//! `EVENTS.md`.

use serde::{Deserialize, Serialize};

/// One agent event, stamped with its offset from the start of the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Milliseconds since the session started; replay paces itself by the
    /// differences between consecutive events.
    pub t: u64,
    /// What happened.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// An event of `kind` at `t` milliseconds into the session.
    pub fn new(t: u64, kind: EventKind) -> Self {
        Self { t, kind }
    }
}

/// The event types, tagged on the wire by a `"type"` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// The user's prompt, which opens a turn.
    Prompt {
        /// The prompt as typed.
        text: String,
    },
    /// Markdown appended to the assistant's prose for `turn`.
    TextDelta {
        /// The turn this text belongs to.
        turn: u32,
        /// The appended text; deltas concatenate into the turn's prose.
        text: String,
    },
    /// The assistant finished `turn`.
    TurnEnd {
        /// The turn that ended.
        turn: u32,
    },
    /// A tool call started.
    ToolStart {
        /// Identifies the call in its `tool_output` and `tool_end` events.
        call: String,
        /// The tool's name, such as `bash` or `edit`.
        tool: String,
        /// A one-line summary of the call's arguments.
        title: String,
    },
    /// A chunk of a running tool call's output.
    ToolOutput {
        /// The call this output belongs to.
        call: String,
        /// Which stream the chunk came from.
        stream: Stream,
        /// The chunk; chunks concatenate into the call's output.
        text: String,
    },
    /// A tool call finished.
    ToolEnd {
        /// The call that finished.
        call: String,
        /// Whether the call succeeded.
        status: ToolStatus,
        /// The process exit code, for tools that run one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// How long the call ran.
        duration_ms: u64,
    },
    /// A change the agent made to a file.
    Diff {
        /// The file, relative to the workspace.
        path: String,
        /// The change as a unified diff.
        diff: String,
    },
    /// The agent asks the user to approve an action.
    Approval {
        /// Identifies the request so the answer can name it.
        request: String,
        /// The command or action awaiting approval.
        summary: String,
        /// The answers the user can give, in display order.
        choices: Vec<String>,
    },
    /// A status line update.
    Status {
        /// The model serving the session.
        model: String,
        /// Input tokens used so far.
        input_tokens: u64,
        /// Output tokens used so far.
        output_tokens: u64,
        /// What the agent is doing, such as `thinking` or `running tool`.
        state: String,
        /// Time since the session started, as the agent reports it.
        elapsed_ms: u64,
    },
}

impl EventKind {
    /// Every wire tag, in schema order.
    pub const TYPES: [&'static str; 9] = [
        "prompt",
        "text_delta",
        "turn_end",
        "tool_start",
        "tool_output",
        "tool_end",
        "diff",
        "approval",
        "status",
    ];

    /// The wire tag of this event, one of [`EventKind::TYPES`].
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::TextDelta { .. } => "text_delta",
            Self::TurnEnd { .. } => "turn_end",
            Self::ToolStart { .. } => "tool_start",
            Self::ToolOutput { .. } => "tool_output",
            Self::ToolEnd { .. } => "tool_end",
            Self::Diff { .. } => "diff",
            Self::Approval { .. } => "approval",
            Self::Status { .. } => "status",
        }
    }
}

/// The output stream a tool chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// The call succeeded.
    Ok,
    /// The call failed.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One event of every type, in [`EventKind::TYPES`] order.
    fn one_of_each() -> Vec<Event> {
        vec![
            Event::new(
                0,
                EventKind::Prompt {
                    text: "Fix the tab width".into(),
                },
            ),
            Event::new(
                10,
                EventKind::TextDelta {
                    turn: 1,
                    text: "Looking at `parse.rs`:\n- 一\n- 🦀".into(),
                },
            ),
            Event::new(20, EventKind::TurnEnd { turn: 1 }),
            Event::new(
                30,
                EventKind::ToolStart {
                    call: "c1".into(),
                    tool: "bash".into(),
                    title: "cargo test".into(),
                },
            ),
            Event::new(
                40,
                EventKind::ToolOutput {
                    call: "c1".into(),
                    stream: Stream::Stderr,
                    text: "warning: unused\n".into(),
                },
            ),
            Event::new(
                50,
                EventKind::ToolEnd {
                    call: "c1".into(),
                    status: ToolStatus::Error,
                    exit_code: Some(101),
                    duration_ms: 1234,
                },
            ),
            Event::new(
                60,
                EventKind::Diff {
                    path: "src/parse.rs".into(),
                    diff: "@@ -1 +1 @@\n-a\n+b\n".into(),
                },
            ),
            Event::new(
                70,
                EventKind::Approval {
                    request: "r1".into(),
                    summary: "rm -rf target".into(),
                    choices: vec!["yes".into(), "no".into()],
                },
            ),
            Event::new(
                80,
                EventKind::Status {
                    model: "model-x".into(),
                    input_tokens: 1,
                    output_tokens: 2,
                    state: "idle".into(),
                    elapsed_ms: 80,
                },
            ),
        ]
    }

    #[test]
    fn every_event_type_round_trips() {
        let events = one_of_each();
        let tags: Vec<_> = events.iter().map(|e| e.kind.type_name()).collect();
        assert_eq!(tags, EventKind::TYPES);
        for event in events {
            let line = serde_json::to_string(&event).unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["type"], event.kind.type_name(), "{line}");
            assert_eq!(value["t"], event.t, "{line}");
            let back: Event = serde_json::from_str(&line).unwrap();
            assert_eq!(back, event, "{line}");
        }
    }

    #[test]
    fn tool_end_without_exit_code_omits_and_accepts_the_field() {
        let event = Event::new(
            5,
            EventKind::ToolEnd {
                call: "c2".into(),
                status: ToolStatus::Ok,
                exit_code: None,
                duration_ms: 7,
            },
        );
        let line = serde_json::to_string(&event).unwrap();
        assert_eq!(
            line,
            r#"{"t":5,"type":"tool_end","call":"c2","status":"ok","duration_ms":7}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), event);
    }
}
