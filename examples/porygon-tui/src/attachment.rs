//! Stage a read-only run/history fetch before changing the active conversation.

use ulttui::prelude::Cmd;
use ulttui::toast::Toast;

use crate::connection::RunSnapshot;
use crate::conversations;
use crate::types::ChatMessage;
use crate::work::Page;
use crate::{Msg, PorygonTui};

pub(crate) struct Request {
    generation: u64,
    id: String,
    conversation_id: Option<String>,
    run_id: Option<String>,
    run_epoch: u64,
    page: Page,
    detail_id: Option<String>,
    picker_open: bool,
    help_open: bool,
}

#[derive(Debug)]
pub(crate) struct Snapshot {
    run: RunSnapshot,
    conversation_id: String,
    messages: Vec<ChatMessage>,
}

impl PorygonTui {
    pub(crate) fn begin_attach(&mut self, id: String) -> Cmd<Msg> {
        if !self.can_switch() {
            return Cmd::ShowToast(Toast::warning(
                "Resolve current work before attaching another run. Use /reconnect to inspect it.",
            ));
        }
        let Some(connection) = self.connection.clone() else {
            return Cmd::ShowToast(Toast::warning("Daemon connection is not configured."));
        };
        self.attach_generation = self.attach_generation.wrapping_add(1);
        let generation = self.attach_generation;
        self.pending_attach = Some(Request {
            generation,
            id: id.clone(),
            conversation_id: self.conversation_id.clone(),
            run_id: self.current_run.clone(),
            run_epoch: self.run_epoch,
            page: self.work.page,
            detail_id: self.work.detail_id.clone(),
            picker_open: self.conversations.open,
            help_open: self.show_help,
        });
        Cmd::spawn(async move {
            let result = async {
                let run = connection.fetch_run(&id).await?;
                let conversation_id = run.metadata["conversation_id"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or(
                        "Run has no known conversation. Inspect it in Work; nothing was attached.",
                    )?
                    .to_owned();
                let messages = conversations::fetch_history(&connection, &conversation_id).await?;
                Ok(Snapshot {
                    run,
                    conversation_id,
                    messages,
                })
            }
            .await;
            Msg::AttachReady(generation, result)
        })
    }

    pub(crate) fn finish_attach(
        &mut self,
        generation: u64,
        result: Result<Snapshot, String>,
    ) -> Cmd<Msg> {
        if self
            .pending_attach
            .as_ref()
            .is_none_or(|request| request.generation != generation)
        {
            return Cmd::none();
        }
        let request = self
            .pending_attach
            .take()
            .expect("matching attach request exists");
        if !self.can_switch()
            || self.conversation_id != request.conversation_id
            || self.current_run != request.run_id
            || self.run_epoch != request.run_epoch
            || self.work.page != request.page
            || self.work.detail_id != request.detail_id
            || self.conversations.open != request.picker_open
            || self.show_help != request.help_open
        {
            return Cmd::none();
        }
        match result {
            Err(error) => Cmd::ShowToast(Toast::error(format!(
                "Could not attach run: {error}. Previous conversation preserved."
            ))),
            Ok(snapshot) => {
                if snapshot.run.id != request.id
                    || snapshot.run.metadata["conversation_id"].as_str()
                        != Some(&snapshot.conversation_id)
                {
                    return Cmd::ShowToast(Toast::error(
                        "Run identity changed; previous conversation preserved.",
                    ));
                }
                self.select_loaded_conversation(snapshot.conversation_id, snapshot.messages);
                self.current_run = Some(snapshot.run.id.clone());
                self.run_epoch = self.run_epoch.wrapping_add(1);
                self.run_unresolved = true;
                self.run_can_continue = false;
                if let Some(result) = snapshot.run.result.as_deref() {
                    self.current_response = self.messages.iter().rposition(|message| {
                        message.role == "assistant" && message.content == result
                    });
                }
                let command = self.observe_run(snapshot.run.id.clone(), Ok(snapshot.run));
                self.work.page = Page::Closed;
                self.flush_drafts();
                command
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Connection;
    use serde_json::json;

    fn message(content: &str) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: content.into(),
            timestamp: "now".into(),
        }
    }

    fn app() -> PorygonTui {
        let mut app = PorygonTui::new();
        app.connection =
            Some(Connection::from_values("http://127.0.0.1:18789", "fixture").unwrap());
        app.select_loaded_conversation("B".into(), vec![message("B history")]);
        app.input.set_value("B-only draft");
        app.select_loaded_conversation("A".into(), vec![message("A history")]);
        app.input.set_value("A-only draft");
        app
    }

    fn snapshot(id: &str, status: &str) -> Snapshot {
        Snapshot {
            run: RunSnapshot {
                id: id.into(),
                status: status.into(),
                result: None,
                error: None,
                metadata: json!({"conversation_id":"B"}),
            },
            conversation_id: "B".into(),
            messages: vec![message("B history")],
        }
    }

    #[test]
    fn test_success_restores_target_history_draft_and_waiting_run_without_submission() {
        let mut app = app();
        app.begin_attach("run-B".into());
        assert_eq!(app.conversation_id.as_deref(), Some("A"));
        assert_eq!(app.input.current_value(), "A-only draft");
        app.finish_attach(
            app.attach_generation,
            Ok(snapshot("run-B", "awaiting_input")),
        );
        assert_eq!(app.conversation_id.as_deref(), Some("B"));
        assert_eq!(app.input.current_value(), "B-only draft");
        assert_eq!(app.messages[0].content, "B history");
        assert_eq!(app.current_run.as_deref(), Some("run-B"));
        assert!(app.run_unresolved);
        assert!(
            !app.run_can_continue,
            "pending actions must be inspected before replying"
        );
        app.observe_run_approvals(
            "run-B".into(),
            app.run_epoch,
            app.approval_check_generation,
            Ok(crate::approval_client::PendingApprovals {
                count: 0,
                limit: 200,
                updated_at: None,
                items: Vec::new(),
            }),
        );
        assert!(app.run_can_continue);
        assert!(!app.waiting_response);
        app.run_unresolved = false;
        app.select_loaded_conversation("A".into(), vec![message("A history")]);
        assert_eq!(app.input.current_value(), "A-only draft");
    }

    #[test]
    fn test_fetch_failure_and_stale_generation_preserve_original_identity_and_draft() {
        let mut app = app();
        app.begin_attach("run-B".into());
        let stale = app.attach_generation;
        app.begin_attach("run-C".into());
        app.finish_attach(stale, Ok(snapshot("run-B", "completed")));
        assert_eq!(app.conversation_id.as_deref(), Some("A"));
        assert!(app.pending_attach.is_some());
        app.finish_attach(app.attach_generation, Err("history failed".into()));
        assert_eq!(app.conversation_id.as_deref(), Some("A"));
        assert_eq!(app.input.current_value(), "A-only draft");
        assert_eq!(app.messages[0].content, "A history");
        assert!(app.current_run.is_none());
    }

    #[test]
    fn test_busy_state_and_navigation_invalidate_pending_attachment() {
        let mut app = app();
        app.begin_attach("run-B".into());
        app.stopping = true;
        app.finish_attach(app.attach_generation, Ok(snapshot("run-B", "completed")));
        assert_eq!(app.conversation_id.as_deref(), Some("A"));
        assert!(app.pending_attach.is_none());
        app.begin_attach("run-B".into());
        assert!(app.pending_attach.is_none());
        app.stopping = false;
        app.run_unresolved = true;
        app.begin_attach("run-B".into());
        assert!(app.pending_attach.is_none());
        app.run_unresolved = false;
        app.begin_attach("run-B".into());
        app.show_help = true;
        app.finish_attach(app.attach_generation, Ok(snapshot("run-B", "completed")));
        assert_eq!(app.conversation_id.as_deref(), Some("A"));
        assert_eq!(app.input.current_value(), "A-only draft");
    }
}
