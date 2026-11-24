//! Shared terminal and pane event conversion.

use crate::event_loop::TerminalEvent;
use crate::terminal_manager::{TerminalId, TerminalManager, WindowKey};
use ultty_protocol::control::ControlEvent;

/// The subscriber event for `event` in `pane`, if subscribers hear of it.
pub fn control_event(
    pane: TerminalId,
    window: Option<u64>,
    event: &TerminalEvent,
) -> Option<ControlEvent> {
    let pane = pane.raw() as u64;
    Some(match event {
        TerminalEvent::CommandStarted => ControlEvent::CommandStarted { pane, window },
        TerminalEvent::CommandFinished { exit_code } => ControlEvent::CommandFinished {
            pane,
            exit_code: *exit_code,
            window,
        },
        TerminalEvent::Bell => ControlEvent::Bell { pane, window },
        TerminalEvent::Notification { title, body } => ControlEvent::Notification {
            pane,
            title: title.clone(),
            body: body.clone(),
            window,
        },
        TerminalEvent::TitleChanged(title) => ControlEvent::TitleChanged {
            pane,
            title: title.clone(),
            window,
        },
        TerminalEvent::ChildExited { status } => ControlEvent::ProcessExited {
            pane,
            status: *status,
            window,
        },
        TerminalEvent::ProgramStatusChanged { generation, reason } => {
            ControlEvent::ProgramStatusChanged {
                pane,
                generation: *generation,
                reason: status_reason(*reason),
                window,
            }
        }
        // Redraws are too frequent to be useful; clipboard contents stay
        // private to the terminal.
        // Progress shows in `list_panes` statuses.
        TerminalEvent::Redraw
        | TerminalEvent::SetClipboard { .. }
        | TerminalEvent::Progress(_)
        | TerminalEvent::ShellCommandFinished { .. } => return None,
    })
}

/// Convert the terminal's authoritative status change reason for clients.
pub fn status_reason(
    reason: ultty_vt::program_status::StatusChangeReason,
) -> ultty_protocol::control::StatusChangeReason {
    use ultty_protocol::control::StatusChangeReason;
    use ultty_vt::program_status::StatusChangeReason as Source;

    match reason {
        Source::Report => StatusChangeReason::Report,
        Source::Clear => StatusChangeReason::Clear,
        Source::Prompt => StatusChangeReason::Prompt,
        Source::ProcessExit => StatusChangeReason::ProcessExit,
        Source::Reset => StatusChangeReason::Reset,
    }
}

/// Every open pane with the index of its tab and its owning window.
pub fn open_panes(manager: &TerminalManager) -> Vec<(TerminalId, usize, WindowKey)> {
    manager
        .tabs()
        .iter()
        .enumerate()
        .flat_map(|(tab_index, tab)| {
            let window = tab.window_id;
            tab.terminals
                .iter()
                .map(move |&pane| (pane, tab_index, window))
        })
        .collect()
}

/// Panes closed since `before` (in its order), then panes opened (in `now`'s).
pub fn pane_changes(
    before: &[(TerminalId, Option<u64>)],
    now: &[(TerminalId, usize, Option<u64>)],
) -> Vec<ControlEvent> {
    let closed = before
        .iter()
        .filter(|(pane, _)| !now.iter().any(|(open, _, _)| open == pane))
        .map(|(pane, window)| ControlEvent::PaneClosed {
            pane: pane.raw() as u64,
            window: *window,
        });
    let opened = now
        .iter()
        .filter(|(pane, _, _)| !before.iter().any(|(seen, _)| seen == pane))
        .map(|(pane, tab_index, window)| ControlEvent::PaneOpened {
            pane: pane.raw() as u64,
            tab_index: *tab_index,
            window: *window,
        });
    closed.chain(opened).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: usize) -> TerminalId {
        TerminalId::new(n)
    }

    #[test]
    fn command_lifecycle_and_attention_events_reach_subscribers() {
        assert_eq!(
            control_event(
                id(4),
                Some(2),
                &TerminalEvent::CommandFinished { exit_code: Some(2) }
            ),
            Some(ControlEvent::CommandFinished {
                pane: 4,
                exit_code: Some(2),
                window: Some(2),
            })
        );
        assert_eq!(
            control_event(id(4), None, &TerminalEvent::CommandStarted),
            Some(ControlEvent::CommandStarted {
                pane: 4,
                window: None
            })
        );
        assert_eq!(
            control_event(id(1), Some(0), &TerminalEvent::Bell),
            Some(ControlEvent::Bell {
                pane: 1,
                window: Some(0)
            })
        );
        assert_eq!(
            control_event(id(1), None, &TerminalEvent::ChildExited { status: 0 }),
            Some(ControlEvent::ProcessExited {
                pane: 1,
                status: 0,
                window: None
            })
        );
    }

    #[test]
    fn redraws_and_clipboard_contents_are_not_published() {
        assert_eq!(control_event(id(1), None, &TerminalEvent::Redraw), None);
        let clipboard = TerminalEvent::SetClipboard {
            clipboard: 'c',
            data: "secret".to_string(),
        };
        assert_eq!(control_event(id(1), None, &clipboard), None);
    }

    #[test]
    fn test_program_status_event_is_a_pane_scoped_snapshot_notification() {
        let event = control_event(
            id(7),
            None,
            &TerminalEvent::ProgramStatusChanged {
                generation: 42,
                reason: ultty_vt::program_status::StatusChangeReason::Reset,
            },
        )
        .unwrap();
        assert_eq!(event.pane(), Some(7));
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "event": "program_status_changed", "pane": 7, "generation": 42, "reason": "reset"
            })
        );
        assert_eq!(
            control_event(
                id(7),
                Some(0),
                &TerminalEvent::ShellCommandFinished {
                    sequence: 9,
                    exit_code: Some(0)
                }
            ),
            None,
            "internal completion must not duplicate legacy command events"
        );
    }

    #[test]
    fn pane_changes_report_closed_then_opened_panes() {
        let before = [(id(0), Some(0)), (id(1), Some(0)), (id(2), Some(1))];
        let now = [
            (id(0), 0, Some(0)),
            (id(2), 0, Some(1)),
            (id(3), 1, Some(2)),
        ];
        assert_eq!(
            pane_changes(&before, &now),
            vec![
                ControlEvent::PaneClosed {
                    pane: 1,
                    window: Some(0)
                },
                ControlEvent::PaneOpened {
                    pane: 3,
                    tab_index: 1,
                    window: Some(2),
                },
            ]
        );
        assert!(pane_changes(
            &before,
            &[
                (id(0), 0, Some(0)),
                (id(1), 0, Some(0)),
                (id(2), 1, Some(1))
            ]
        )
        .is_empty());
    }
}
