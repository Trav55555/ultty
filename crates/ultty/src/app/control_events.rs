//! Events for control-plane subscribers (`ulttyctl subscribe`).
//!
//! Terminal events become [`ControlEvent`]s named by pane, and the panes
//! seen at the end of each update are compared with the previous set to
//! report panes that opened or closed.

use ultty::event_loop::TerminalEvent;
use ultty::terminal_manager::{TerminalId, TerminalManager};
use ultty_protocol::control::ControlEvent;

use super::App;

/// The subscriber event for `event` in `pane`, if subscribers hear of it.
pub(super) fn control_event(pane: TerminalId, event: &TerminalEvent) -> Option<ControlEvent> {
    let pane = pane.raw() as u64;
    Some(match event {
        TerminalEvent::CommandStarted => ControlEvent::CommandStarted { pane },
        TerminalEvent::CommandFinished { exit_code } => ControlEvent::CommandFinished {
            pane,
            exit_code: *exit_code,
        },
        TerminalEvent::Bell => ControlEvent::Bell { pane },
        TerminalEvent::Notification { title, body } => ControlEvent::Notification {
            pane,
            title: title.clone(),
            body: body.clone(),
        },
        TerminalEvent::TitleChanged(title) => ControlEvent::TitleChanged {
            pane,
            title: title.clone(),
        },
        TerminalEvent::ChildExited { status } => ControlEvent::ProcessExited {
            pane,
            status: *status,
        },
        // Redraws are too frequent to be useful; clipboard contents stay
        // private to the terminal.
        TerminalEvent::Redraw | TerminalEvent::SetClipboard { .. } => return None,
    })
}

/// Every open pane with the index of its tab.
pub(super) fn open_panes(manager: &TerminalManager) -> Vec<(TerminalId, usize)> {
    manager
        .tabs()
        .iter()
        .enumerate()
        .flat_map(|(tab_index, tab)| tab.terminals.iter().map(move |&pane| (pane, tab_index)))
        .collect()
}

/// Panes closed since `before` (in its order), then panes opened (in `now`'s).
pub(super) fn pane_changes(
    before: &[TerminalId],
    now: &[(TerminalId, usize)],
) -> Vec<ControlEvent> {
    let closed = before
        .iter()
        .filter(|pane| !now.iter().any(|(open, _)| open == *pane))
        .map(|pane| ControlEvent::PaneClosed {
            pane: pane.raw() as u64,
        });
    let opened = now
        .iter()
        .filter(|(pane, _)| !before.contains(pane))
        .map(|(pane, tab_index)| ControlEvent::PaneOpened {
            pane: pane.raw() as u64,
            tab_index: *tab_index,
        });
    closed.chain(opened).collect()
}

impl App {
    /// Tell subscribers about panes opened or closed since the last call,
    /// whether by a key binding, a control request or a shell exiting.
    pub(super) fn publish_pane_changes(&mut self) {
        let now = open_panes(&self.terminal_manager);
        if now.len() == self.published_panes.len()
            && now
                .iter()
                .zip(&self.published_panes)
                .all(|((pane, _), seen)| pane == seen)
        {
            return;
        }
        let changes = pane_changes(&self.published_panes, &now);
        self.published_panes = now.into_iter().map(|(pane, _)| pane).collect();
        if let Some(server) = self.control_server.as_mut() {
            for event in &changes {
                server.publish(event);
            }
        }
    }
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
                &TerminalEvent::CommandFinished { exit_code: Some(2) }
            ),
            Some(ControlEvent::CommandFinished {
                pane: 4,
                exit_code: Some(2)
            })
        );
        assert_eq!(
            control_event(id(4), &TerminalEvent::CommandStarted),
            Some(ControlEvent::CommandStarted { pane: 4 })
        );
        assert_eq!(
            control_event(id(1), &TerminalEvent::Bell),
            Some(ControlEvent::Bell { pane: 1 })
        );
        assert_eq!(
            control_event(id(1), &TerminalEvent::ChildExited { status: 0 }),
            Some(ControlEvent::ProcessExited { pane: 1, status: 0 })
        );
    }

    #[test]
    fn redraws_and_clipboard_contents_are_not_published() {
        assert_eq!(control_event(id(1), &TerminalEvent::Redraw), None);
        let clipboard = TerminalEvent::SetClipboard {
            clipboard: 'c',
            data: "secret".to_string(),
        };
        assert_eq!(control_event(id(1), &clipboard), None);
    }

    #[test]
    fn pane_changes_report_closed_then_opened_panes() {
        let before = [id(0), id(1), id(2)];
        let now = [(id(0), 0), (id(2), 0), (id(3), 1)];
        assert_eq!(
            pane_changes(&before, &now),
            vec![
                ControlEvent::PaneClosed { pane: 1 },
                ControlEvent::PaneOpened {
                    pane: 3,
                    tab_index: 1
                },
            ]
        );
        assert!(pane_changes(&before, &[(id(0), 0), (id(1), 0), (id(2), 1)]).is_empty());
    }
}
