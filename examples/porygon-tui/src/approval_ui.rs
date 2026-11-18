//! The action shown for inspection keeps its server-issued decision binding.

use ulttui::prelude::Cmd;
use ulttui::toast::Toast;

use crate::approval_client::PendingApproval;
use crate::types::ApprovalRequest;
use crate::{Msg, PorygonTui};

/// Shown whenever the daemon says the approved command leaves the sandbox.
pub(crate) const HOST_EXECUTION_LINE: &str = "Runs on your machine (outside the project sandbox)";

/// Shown when the daemon says the command runs in the disposable project sandbox.
pub(crate) const CONTAINED_EXECUTION_LINE: &str =
    "Runs sandboxed (a disposable copy of the project, no network)";

/// The menu's choices, worded like common terminal-agent approval menus.
pub(crate) const APPROVE_ONCE_LABEL: &str = "Yes";
pub(crate) const APPROVE_SESSION_LABEL: &str = "Yes, and don't ask again for this command";
pub(crate) const REJECT_FEEDBACK_LABEL: &str = "No, and tell Porygon what to do differently";
pub(crate) const REJECT_LABEL: &str = "No";

/// Whether the daemon marked this request as contained (sandboxed) execution.
/// Older daemons never send it, and a host request is never contained.
pub(crate) fn runs_contained(request: &ApprovalRequest) -> bool {
    let context = &request.params;
    !runs_on_host(request)
        && (context.get("execution").and_then(serde_json::Value::as_str) == Some("contained")
            || context.get("reason").and_then(serde_json::Value::as_str)
                == Some("contained_project_work"))
}

