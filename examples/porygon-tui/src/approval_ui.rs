//! The action shown for inspection keeps its server-issued decision binding.

use ulttui::prelude::Cmd;
use ulttui::toast::Toast;

use crate::approval_client::PendingApproval;
use crate::types::ApprovalRequest;
use crate::{Msg, PorygonTui};

impl PorygonTui {
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
            reasoning: item.objective.clone(),
            blast_radius: "unknown".into(),
            reversible: None,
        });
        self.approval_is_stream = false;
        self.approval_in_flight = false;
        self.approval_scroll = 0;
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
