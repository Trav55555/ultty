//! Client presentation of a daemon-owned run and its pending decisions.

use tokio::sync::mpsc;
use ulttui::prelude::Cmd;
use ulttui::toast::Toast;

use crate::approval_client::{self, ApprovalDecision};
use crate::connection::{self, RunSnapshot};
use crate::native_stream::{self, Control, Event, Submission};
use crate::types::{ApprovalRequest, ChatMessage, ToolCall, ToolStatus};
use crate::views::format_timestamp;
use crate::{Msg, PorygonTui};

impl PorygonTui {
    pub(crate) fn start_native_turn(&mut self) -> Cmd<Msg> {
        if self.waiting_response
            || self.stopping
            || (self.run_unresolved && !self.run_can_continue)
            || self.pending_approval.is_some()
        {
            return Cmd::ShowToast(Toast::warning(
                "Work is unresolved. Use /reconnect to inspect it; your draft is preserved.",
            ));
        }
        let Some(connection) = self.connection.clone().filter(|_| self.connected) else {
            return Cmd::ShowToast(Toast::warning(
                "Daemon is not authenticated. Your draft is preserved.",
            ));
        };
        let run_id = match self
            .current_run
            .clone()
            .filter(|_| self.run_can_continue)
            .map(Ok)
            .unwrap_or_else(connection::new_run_id)
        {
            Ok(id) => id,
            Err(error) => return Cmd::ShowToast(Toast::error(error)),
        };
        let message = self.input.submit_value();
        self.persist_conversation();
        self.suggestions.clear();
        self.messages.push(ChatMessage {
            role: "user".into(),
            content: message.clone(),
            timestamp: format_timestamp(),
        });
        self.messages.push(ChatMessage {
            role: "system".into(),
            content: format!("Run `{run_id}`. If disconnected, use `/attach {run_id}` to inspect it. The prompt will not be resent."),
            timestamp: format_timestamp(),
        });
        self.messages.push(ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            timestamp: format_timestamp(),
        });
        self.current_run = Some(run_id.clone());
        self.run_epoch = self.run_epoch.wrapping_add(1);
        self.current_response = self.messages.len().checked_sub(1);
        self.submitting_parked_run = self.run_can_continue;
        self.run_can_continue = false;
        self.run_unresolved = true;
        self.waiting_response = true;
        self.stopping = false;
        self.error = None;
        self.status.step = "submitting (not yet confirmed)".into();
        self.streaming_content.clear();
        self.stream_token_count = 0;
        self.tool_calls.clear();
        self.tool_ids.clear();
        self.flush_drafts();
        self.begin_stream_metrics();
        self.auto_scroll_chat();
        let (sender, receiver) = mpsc::channel(8);
        self.stream_control = Some(sender);
        Cmd::subscribe(
            "agent-stream",
            native_stream::stream(
                Submission {
                    connection,
                    epoch: self.run_epoch,
                    run_id,
                    conversation_id: self.conversation_id.clone(),
                    message,
                },
                receiver,
            ),
        )
    }

    pub(crate) fn native_event(&mut self, run_id: String, event: Event) -> Cmd<Msg> {
        if self.current_run.as_deref() != Some(&run_id) {
            return Cmd::none();
        }
        match event {
            Event::NotSubmitted { message, error } => {
                self.end_native_stream();
                self.run_unresolved = self.submitting_parked_run;
                self.run_can_continue = self.submitting_parked_run;
                self.stopping = false;
                if !self.submitting_parked_run {
                    self.current_run = None;
                }
                self.status.step = "not submitted; draft preserved".into();
                if self.input.current_value().is_empty() { self.input.set_value(message); }
                self.error = Some(error.clone());
                Cmd::ShowToast(Toast::error(error))
            }
            Event::Submitted => {
                self.status.step = "submitted; awaiting daemon".into();
                Cmd::none()
            }
            Event::Token(text) => self.handle_update(Msg::StreamToken(text)),
            Event::Reset => {
                self.streaming_content.clear();
                self.stream_token_count = 0;
                if let Some(message) = self.current_response.and_then(|index| self.messages.get_mut(index)) {
                    message.content.clear();
                }
                Cmd::none()
            }
            Event::Approval {
                id,
                tool,
                arguments,
            } => {
                self.handle_update(Msg::ApprovalReceived(ApprovalRequest {
                    id, tool_name: tool, params: arguments, risk_score: -1.0,
                    reasoning: format!("Run {run_id}. Approve this action once, or deny. Scope and reversibility are not supplied by this stream."),
                    blast_radius: "unknown".into(), reversible: None,
                }, true))
            }
            Event::DecisionSent(id) => {
                if self.pending_approval.as_ref().is_some_and(|r| r.id == id) {
                    self.show_approval_modal = false;
                    self.pending_approval = None;
                    self.approval_in_flight = false;
                    self.approval_is_stream = false;
                    self.status.step = "decision sent; awaiting daemon outcome".into();
                    if let Some(next) = self.queued_stream_approvals.pop_front() {
                        return self.handle_update(Msg::ApprovalReceived(next, true));
                    }
                    return Cmd::ShowToast(Toast::warning(
                        "Decision sent. Tool outcome is not yet confirmed.",
                    ));
                }
                Cmd::none()
            }
            Event::ToolCall {
                id,
                name,
                arguments,
            } => {
                self.tool_ids.push(id);
                self.tool_calls.push(ToolCall {
                    name,
                    args: arguments.to_string(),
                    status: ToolStatus::Running,
                    result: None,
                });
                Cmd::none()
            }
            Event::ToolResult {
                id,
                result,
                success,
            } => {
                if let Some(index) = self.tool_ids.iter().rposition(|current| current == &id) {
                    self.tool_calls[index].status = if success {
                        ToolStatus::Completed
                    } else {
                        ToolStatus::Failed
                    };
                    self.tool_calls[index].result = Some(result);
                }
                Cmd::none()
            }
            Event::Error(error) => {
                self.error = Some(error.clone());
                self.status.step = "stream error; run outcome unconfirmed".into();
                Cmd::ShowToast(Toast::error(error))
            }
            Event::Done(conversation) => {
                if let Some(id) = conversation {
                    self.assign_conversation(id);
                }
                self.end_native_stream();
                self.status.step = "stream ended; checking run outcome".into();
                Cmd::batch(vec![
                    Cmd::unsubscribe("agent-stream"),
                    Cmd::message(Msg::InspectRun),
                ])
            }
            Event::Disconnected => {
                if !self.waiting_response {
                    return Cmd::none();
                }
                self.end_native_stream();
                self.status.step = "disconnected; run outcome unconfirmed".into();
                Cmd::batch(vec![
                    Cmd::unsubscribe("agent-stream"),
                    Cmd::message(Msg::InspectRun),
                ])
            }
        }
    }

    fn end_native_stream(&mut self) {
        let _ = self.finish_stream_metrics();
        self.waiting_response = false;
        self.stream_control = None;
        self.queued_stream_approvals.clear();
        if self.approval_is_stream {
            self.pending_approval = None;
            self.show_approval_modal = false;
            self.approval_in_flight = false;
        }
    }

    pub(crate) fn inspect_run(&self, cancel: bool) -> Cmd<Msg> {
        let (Some(connection), Some(id)) = (self.connection.clone(), self.current_run.clone())
        else {
            return Cmd::none();
        };
        let epoch = self.run_epoch;
        Cmd::spawn(async move {
            if cancel {
                Msg::StopObserved(id.clone(), epoch, connection.cancel_run(&id).await)
            } else {
                Msg::RunObserved(id.clone(), epoch, connection.fetch_run(&id).await)
            }
        })
    }

    pub(crate) fn observe_run(
        &mut self,
        id: String,
        result: Result<RunSnapshot, String>,
    ) -> Cmd<Msg> {
        if self.current_run.as_deref() != Some(&id) || !self.run_unresolved {
            return Cmd::none();
        }
        match result {
            Err(error) => {
                self.error = Some(format!(
                    "Run {id}: {error}. Outcome remains unconfirmed; no prompt was resent."
                ));
                self.status.step = "run outcome unconfirmed; /reconnect to inspect".into();
                Cmd::none()
            }
            Ok(run) => {
                if let Some(id) = run.metadata["conversation_id"].as_str() {
                    self.assign_conversation(id.to_string());
                }
                let terminal = matches!(
                    run.status.as_str(),
                    "completed" | "failed" | "cancelled" | "insufficient_oracle"
                );
                self.run_unresolved = !terminal;
                self.run_can_continue = run.status == "awaiting_input";
                self.status.step = format!("run {}", run.status);
                if let Some(error) = run.error {
                    self.error = Some(error);
                }
                if terminal {
                    self.stopping = false;
                    self.end_native_stream();
                    if let Some(result) = run.result {
                        if let Some(last) = self
                            .current_response
                            .and_then(|index| self.messages.get_mut(index))
                        {
                            last.content = result;
                        } else {
                            self.messages.push(ChatMessage {
                                role: "system".into(),
                                content: format!("Result for run {id}"),
                                timestamp: format_timestamp(),
                            });
                            self.messages.push(ChatMessage {
                                role: "assistant".into(),
                                content: result,
                                timestamp: format_timestamp(),
                            });
                            self.current_response = self.messages.len().checked_sub(1);
                        }
                    }
                    return Cmd::unsubscribe("agent-stream");
                }
                Cmd::none()
            }
        }
    }

    pub(crate) fn stop_native_run(&mut self) -> Cmd<Msg> {
        if self.current_run.is_none() || !self.run_unresolved {
            return Cmd::ShowToast(Toast::warning("No known run to stop."));
        }
        self.stopping = true;
        self.status.step = "stop requested; awaiting daemon".into();
        if let Some(sender) = &self.stream_control {
            let _ = sender.try_send(Control::Cancel);
        }
        self.inspect_run(true)
    }

    pub(crate) fn decide_approval(&mut self, approved: bool) -> Cmd<Msg> {
        if self.approval_in_flight {
            return Cmd::none();
        }
        let Some(request) = self.pending_approval.as_ref() else {
            return Cmd::none();
        };
        if self.approval_is_stream {
            let sent = self.stream_control.as_ref().is_some_and(|sender| {
                sender
                    .try_send(Control::Approval {
                        id: request.id.clone(),
                        approved,
                    })
                    .is_ok()
            });
            if sent {
                self.approval_in_flight = true;
                return Cmd::none();
            }
            return Cmd::ShowToast(Toast::error(
                "Approval channel is unavailable. Inspect the run.",
            ));
        }
        let Some(connection) = self.connection.clone() else {
            return Cmd::none();
        };
        let id = request.id.clone();
        self.approval_in_flight = true;
        Cmd::spawn(async move {
            let decision = if approved {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Deny
            };
            let result = approval_client::submit_decision(
                &connection.client,
                &connection.base_url,
                &id,
                decision,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
            Msg::ApprovalSubmitted(id, result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Connection;
    use serde_json::json;

    fn active() -> PorygonTui {
        let mut app = PorygonTui::new();
        app.connection =
            Some(Connection::from_values("http://127.0.0.1:18789", "fixture-token").unwrap());
        app.connected = true;
        app.current_run = Some("run-a".into());
        app.run_unresolved = true;
        app.waiting_response = true;
        app
    }

    fn snapshot(status: &str) -> RunSnapshot {
        RunSnapshot {
            id: "run-a".into(),
            status: status.into(),
            result: None,
            error: None,
            metadata: json!({"conversation_id":"conv-a"}),
        }
    }

    fn approval(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.into(),
            tool_name: "shell".into(),
            params: json!({"command":"true"}),
            risk_score: -1.0,
            reasoning: "fixture".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        }
    }

    #[test]
    fn disconnected_send_preserves_draft() {
        let mut app = PorygonTui::new();
        app.connected = false;
        app.input.set_value("keep this draft");
        let before = app.messages.len();
        app.start_native_turn();
        assert_eq!(app.input.current_value(), "keep this draft");
        assert_eq!(app.messages.len(), before);
    }

    #[test]
    fn connection_failure_restores_draft_and_releases_unsent_run() {
        let mut app = active();
        app.native_event(
            "run-a".into(),
            Event::NotSubmitted {
                message: "unsent".into(),
                error: "connection failed".into(),
            },
        );
        assert_eq!(app.input.current_value(), "unsent");
        assert!(!app.run_unresolved);
        assert!(!app.waiting_response);
        assert!(app.current_run.is_none());
    }

    #[test]
    fn done_after_error_is_not_success_and_missing_readback_stays_uncertain() {
        let mut app = active();
        app.native_event("run-a".into(), Event::Error("provider failed".into()));
        app.native_event("run-a".into(), Event::Done(Some("conv-a".into())));
        assert!(app.run_unresolved);
        assert!(!app.waiting_response);
        assert_eq!(app.error.as_deref(), Some("provider failed"));
        app.observe_run("run-a".into(), Err("HTTP 404".into()));
        assert!(app.run_unresolved);
        assert!(app.status.step.contains("unconfirmed"));
    }

    #[test]
    fn stop_requires_daemon_disposition_and_preserves_draft() {
        let mut app = active();
        app.input.set_value("next thought");
        app.stop_native_run();
        assert!(app.waiting_response);
        assert!(app.stopping);
        assert!(app.status_step_text().contains("requested"));
        app.observe_run("run-a".into(), Ok(snapshot("cancelled")));
        assert!(!app.waiting_response);
        assert!(!app.run_unresolved);
        assert!(!app.stopping);
        assert_eq!(app.input.current_value(), "next thought");
    }

    #[test]
    fn attached_result_does_not_replace_unrelated_answer() {
        let mut app = active();
        app.waiting_response = false;
        app.messages.push(ChatMessage {
            role: "assistant".into(),
            content: "other run".into(),
            timestamp: "now".into(),
        });
        let other = app.messages.len() - 1;
        let mut run = snapshot("completed");
        run.result = Some("attached result".into());
        app.observe_run("run-a".into(), Ok(run));
        assert_eq!(app.messages[other].content, "other run");
        assert_eq!(app.messages.last().unwrap().content, "attached result");
    }

    #[test]
    fn clearing_transcript_invalidates_response_position() {
        let mut app = active();
        app.current_response = Some(1);
        app.input.set_value("/clear");
        app.handle_update(Msg::SendMessage);
        app.messages.push(ChatMessage {
            role: "assistant".into(),
            content: "unrelated".into(),
            timestamp: "now".into(),
        });
        let mut run = snapshot("completed");
        run.result = Some("run result".into());
        app.observe_run("run-a".into(), Ok(run));
        assert_eq!(app.messages[1].content, "unrelated");
        assert_eq!(app.messages.last().unwrap().content, "run result");
    }

    #[test]
    fn parked_reply_is_explicit_new_turn_on_same_run() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        app.input.set_value("my answer");
        let command = app.start_native_turn();
        assert!(!matches!(command, Cmd::None));
        assert_eq!(app.current_run.as_deref(), Some("run-a"));
        assert_eq!(app.conversation_id.as_deref(), Some("conv-a"));
        assert!(app.waiting_response);
        assert!(!app.run_can_continue);
    }

    #[test]
    fn uncertain_durable_decision_cannot_be_posted_twice() {
        let mut app = active();
        app.waiting_response = false;
        app.handle_update(Msg::ApprovalReceived(approval("a1"), false));
        app.decide_approval(true);
        app.handle_update(Msg::ApprovalSubmitted(
            "a1".into(),
            Err("response lost".into()),
        ));
        assert!(app.approval_in_flight);
        app.input.set_value("/attach other");
        app.handle_update(Msg::SendMessage);
        assert_eq!(app.current_run.as_deref(), Some("run-a"));
        assert_eq!(app.pending_approval.as_ref().unwrap().id, "a1");
        assert!(matches!(app.decide_approval(true), Cmd::None));
        app.handle_update(Msg::ApprovalReconciled("a1".into(), Ok(true)));
        assert!(app.approval_in_flight);
        app.handle_update(Msg::ApprovalReconciled("a1".into(), Ok(false)));
        assert!(app.pending_approval.is_none());
        assert!(!app.show_approval_modal);
    }

    #[test]
    fn concurrent_stream_decisions_keep_their_ids() {
        let mut app = active();
        app.handle_update(Msg::ApprovalReceived(approval("a1"), true));
        app.handle_update(Msg::ApprovalReceived(approval("a2"), true));
        app.native_event("run-a".into(), Event::DecisionSent("a1".into()));
        assert_eq!(app.pending_approval.as_ref().unwrap().id, "a2");
        assert!(app.approval_is_stream);
        assert!(!app.approval_in_flight);
    }

    #[test]
    fn unsent_parked_reply_keeps_run_available() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        app.input.set_value("reply");
        app.start_native_turn();
        app.native_event(
            "run-a".into(),
            Event::NotSubmitted {
                message: "reply".into(),
                error: "offline".into(),
            },
        );
        assert_eq!(app.current_run.as_deref(), Some("run-a"));
        assert!(app.run_can_continue);
        assert!(app.run_unresolved);
        assert_eq!(app.input.current_value(), "reply");
    }

    #[test]
    fn parked_reply_waits_for_stop_outcome() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        app.stop_native_run();
        app.input.set_value("reply");
        app.start_native_turn();
        assert!(!app.waiting_response);
        assert_eq!(app.input.current_value(), "reply");
    }

    #[test]
    fn old_parked_read_cannot_reopen_a_later_turn() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        let old_epoch = app.run_epoch;
        app.input.set_value("reply");
        app.start_native_turn();
        app.handle_update(Msg::NativeEvent(
            "run-a".into(),
            old_epoch,
            Event::Disconnected,
        ));
        assert!(app.waiting_response);
        app.native_event("run-a".into(), Event::Done(None));
        app.handle_update(Msg::RunObserved(
            "run-a".into(),
            old_epoch,
            Ok(snapshot("awaiting_input")),
        ));
        assert!(!app.run_can_continue);
        app.handle_update(Msg::RunObserved(
            "run-a".into(),
            app.run_epoch,
            Ok(snapshot("awaiting_input")),
        ));
        assert!(app.run_can_continue);
    }

    #[test]
    fn late_read_does_not_reverse_terminal_confirmation() {
        let mut app = active();
        app.observe_run("run-a".into(), Ok(snapshot("cancelled")));
        app.observe_run("run-a".into(), Ok(snapshot("executing")));
        app.observe_run("run-a".into(), Err("read timeout".into()));
        assert_eq!(app.status.step, "run cancelled");
        assert!(!app.run_unresolved);
        assert!(!app.stopping);
        app.stop_native_run();
        assert!(!app.stopping);
    }
}
