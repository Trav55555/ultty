//! The `claude --output-format stream-json` stream from Claude Code 2.1.330.

use std::collections::HashMap;

use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::Value;

use agent_console::event::{EventKind, Stream, ToolStatus};

pub(super) const VERSION: &str = "2.1.330";

#[derive(Deserialize)]
struct Envelope {
    #[serde(flatten)]
    record: Record,
    parent_tool_use_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record {
    System {
        subtype: String,
        model: Option<String>,
        claude_code_version: Option<String>,
    },
    StreamEvent {
        event: PartialEvent,
    },
    Assistant {
        message: AssistantMessage,
    },
    User {
        message: UserMessage,
    },
    Result {
        is_error: bool,
        result: Option<String>,
        #[serde(default)]
        errors: Vec<String>,
        #[serde(default)]
        usage: Usage,
    },
    #[serde(other)]
    Ignored,
}

#[derive(Deserialize)]
struct AssistantMessage {
    id: String,
    content: Vec<Content>,
}

#[derive(Deserialize)]
struct UserMessage {
    content: ToolOutput,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: ToolOutput,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Ignored,
}

#[derive(Default, Deserialize)]
#[serde(untagged)]
enum ToolOutput {
    Text(String),
    Blocks(Vec<Content>),
    #[default]
    Empty,
}

impl ToolOutput {
    fn text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Blocks(blocks) => blocks
                .into_iter()
                .filter_map(|content| match content {
                    Content::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Empty => String::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PartialEvent {
    MessageStart {
        message: MessageId,
    },
    ContentBlockStart {
        index: usize,
        content_block: Content,
    },
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageStop,
    #[serde(other)]
    Ignored,
}

#[derive(Deserialize)]
struct MessageId {
    id: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    TextDelta {
        text: String,
    },
    #[serde(other)]
    Ignored,
}

#[derive(Default, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

struct PartialText {
    index: usize,
    rendered: String,
}

pub(super) struct Claude {
    model: String,
    message_id: Option<String>,
    partial: Option<PartialText>,
    running: HashMap<String, u64>,
    last_text: Option<String>,
    failed: Option<bool>,
}

impl Default for Claude {
    fn default() -> Self {
        Self {
            model: "Claude Code".into(),
            message_id: None,
            partial: None,
            running: HashMap::new(),
            last_text: None,
            failed: None,
        }
    }
}

impl Claude {
    pub(super) fn translate(&mut self, line: &str, t: u64) -> anyhow::Result<Vec<EventKind>> {
        let mut events = Vec::new();
        let envelope: Envelope = serde_json::from_str(line)?;
        if envelope.parent_tool_use_id.is_some() {
            return Ok(events);
        }
        match envelope.record {
            Record::System {
                subtype,
                model,
                claude_code_version,
            } if subtype == "init" => {
                if let Some(version) = claude_code_version.filter(|version| version != VERSION) {
                    bail!(
                        "Claude Code stream version {version:?} is unsupported; expected {VERSION}"
                    );
                }
                if let Some(model) = model {
                    self.model = model;
                }
                events.push(self.status(t, "running", Usage::default()));
            }
            Record::StreamEvent { event } => self.partial_event(event, &mut events),
            Record::Assistant { message } => {
                for block in message.content {
                    self.assistant_block(block, &message.id, t, &mut events);
                }
            }
            Record::User { message } => {
                if let ToolOutput::Blocks(blocks) = message.content {
                    for block in blocks {
                        self.tool_result(block, t, &mut events);
                    }
                }
            }
            Record::Result {
                is_error,
                result,
                errors,
                usage,
            } => {
                self.end_partial(&mut events);
                self.failed = Some(is_error);
                if is_error {
                    for message in result.into_iter().chain(errors) {
                        if self.last_text.as_ref() != Some(&message) {
                            text_line(&mut events, &message);
                            self.last_text = Some(message);
                        }
                    }
                }
                events.push(EventKind::TurnEnd { turn: 1 });
                events.push(self.status(t, if is_error { "failed" } else { "completed" }, usage));
            }
            _ => {}
        }
        Ok(events)
    }

    fn partial_event(&mut self, event: PartialEvent, events: &mut Vec<EventKind>) {
        match event {
            PartialEvent::MessageStart { message } => {
                self.end_partial(events);
                self.message_id = Some(message.id);
            }
            PartialEvent::ContentBlockStart {
                index,
                content_block: Content::Text { text },
            } => {
                self.end_partial(events);
                if !text.is_empty() {
                    events.push(delta(text.clone()));
                }
                self.partial = Some(PartialText {
                    index,
                    rendered: text,
                });
            }
            PartialEvent::ContentBlockDelta {
                index,
                delta: Delta::TextDelta { text },
            } => {
                let partial = self.partial.get_or_insert_with(|| PartialText {
                    index,
                    rendered: String::new(),
                });
                if partial.index == index {
                    partial.rendered.push_str(&text);
                    events.push(delta(text));
                }
            }
            PartialEvent::ContentBlockStop { index }
                if self
                    .partial
                    .as_ref()
                    .is_some_and(|partial| partial.index == index) =>
            {
                self.end_partial(events);
            }
            PartialEvent::MessageStop => {
                self.end_partial(events);
                self.message_id = None;
            }
            _ => {}
        }
    }

    fn end_partial(&mut self, events: &mut Vec<EventKind>) {
        if let Some(partial) = self.partial.take() {
            if !partial.rendered.is_empty() && !partial.rendered.ends_with('\n') {
                events.push(delta("\n".into()));
            }
        }
    }

    fn assistant_block(&mut self, block: Content, id: &str, t: u64, events: &mut Vec<EventKind>) {
        match block {
            Content::Text { text } => {
                let partial = self.partial.take();
                let suffix = partial
                    .as_ref()
                    .filter(|_| self.message_id.as_deref() == Some(id))
                    .and_then(|partial| text.strip_prefix(&partial.rendered));
                if let Some(suffix) = suffix {
                    if !suffix.is_empty() {
                        events.push(delta(suffix.into()));
                    }
                    if !text.ends_with('\n') {
                        events.push(delta("\n".into()));
                    }
                } else {
                    self.partial = partial;
                    self.end_partial(events);
                    text_line(events, &text);
                }
                self.last_text = Some(text);
            }
            Content::ToolUse { id, name, input } => {
                self.end_partial(events);
                self.running.insert(id.clone(), t);
                let title = ["command", "file_path", "query", "pattern", "url"]
                    .into_iter()
                    .find_map(|field| input[field].as_str())
                    .map_or_else(|| input.to_string(), str::to_owned);
                events.push(EventKind::ToolStart {
                    call: id,
                    tool: name,
                    title,
                });
            }
            _ => {}
        }
    }

    fn tool_result(&mut self, block: Content, t: u64, events: &mut Vec<EventKind>) {
        let Content::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = block
        else {
            return;
        };
        let Some(started) = self.running.remove(&tool_use_id) else {
            return;
        };
        let text = content.text();
        if !text.is_empty() {
            events.push(EventKind::ToolOutput {
                call: tool_use_id.clone(),
                stream: if is_error {
                    Stream::Stderr
                } else {
                    Stream::Stdout
                },
                text,
            });
        }
        events.push(EventKind::ToolEnd {
            call: tool_use_id,
            status: if is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            },
            exit_code: None,
            duration_ms: t - started,
        });
    }

    fn status(&self, t: u64, state: &str, usage: Usage) -> EventKind {
        EventKind::Status {
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            state: state.into(),
            elapsed_ms: t,
        }
    }

    pub(super) fn finish(self) -> anyhow::Result<()> {
        if self
            .failed
            .context("Claude Code stream ended before its result")?
        {
            bail!("Claude Code turn failed");
        }
        Ok(())
    }
}

fn delta(text: String) -> EventKind {
    EventKind::TextDelta { turn: 1, text }
}

fn text_line(events: &mut Vec<EventKind>, text: &str) {
    if !text.is_empty() {
        events.push(delta(if text.ends_with('\n') {
            text.into()
        } else {
            format!("{text}\n")
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE_LIMIT: &str = include_str!("../../fixtures/live/claude-2.1.330-rate-limit.jsonl");
    const STREAM_SPEC: &str = include_str!("../../fixtures/live/claude-2.1.330-stream-spec.jsonl");

    #[test]
    fn test_streamed_text_and_full_blocks_do_not_duplicate() -> anyhow::Result<()> {
        let mut adapter = Claude::default();
        let mut batches = Vec::new();
        for (step, line) in STREAM_SPEC.lines().enumerate() {
            batches.push(adapter.translate(line, step as u64 * 10)?);
        }
        assert!(matches!(batches[3].as_slice(), [
            EventKind::TextDelta { text, .. }
        ] if text == "I’ll inspect"));
        let events = batches.into_iter().flatten().collect::<Vec<_>>();
        let text = events
            .iter()
            .filter_map(|event| match event {
                EventKind::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "I’ll inspect the probe.\nULTY_CLAUDE_ADAPTER_OK\n");
        assert!(events.iter().any(|event| matches!(event,
            EventKind::ToolStart { call, tool, title }
            if call == "tool_probe" && tool == "Read" && title == "probe.txt"
        )));
        assert!(events.iter().any(|event| matches!(event,
            EventKind::ToolOutput { call, text, .. }
            if call == "tool_probe" && text == "ULTTY_CLAUDE_TOOL_OK"
        )));
        assert!(events.iter().any(|event| matches!(event,
            EventKind::ToolEnd { call, status: ToolStatus::Ok, duration_ms: 40, .. }
            if call == "tool_probe"
        )));
        assert!(matches!(events.last(), Some(EventKind::Status {
            state, input_tokens: 27, output_tokens: 18, ..
        }) if state == "completed"));
        adapter.finish()
    }

    #[test]
    fn test_recorded_rate_limit_reports_failure_once() -> anyhow::Result<()> {
        let mut adapter = Claude::default();
        let mut messages = Vec::new();
        let mut last = None;
        for line in RATE_LIMIT.lines() {
            for event in adapter.translate(line, 10)? {
                if let EventKind::TextDelta { text, .. } = &event {
                    messages.push(text.clone());
                }
                last = Some(event);
            }
        }
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("You've hit your weekly limit"));
        assert!(matches!(last, Some(EventKind::Status { state, .. }) if state == "failed"));
        assert!(adapter.finish().is_err());
        assert!(Claude::default().finish().is_err());
        Ok(())
    }
}