/// Whether the daemon marked this request as host execution. Older daemons
/// omit these fields, so their requests render as before. A refreshed pending
/// record carries the stored reason and arguments rather than `execution`.
pub(crate) fn runs_on_host(request: &ApprovalRequest) -> bool {
    let context = &request.params;
    context.get("execution").and_then(serde_json::Value::as_str) == Some("host")
        || context.get("reason").and_then(serde_json::Value::as_str)
            == Some("host_execution_request")
        || context
            .get("params")
            .and_then(|params| params.get("host"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

/// The model's stated reason for host execution, when it gave one.
pub(crate) fn host_justification(request: &ApprovalRequest) -> Option<&str> {
    let context = &request.params;
    context
        .get("justification")
        .or_else(|| {
            context
                .get("params")
                .and_then(|params| params.get("justification"))
        })
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

impl PorygonTui {
    pub(crate) fn approval_session_available(&self) -> bool {
        if self.approval_is_stream || !self.approval_supports("session") {
            return false;
        }
        self.pending_approval.as_ref().is_some_and(|r| {
            let scope = &r.params["session_scope"];
            ["conversation_id", "project_root"].iter().all(|key| {
                scope
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty())
            }) && scope
                .get("argv")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|a| !a.is_empty() && a.iter().all(serde_json::Value::is_string))
                && scope.get("argv") == r.params.get("params").and_then(|p| p.get("argv"))
        })
    }

    pub(crate) fn approval_supports(&self, option: &str) -> bool {
        self.pending_approval
            .as_ref()
            .and_then(|r| r.params.get("approval_options"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| items.iter().any(|v| v.as_str() == Some(option)))
    }

    pub(crate) fn approval_decision_blocker(&self) -> Option<&'static str> {
        if self.approval_is_stream {
            return None;
        }
        let request = self.pending_approval.as_ref()?;
        if request.decision_stale {
            return Some("Changed approval: Esc, F4, r, Enter to reinspect.");
        }
        if request.scope.as_deref() != Some("tool_call") {
            return Some("Read-only approval: use its originating workflow.");
        }
        if request
            .decision_version
            .as_deref()
            .is_none_or(|version| version.trim().is_empty())
        {
            return Some("Read-only: Upgrade daemon, refresh and reinspect.");
        }
        None
    }

    pub(crate) fn notice_durable_approval(&mut self, item: &PendingApproval) {
        if self.approval_is_stream {
            return;
        }
        if let Some(request) = self
            .pending_approval
            .as_mut()
            .filter(|request| request.id == item.id)
        {
            if request.decision_version != item.decision_version || request.scope != item.scope {
                request.decision_stale = true;
            }
        }
    }

    pub(crate) fn inspect_durable_approval(&mut self, item: &PendingApproval) -> Cmd<Msg> {
        if let Some(request) = &self.pending_approval {
            if request.id != item.id || self.approval_is_stream {
                return Cmd::ShowToast(Toast::warning(
                    "Resolve or refresh the current approval first.",
                ));
            }
            if request.decision_version == item.decision_version && request.scope == item.scope {
                // An uncertain write on this exact incarnation stays locked. A
                // refresh or reopening its details must never authorize retry.
                self.show_approval_modal = true;
                return Cmd::none();
            }
        } else if self.approval_in_flight {
            return Cmd::ShowToast(Toast::warning("Decision is unconfirmed."));
        }
        // Only this explicit inspection adopts a new incarnation. Background
        // observations keep the original context and mark it stale instead.
        self.pending_approval = Some(ApprovalRequest {
            id: item.id.clone(),
            scope: item.scope.clone(),
            decision_version: item.decision_version.clone(),
            decision_stale: false,
            tool_name: item
                .context
                .tool_name
                .clone()
                .unwrap_or_else(|| item.source.clone()),
            params: serde_json::to_value(&item.context).unwrap_or_default(),
            risk_score: -1.0,
            reasoning: if item
                .context
                .additional
                .get("reason")
                .and_then(|value| value.as_str())
                == Some("missing_policy_binding")
            {
                format!("Execution is not enabled. Approve this exact action once, or deny it. This does not enable future commands. {}", item.objective)
            } else {
                item.objective.clone()
            },
            blast_radius: "unknown".into(),
            reversible: None,
        });
        self.approval_is_stream = false;
        self.approval_in_flight = false;
        self.approval_scroll = 0;
        self.approval_choice = 0;
        self.approval_details = false;
        self.approval_stage = 0;
        self.approval_feedback.set_value("");
        self.show_approval_modal = true;
        Cmd::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_client::ApprovalRefresh;
    use crate::connection::Connection;
    use serde_json::json;

    fn item(version: Option<&str>, command: &str) -> PendingApproval {
        serde_json::from_value(json!({
            "id":"same-id", "scope":"tool_call", "decision_version":version,
            "source":"tool:shell", "objective":command, "priority":"HIGH", "created_at":"now",
            "context":{"tool_name":"shell","params":{"command":command},"run_id":"run"}
        }))
        .unwrap()
    }

    fn app() -> PorygonTui {
        let mut app = PorygonTui::new();
        app.connection =
            Some(Connection::from_values("http://127.0.0.1:18789", "fixture").unwrap());
        app
    }

    #[test]
    fn test_same_id_replacement_keeps_inspected_context_until_explicit_reinspection() {
        let mut app = app();
        let old = item(Some("version-old"), "old action");
        let new = item(Some("version-new"), "new action");
        app.inspect_durable_approval(&old);
        app.notice_durable_approval(&new);
        let inspected = app.pending_approval.as_ref().unwrap();
        assert_eq!(inspected.decision_version.as_deref(), Some("version-old"));
        assert_eq!(inspected.params["params"]["command"], "old action");
        assert!(inspected.decision_stale);
        assert!(matches!(app.decide_approval(true), Cmd::ShowToast(_)));
        assert!(!app.approval_in_flight);
        app.inspect_durable_approval(&new);
        let inspected = app.pending_approval.as_ref().unwrap();
        assert_eq!(inspected.decision_version.as_deref(), Some("version-new"));
        assert_eq!(inspected.params["params"]["command"], "new action");
        assert!(!inspected.decision_stale);
    }

    #[test]
    fn test_late_old_incarnation_callbacks_cannot_clear_reinspected_replacement() {
        let mut app = app();
        app.inspect_durable_approval(&item(Some("old"), "old action"));
        app.decide_approval(true);
        app.notice_durable_approval(&item(Some("new"), "new action"));
        app.inspect_durable_approval(&item(Some("new"), "new action"));
        app.handle_update(Msg::ApprovalSubmitted(
            "same-id".into(),
            "old".into(),
            Ok(()),
        ));
        app.handle_update(Msg::ApprovalReconciled(
            "same-id".into(),
            Some("old".into()),
            Ok(ApprovalRefresh::Missing),
        ));
        assert_eq!(
            app.pending_approval
                .as_ref()
                .unwrap()
                .decision_version
                .as_deref(),
            Some("new")
        );
        assert!(app.show_approval_modal);
    }

    #[test]
    fn test_uncertain_same_incarnation_remains_locked_across_refresh_and_reinspection() {
        let mut app = app();
        let inspected = item(Some("bound"), "action");
        app.inspect_durable_approval(&inspected);
        app.decide_approval(true);
        app.handle_update(Msg::ApprovalSubmitted(
            "same-id".into(),
            "bound".into(),
            Err("Approval consumed; execution outcome unknown".into()),
        ));
        app.handle_update(Msg::ApprovalReconciled(
            "same-id".into(),
            Some("bound".into()),
            Ok(ApprovalRefresh::Same),
        ));
        app.inspect_durable_approval(&inspected);
        assert!(app.approval_in_flight);
        assert!(matches!(app.decide_approval(true), Cmd::None));
        assert!(matches!(app.decide_approval(false), Cmd::None));
    }

    #[test]
    fn test_unversioned_tools_and_heartbeats_are_read_only() {
        let mut app = app();
        app.inspect_durable_approval(&item(None, "legacy"));
        assert!(app.approval_decision_blocker().unwrap().contains("Upgrade"));
        assert!(matches!(app.decide_approval(true), Cmd::ShowToast(_)));
        assert!(matches!(app.decide_approval(false), Cmd::ShowToast(_)));
        assert!(!app.approval_in_flight);
        app.pending_approval = None;
        let mut heartbeat = item(Some("versioned-heartbeat"), "heartbeat");
        heartbeat.scope = None;
        app.inspect_durable_approval(&heartbeat);
        assert!(app
            .approval_decision_blocker()
            .unwrap()
            .contains("originating workflow"));
        assert!(matches!(app.decide_approval(true), Cmd::ShowToast(_)));
        assert!(!app.approval_in_flight);
    }

    #[test]
    fn test_refresh_and_same_id_notice_mark_stale_without_adopting_replacement() {
        let mut app = app();
        app.inspect_durable_approval(&item(Some("old"), "old action"));
        let mut notice = app.pending_approval.as_ref().unwrap().clone();
        notice.decision_version = Some("replacement".into());
        notice.reasoning = "different action".into();
        app.handle_update(Msg::ApprovalReceived(notice, false));
        app.handle_update(Msg::ApprovalReconciled(
            "same-id".into(),
            Some("old".into()),
            Ok(ApprovalRefresh::Changed),
        ));
        let inspected = app.pending_approval.as_ref().unwrap();
        assert_eq!(inspected.decision_version.as_deref(), Some("old"));
        assert_eq!(inspected.reasoning, "old action");
        assert!(inspected.decision_stale);
        assert!(matches!(app.decide_approval(true), Cmd::ShowToast(_)));
    }
    #[test]
    fn test_old_confirmed_decision_does_not_remove_new_same_id_queue_item() {
        let mut app = app();
        app.inspect_durable_approval(&item(Some("old"), "old action"));
        app.decide_approval(true);
        let replacement = item(Some("new"), "new action");
        app.notice_durable_approval(&replacement);
        app.work.approvals = vec![replacement];
        app.handle_update(Msg::ApprovalSubmitted(
            "same-id".into(),
            "old".into(),
            Ok(()),
        ));
        assert!(app.pending_approval.is_none());
        assert_eq!(app.work.approvals.len(), 1);
        assert_eq!(
            app.work.approvals[0].decision_version.as_deref(),
            Some("new")
        );
    }
    #[test]
    fn test_server_stale_response_blocks_old_binding_and_ignores_late_replacement_callback() {
        let mut app = app();
        app.inspect_durable_approval(&item(Some("old"), "old action"));
        app.decide_approval(true);
        app.handle_update(Msg::ApprovalBecameStale(
            "same-id".into(),
            "old".into(),
            "server stale".into(),
        ));
        assert!(app.pending_approval.as_ref().unwrap().decision_stale);
        assert!(matches!(app.decide_approval(true), Cmd::None));
        app.inspect_durable_approval(&item(Some("new"), "new action"));
        app.handle_update(Msg::ApprovalBecameStale(
            "same-id".into(),
            "old".into(),
            "late stale".into(),
        ));
        assert!(!app.pending_approval.as_ref().unwrap().decision_stale);
        assert!(!app.approval_in_flight);
    }
    #[test]
    fn test_read_only_refresh_compares_incarnation_and_preserves_paginated_uncertainty() {
        let mut queue = crate::approval_client::PendingApprovals {
            count: 1,
            limit: 200,
            updated_at: None,
            items: vec![item(Some("new"), "replacement")],
        };
        assert_eq!(
            crate::approval_client::reconcile_pending(
                &queue,
                "same-id",
                Some("old"),
                Some("tool_call")
            ),
            ApprovalRefresh::Changed
        );
        assert_eq!(
            crate::approval_client::reconcile_pending(
                &queue,
                "same-id",
                Some("new"),
                Some("tool_call")
            ),
            ApprovalRefresh::Same
        );
        queue.items.clear();
        assert_eq!(
            crate::approval_client::reconcile_pending(
                &queue,
                "same-id",
                Some("old"),
                Some("tool_call")
            ),
            ApprovalRefresh::Unknown
        );
        queue.count = 0;
        assert_eq!(
            crate::approval_client::reconcile_pending(
                &queue,
                "same-id",
                Some("old"),
                Some("tool_call")
            ),
            ApprovalRefresh::Missing
        );
    }
}
