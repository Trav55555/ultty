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
        self.approval_execution_pending = false;
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
            scope: None, decision_version: None, decision_stale: false,
                    id, tool_name: tool, params: arguments, risk_score: -1.0,
                    reasoning: format!("Run {run_id}. Approve this action once, or deny. Scope and reversibility are not supplied by this stream."),
                    blast_radius: "unknown".into(), reversible: None,
                }, true))
            }
            Event::DurableApproval { id, decision_version, context } => {
                if context["run_id"].as_str() != Some(&run_id) {
                    self.error = Some("Approval belongs to another run; no decision was enabled.".into());
                    return Cmd::ShowToast(Toast::error("Approval run identity does not match."));
                }
                let tool_name = context["tool_name"].as_str().unwrap_or_default().to_owned();
                if let Some(index) = context["tool_call_id"].as_str().and_then(|call_id| {
                    self.tool_ids.iter().rposition(|id| id == call_id)
                }) {
                    self.tool_calls[index].status = ToolStatus::Pending;
                }
                let reasoning = if context["reason"] == "missing_policy_binding" {
                    "Execution is not enabled. Approve this exact action once, or deny it. This does not enable future commands."
                } else {
                    "Approve the exact action shown once, or deny it. Execution has not happened yet."
                };
                self.handle_update(Msg::ApprovalReceived(ApprovalRequest {
                    scope: Some("tool_call".into()), decision_version: Some(decision_version), decision_stale: false,
                    id, tool_name, params: context, risk_score: -1.0,
                    reasoning: reasoning.into(), blast_radius: "unknown".into(), reversible: None,
                }, false))
            }
            Event::DecisionSent(id) => {
                if self.pending_approval.as_ref().is_some_and(|r| r.id == id) {
                    self.show_approval_modal = false;
                    self.pending_approval = None;
                    self.approval_in_flight = false;
                    self.approval_is_stream = false;
                    self.status.step = "decision sent; awaiting daemon outcome".into();
                    if let Some((next, is_stream)) = self.queued_stream_approvals.pop_front() {
                        return self.handle_update(Msg::ApprovalReceived(next, is_stream));
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
                let resolves_pending_action = self.pending_approval.as_ref().is_some_and(|request| {
                    request.params["tool_call_id"].as_str() == Some(&id)
                });
                if let Some(index) = self.tool_ids.iter().rposition(|current| current == &id) {
                    let was_approved = matches!(self.tool_calls[index].status, ToolStatus::Approved);
                    self.tool_calls[index].status = if success {
                        ToolStatus::Completed
                    } else {
                        ToolStatus::Failed
                    };
                    self.tool_calls[index].result = Some(result);
                    if was_approved || resolves_pending_action {
                        self.approval_execution_pending = self.tool_calls.iter().any(|tool| {
                            matches!(tool.status, ToolStatus::Approved)
                        });
                    }
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
        self.approval_check_generation = self.approval_check_generation.wrapping_add(1);
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
                self.run_can_continue = false;
                self.status.step = match run.status.as_str() {
                    "completed" if run.error.is_some() => "turn ended with error".into(),
                    "completed" => "turn ended".into(),
                    "paused_needs_review" | "insufficient_oracle" => {
                        "blocked; inspect Attention (F4)".into()
                    }
                    "awaiting_input" => "checking pending actions before continuing".into(),
                    _ => format!("run {}", run.status),
                };
                if let Some(error) = run.error {
                    self.error = Some(error);
                }
                if terminal {
                    self.approval_execution_pending = false;
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
                if run.status == "awaiting_input" {
                    return self.inspect_run_approvals(id);
                }
                Cmd::none()
            }
        }
    }

    fn inspect_run_approvals(&self, id: String) -> Cmd<Msg> {
        let Some(connection) = self.connection.clone() else {
            return Cmd::none();
        };
        let epoch = self.run_epoch;
        let generation = self.approval_check_generation;
        Cmd::spawn(async move {
            let result =
                approval_client::fetch_pending(&connection.client, &connection.base_url, 200)
                    .await
                    .map_err(|error| error.to_string());
            Msg::RunApprovalsObserved(id, epoch, generation, result)
        })
    }

    pub(crate) fn observe_run_approvals(
        &mut self,
        id: String,
        epoch: u64,
        generation: u64,
        result: Result<approval_client::PendingApprovals, String>,
    ) -> Cmd<Msg> {
        if self.current_run.as_deref() != Some(&id)
            || epoch != self.run_epoch
            || generation != self.approval_check_generation
            || !self.run_unresolved
            || self.waiting_response
        {
            return Cmd::none();
        }
        let pending = match result {
            Ok(pending) => pending,
            Err(error) => {
                self.error = Some(format!("Could not inspect pending actions: {error}. Use /reconnect; your draft is preserved."));
                self.status.step = "pending actions unconfirmed; /reconnect".into();
                return Cmd::none();
            }
        };
        if let Some(item) = pending
            .items
            .iter()
            .find(|item| item.context.run_id.as_deref() == Some(&id))
        {
            self.status.step = "blocked; approval required (F4)".into();
            self.run_can_continue = false;
            return self.inspect_durable_approval(item);
        }
        if pending.count > pending.items.len() as u64 {
            self.status.step = "pending actions list incomplete; inspect Attention (F4)".into();
        } else if self.approval_execution_pending || self.pending_approval.is_some() {
            self.status.step = "decision accepted; awaiting run outcome".into();
        } else {
            self.run_can_continue = true;
            self.status.step = "waiting for your reply".into();
        }
        Cmd::none()
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
        if let Some(reason) = self.approval_decision_blocker() {
            return Cmd::ShowToast(Toast::warning(reason));
        }
        let Some(connection) = self.connection.clone() else {
            return Cmd::none();
        };
        let id = request.id.clone();
        let version = request
            .decision_version
            .clone()
            .expect("durable decision blocker requires a version");
        self.approval_in_flight = true;
        let decision = if approved {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        };
        self.pending_approval_decision = Some(decision);
        self.approval_execution_pending = self.approval_execution_pending
            || approved
            || self
                .tool_calls
                .iter()
                .any(|tool| matches!(tool.status, ToolStatus::Approved));
        Cmd::spawn(async move {
            let result = approval_client::submit_decision(
                &connection.client,
                &connection.base_url,
                &id,
                &version,
                decision,
            )
            .await;
            match result {
                Err(error) if error.is::<approval_client::StaleApproval>() => {
                    Msg::ApprovalBecameStale(id, version, error.to_string())
                }
                result => Msg::ApprovalSubmitted(
                    id,
                    version,
                    result.map(|_| ()).map_err(|error| error.to_string()),
                ),
            }
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

    fn confirm_no_pending_actions(app: &mut PorygonTui) {
        app.observe_run_approvals(
            "run-a".into(),
            app.run_epoch,
            app.approval_check_generation,
            Ok(approval_client::PendingApprovals {
                count: 0,
                limit: 200,
                updated_at: None,
                items: Vec::new(),
            }),
        );
    }

    fn approval(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("fixture-version".into()),
            decision_stale: false,
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
        confirm_no_pending_actions(&mut app);
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
            "fixture-version".into(),
            Err("response lost".into()),
        ));
        assert!(app.approval_in_flight);
        app.input.set_value("/attach other");
        app.handle_update(Msg::SendMessage);
        assert_eq!(app.current_run.as_deref(), Some("run-a"));
        assert_eq!(app.pending_approval.as_ref().unwrap().id, "a1");
        assert!(matches!(app.decide_approval(true), Cmd::None));
        app.handle_update(Msg::ApprovalReconciled(
            "a1".into(),
            Some("fixture-version".into()),
            Ok(approval_client::ApprovalRefresh::Same),
        ));
        assert!(app.approval_in_flight);
        app.handle_update(Msg::ApprovalReconciled(
            "a1".into(),
            Some("fixture-version".into()),
            Ok(approval_client::ApprovalRefresh::Missing),
        ));
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
        confirm_no_pending_actions(&mut app);
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
        confirm_no_pending_actions(&mut app);
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
        confirm_no_pending_actions(&mut app);
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
        confirm_no_pending_actions(&mut app);
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

    #[test]
    fn test_completed_readback_reports_turn_end_without_claiming_task_success() {
        let mut app = active();
        app.observe_run("run-a".into(), Ok(snapshot("completed")));
        assert_eq!(app.status.step, "turn ended");
        assert!(!app.run_unresolved);
    }

    #[test]
    fn test_paused_readback_reports_actionable_blocked_status() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("paused_needs_review")));
        assert_eq!(app.status.step, "blocked; inspect Attention (F4)");
        assert!(app.run_unresolved);
        assert!(!app.run_can_continue);
    }

    fn durable_context(run_id: &str) -> serde_json::Value {
        json!({
            "tool_name": "run_argv", "params": {"argv": ["git", "status", "--short"]},
            "run_id": run_id, "principal_id": "owner", "approval_expires_at": "2099-01-01T00:00:00Z",
            "reason": "missing_policy_binding", "tool_call_id": "call-a",
        })
    }

    #[test]
    fn test_live_capability_request_preserves_stream_and_exact_action() {
        let mut app = active();
        let (sender, _receiver) = mpsc::channel(8);
        app.stream_control = Some(sender);
        app.input.set_value("keep next thought");
        app.native_event(
            "run-a".into(),
            Event::ToolCall {
                id: "call-a".into(),
                name: "run_argv".into(),
                arguments: json!({"argv":["git","status","--short"]}),
            },
        );
        app.native_event(
            "run-a".into(),
            Event::DurableApproval {
                id: "approval-a".into(),
                decision_version: "version-a".into(),
                context: durable_context("run-a"),
            },
        );
        let request = app
            .pending_approval
            .as_ref()
            .expect("durable request is actionable");
        assert_eq!(request.decision_version.as_deref(), Some("version-a"));
        assert_eq!(
            request.params["params"]["argv"],
            json!(["git", "status", "--short"])
        );
        assert!(request.reasoning.contains("exact action once"));
        assert!(!app.approval_is_stream, "versioned decisions use HTTP");
        assert!(app.show_approval_modal);
        assert!(app.waiting_response);
        assert!(app.stream_control.is_some());
        assert!(matches!(app.tool_calls[0].status, ToolStatus::Pending));
        app.decide_approval(true);
        app.handle_update(Msg::ApprovalSubmitted(
            "approval-a".into(),
            "version-a".into(),
            Ok(()),
        ));
        assert!(app.pending_approval.is_none());
        assert!(
            app.waiting_response,
            "accepted decision does not finish execution"
        );
        assert!(app.approval_execution_pending);
        assert!(!app.run_can_continue);
        assert_eq!(app.input.current_value(), "keep next thought");
        assert!(app.status.step.contains("awaiting run outcome"));
        assert!(matches!(app.tool_calls[0].status, ToolStatus::Approved));
        assert!(app.tool_calls[0].result.is_none());
        app.native_event(
            "run-a".into(),
            Event::ToolResult {
                id: "call-a".into(),
                result: "clean".into(),
                success: true,
            },
        );
        assert!(matches!(app.tool_calls[0].status, ToolStatus::Completed));
        assert!(!app.approval_execution_pending);
        assert_eq!(app.input.current_value(), "keep next thought");
    }

    #[test]
    fn test_live_capability_request_rejects_other_run_identity() {
        let mut app = active();
        app.native_event(
            "run-a".into(),
            Event::DurableApproval {
                id: "approval-b".into(),
                decision_version: "version-b".into(),
                context: durable_context("run-b"),
            },
        );
        assert!(app.pending_approval.is_none());
        assert!(app
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("another run"));
    }

    #[test]
    fn test_empty_approval_queue_does_not_reopen_turn_after_authorization() {
        let mut app = active();
        app.waiting_response = false;
        app.approval_execution_pending = true;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        confirm_no_pending_actions(&mut app);
        assert!(!app.run_can_continue);
        assert!(app.run_unresolved);
        assert!(app.status.step.contains("awaiting run outcome"));
    }

    #[test]
    fn test_pending_queue_failure_preserves_draft_and_blocks_continuation() {
        let mut app = active();
        app.waiting_response = false;
        app.input.set_value("draft");
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        app.observe_run_approvals(
            "run-a".into(),
            app.run_epoch,
            app.approval_check_generation,
            Err("HTTP 500".into()),
        );
        assert!(!app.run_can_continue);
        assert_eq!(app.input.current_value(), "draft");
        assert!(app.status.step.contains("unconfirmed"));
    }

    #[test]
    fn test_parked_run_opens_only_its_bound_pending_action() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        let item = |id: &str, run_id: &str| {
            serde_json::from_value(json!({
            "id":id,"decision_version":"version-a","scope":"tool_call","source":"tool:run_argv",
            "objective":"inspect repository","priority":"normal","context":durable_context(run_id),"created_at":"now",
        })).unwrap()
        };
        app.observe_run_approvals(
            "run-a".into(),
            app.run_epoch,
            app.approval_check_generation,
            Ok(approval_client::PendingApprovals {
                count: 2,
                limit: 200,
                updated_at: None,
                items: vec![item("other", "run-b"), item("ours", "run-a")],
            }),
        );
        assert_eq!(app.pending_approval.as_ref().unwrap().id, "ours");
        assert!(app.show_approval_modal);
        assert!(!app.run_can_continue);
        assert!(app.status.step.contains("approval required"));
    }

    #[test]
    fn test_incomplete_or_stale_pending_read_does_not_enable_reply() {
        let mut app = active();
        app.waiting_response = false;
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        let generation = app.approval_check_generation;
        app.observe_run_approvals(
            "run-a".into(),
            app.run_epoch,
            generation,
            Ok(approval_client::PendingApprovals {
                count: 201,
                limit: 200,
                updated_at: None,
                items: Vec::new(),
            }),
        );
        assert!(!app.run_can_continue);
        assert!(app.status.step.contains("incomplete"));
        app.observe_run("run-a".into(), Ok(snapshot("executing")));
        app.observe_run_approvals(
            "run-a".into(),
            app.run_epoch,
            generation,
            Ok(approval_client::PendingApprovals {
                count: 0,
                limit: 200,
                updated_at: None,
                items: Vec::new(),
            }),
        );
        assert!(!app.run_can_continue);
        assert_eq!(app.status.step, "run executing");
    }

    #[test]
    fn test_tool_result_before_http_confirmation_stays_authoritative() {
        let mut app = active();
        app.native_event(
            "run-a".into(),
            Event::ToolCall {
                id: "call-a".into(),
                name: "run_argv".into(),
                arguments: json!({"argv":["git","status"]}),
            },
        );
        app.native_event(
            "run-a".into(),
            Event::DurableApproval {
                id: "approval-a".into(),
                decision_version: "version-a".into(),
                context: durable_context("run-a"),
            },
        );
        app.decide_approval(true);
        app.native_event(
            "run-a".into(),
            Event::ToolResult {
                id: "call-a".into(),
                result: "failed".into(),
                success: false,
            },
        );
        app.handle_update(Msg::ApprovalSubmitted(
            "approval-a".into(),
            "version-a".into(),
            Ok(()),
        ));
        assert!(matches!(app.tool_calls[0].status, ToolStatus::Failed));
        assert!(!app.approval_execution_pending);
        assert!(app.waiting_response);
    }

    #[test]
    fn test_terminal_run_before_decision_confirmation_keeps_terminal_status() {
        let mut app = active();
        app.native_event(
            "run-a".into(),
            Event::DurableApproval {
                id: "approval-a".into(),
                decision_version: "version-a".into(),
                context: durable_context("run-a"),
            },
        );
        app.decide_approval(true);
        app.observe_run("run-a".into(), Ok(snapshot("completed")));
        app.handle_update(Msg::ApprovalSubmitted(
            "approval-a".into(),
            "version-a".into(),
            Ok(()),
        ));
        assert_eq!(app.status.step, "turn ended");
        assert!(!app.run_unresolved);
        assert!(!app.approval_execution_pending);
    }

    #[test]
    fn test_queued_durable_request_keeps_http_transport() {
        let mut app = active();
        app.native_event(
            "run-a".into(),
            Event::Approval {
                id: "legacy".into(),
                tool: "shell".into(),
                arguments: json!({}),
            },
        );
        app.native_event(
            "run-a".into(),
            Event::DurableApproval {
                id: "durable".into(),
                decision_version: "version-a".into(),
                context: durable_context("run-a"),
            },
        );
        app.native_event("run-a".into(), Event::DecisionSent("legacy".into()));
        assert_eq!(app.pending_approval.as_ref().unwrap().id, "durable");
        assert!(!app.approval_is_stream);
        assert!(app.show_approval_modal);
    }

    #[test]
    fn test_later_denial_preserves_earlier_unobserved_approved_execution() {
        let mut app = active();
        app.waiting_response = false;
        app.input.set_value("keep next thought");
        for (id, approved) in [("approval-a", true), ("approval-b", false)] {
            let mut request = approval(id);
            request.params = durable_context("run-a");
            app.handle_update(Msg::ApprovalReceived(request, false));
            app.decide_approval(approved);
            app.handle_update(Msg::ApprovalSubmitted(
                id.into(),
                "fixture-version".into(),
                Ok(()),
            ));
        }
        assert!(
            app.tool_calls.is_empty(),
            "attached approvals have no local tool rows"
        );
        assert!(
            app.approval_execution_pending,
            "denying B cannot finish accepted A"
        );
        app.observe_run("run-a".into(), Ok(snapshot("awaiting_input")));
        confirm_no_pending_actions(&mut app);
        assert!(
            !app.run_can_continue,
            "an empty queue cannot prove A finished"
        );
        assert_eq!(app.input.current_value(), "keep next thought");
        app.observe_run("run-a".into(), Ok(snapshot("completed")));
        assert!(!app.approval_execution_pending);
        assert!(!app.run_unresolved);
    }
}
