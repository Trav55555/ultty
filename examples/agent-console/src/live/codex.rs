//! The `codex exec --json` event stream from Codex CLI 0.171.0.

use std::collections::HashMap;

use anyhow::{bail, Context};
use serde::Deserialize;

use agent_console::event::{EventKind, Stream, ToolStatus};

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Record {
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "item.started")]
    ItemStarted { item: Item },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: Item },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: Item },
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: Usage },
    #[serde(rename = "turn.failed")]
    TurnFailed { error: Failure },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Ignored,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Item {
    AgentMessage {
        text: String,
    },
    CommandExecution {
        id: String,
        command: String,
        aggregated_output: String,
        exit_code: Option<i32>,
        status: CommandStatus,
    },
    #[serde(other)]
    Ignored,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandStatus {
    Completed,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Deserialize)]
struct Failure {
    message: String,
}

struct RunningCommand {
    started: u64,
    output: String,
}

enum Outcome {
    Completed,
    Failed,
}

#[derive(Default)]
pub(super) struct Codex {
    running: HashMap<String, RunningCommand>,
    outcome: Option<Outcome>,
}

impl Codex {
    pub(super) fn translate(&mut self, line: &str, t: u64) -> anyhow::Result<Vec<EventKind>> {
        let mut events = Vec::new();
        match serde_json::from_str(line)? {
            Record::TurnStarted => events.push(status(t, "running", 0, 0)),
            Record::ItemStarted { item } | Record::ItemUpdated { item } => {
                self.item(&mut events, item, t, false);
            }
            Record::ItemCompleted { item } => self.item(&mut events, item, t, true),
            Record::TurnCompleted { usage } => {
                self.outcome = Some(Outcome::Completed);
                events.push(EventKind::TurnEnd { turn: 1 });
                events.push(status(
                    t,
                    "completed",
                    usage.input_tokens,
                    usage.output_tokens,
                ));
            }
            Record::TurnFailed { error } => {
                self.outcome = Some(Outcome::Failed);
                events.push(text(error.message));
                events.push(EventKind::TurnEnd { turn: 1 });
                events.push(status(t, "failed", 0, 0));
            }
            Record::Error { message } => {
                events.push(text(message));
                events.push(EventKind::TurnEnd { turn: 1 });
            }
            Record::Ignored => {}
        }
        Ok(events)
    }

    fn item(&mut self, events: &mut Vec<EventKind>, item: Item, t: u64, completed: bool) {
        match item {
            Item::AgentMessage { text: mut message } if completed => {
                if !message.ends_with('\n') {
                    message.push('\n');
                }
                events.push(text(message));
            }
            Item::CommandExecution {
                id,
                command,
                aggregated_output,
                exit_code,
                status,
            } => {
                let running = self.running.entry(id.clone()).or_insert_with(|| {
                    events.push(EventKind::ToolStart {
                        call: id.clone(),
                        tool: "shell".into(),
                        title: command,
                    });
                    RunningCommand {
                        started: t,
                        output: String::new(),
                    }
                });
                let suffix = aggregated_output
                    .strip_prefix(&running.output)
                    .unwrap_or(&aggregated_output);
                if !suffix.is_empty() {
                    events.push(EventKind::ToolOutput {
                        call: id.clone(),
                        stream: Stream::Stdout,
                        text: suffix.into(),
                    });
                }
                running.output = aggregated_output;
                if completed {
                    let duration_ms = t - running.started;
                    self.running.remove(&id);
                    events.push(EventKind::ToolEnd {
                        call: id,
                        status: if matches!(status, CommandStatus::Completed)
                            && exit_code == Some(0)
                        {
                            ToolStatus::Ok
                        } else {
                            ToolStatus::Error
                        },
                        exit_code,
                        duration_ms,
                    });
                }
            }
            _ => {}
        }
    }

    pub(super) fn finish(self) -> anyhow::Result<()> {
        match self
            .outcome
            .context("Codex stream ended before a terminal turn event")?
        {
            Outcome::Completed => Ok(()),
            Outcome::Failed => bail!("Codex turn failed"),
        }
    }
}

fn text(text: String) -> EventKind {
    EventKind::TextDelta { turn: 1, text }
}

fn status(t: u64, state: &str, input_tokens: u64, output_tokens: u64) -> EventKind {
    EventKind::Status {
        model: "Codex".into(),
        input_tokens,
        output_tokens,
        state: state.into(),
        elapsed_ms: t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORDED: &str = include_str!("../../fixtures/live/codex-0.171.0.jsonl");

    #[test]
    fn test_recorded_codex_stream() -> anyhow::Result<()> {
        let mut adapter = Codex::default();
        let mut events = Vec::new();
        for (step, line) in RECORDED.lines().enumerate() {
            events.extend(adapter.translate(line, step as u64 * 10)?);
        }
        assert!(matches!(events.as_slice(), [
            EventKind::Status { state: running, .. },
            EventKind::TextDelta { text: first, .. },
            EventKind::ToolStart { call, tool, title },
            EventKind::ToolOutput { call: output_call, text: output, .. },
            EventKind::ToolEnd {
                call: end_call, status: ToolStatus::Ok, exit_code: Some(0), duration_ms: 10,
            },
            EventKind::TextDelta { text: answer, .. },
            EventKind::TurnEnd { turn: 1 },
            EventKind::Status { state: completed, input_tokens: 50602, output_tokens: 56, .. },
        ] if running == "running" && first == "I’ll run the command once.\n"
            && call == "item_1" && tool == "shell" && title.contains("printf")
            && output_call == call && end_call == call && output == "ULTTY_ADAPTER_TOOL_OK\n"
            && answer == "ULTY_CODEX_ADAPTER_OK\n" && completed == "completed"));
        adapter.finish()
    }

    #[test]
    fn test_command_updates_append_only_new_output() -> anyhow::Result<()> {
        let mut adapter = Codex::default();
        let make_record = |kind, output, code| {
            serde_json::json!({
                "type": kind,
                "item": {
                    "type": "command_execution",
                    "id": "shell",
                    "command": "example",
                    "aggregated_output": output,
                    "status": "failed",
                    "exit_code": code,
                }
            })
        };
        let first = adapter.translate(&make_record("item.updated", "α\n", None).to_string(), 5)?;
        let last = adapter.translate(
            &make_record("item.completed", "α\nβ\n", Some(3)).to_string(),
            8,
        )?;
        assert!(matches!(first.as_slice(), [
            EventKind::ToolStart { .. }, EventKind::ToolOutput { text, .. },
        ] if text == "α\n"));
        assert!(matches!(last.as_slice(), [
            EventKind::ToolOutput { text, .. },
            EventKind::ToolEnd { status: ToolStatus::Error, exit_code: Some(3), duration_ms: 3, .. },
        ] if text == "β\n"));
        Ok(())
    }

    #[test]
    fn test_failed_or_incomplete_turn_reports_error() -> anyhow::Result<()> {
        assert!(Codex::default().finish().is_err());
        let mut adapter = Codex::default();
        let failed = adapter.translate(
            r#"{"type":"turn.failed","error":{"message":"provider unavailable"}}"#,
            50,
        )?;
        assert!(matches!(failed.as_slice(), [
            EventKind::TextDelta { text, .. },
            EventKind::TurnEnd { turn: 1 },
            EventKind::Status { state, .. },
        ] if text == "provider unavailable" && state == "failed"));
        assert!(adapter.finish().is_err());
        Ok(())
    }
}
