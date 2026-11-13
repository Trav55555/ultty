//! Window-independent owner for terminal processes and native app sessions.
//!
//! Window actions, the clipboard and accessibility remain with the viewer.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use polling::{Events, Poller};

use crate::event_loop::{TerminalEvent, Waker};
use crate::platform::socket::process_group;
use crate::protocol::server::ProtocolServer;
use crate::terminal_manager::{TerminalId, TerminalManager};

/// One completed pass over the terminal processes.
pub struct RuntimeStep {
    /// Events in the order each terminal's event loop returned them.
    pub terminal_events: Vec<(TerminalId, Vec<TerminalEvent>)>,
    /// Whether the focused pane's inline rows changed during pairing.
    pub focused_inline_changed: bool,
    /// Whether a visible native session needs a render.
    pub native_dirty: bool,
}

#[derive(PartialEq)]
struct PairingObservation {
    revision: u64,
    on_screen: bool,
    sessions: Vec<(u64, u64)>,
}

/// Owns the live children and their native protocol server independently of a window.
pub struct SessionRuntime {
    /// The authoritative terminal, tab and pane model.
    pub manager: TerminalManager,
    /// Native application connections and their committed scenes.
    pub protocol: Option<ProtocolServer>,
    fallback_grid: (u16, u16),
    native_layout_generation: Option<u64>,
    viewer_present: bool,
    fallback_poller: Option<Arc<Poller>>,
    runtime_waker: Waker,
    pairing_observations: HashMap<TerminalId, PairingObservation>,
}

impl SessionRuntime {
    /// Adopt already initialized terminal and protocol owners.
    pub fn new(
        mut manager: TerminalManager,
        protocol: Option<ProtocolServer>,
        fallback_grid: (u16, u16),
    ) -> io::Result<Self> {
        let fallback_poller = if protocol.is_none() {
            Some(Arc::new(Poller::new()?))
        } else {
            None
        };
        let waker = if let Some(server) = protocol.as_ref() {
            server.wake_handle()
        } else {
            let poller = Arc::clone(
                fallback_poller
                    .as_ref()
                    .ok_or_else(|| io::Error::other("missing runtime poller"))?,
            );
            Arc::new(move || {
                if let Err(error) = poller.notify() {
                    tracing::warn!(%error, "failed to wake runtime poller");
                }
            })
        };
        manager.set_waker(Arc::clone(&waker));
        Ok(Self {
            manager,
            protocol,
            fallback_grid,
            native_layout_generation: None,
            viewer_present: false,
            fallback_poller,
            runtime_waker: waker,
            pairing_observations: HashMap::new(),
        })
    }

    /// Access the terminal model for read-only queries.
    pub fn manager(&self) -> &TerminalManager {
        &self.manager
    }

    /// Apply terminal model commands, including pane layout and resize.
    pub fn manager_mut(&mut self) -> &mut TerminalManager {
        &mut self.manager
    }

    /// Access native sessions for read-only inspection.
    pub fn protocol(&self) -> Option<&ProtocolServer> {
        self.protocol.as_ref()
    }

    /// Access native sessions for input and application replies.
    pub fn protocol_mut(&mut self) -> Option<&mut ProtocolServer> {
        self.protocol.as_mut()
    }

    /// Set the fallback grid for native sessions not yet bound to a pane.
    pub fn set_fallback_grid(&mut self, size: (u16, u16)) {
        self.fallback_grid = size;
        self.native_layout_generation = None;
    }

    /// Forward PTY wakeups to a viewer as well as the runtime poller.
    pub fn set_viewer_waker(&mut self, viewer: Option<Waker>) {
        let runtime = Arc::clone(&self.runtime_waker);
        self.manager.set_waker(Arc::new(move || {
            runtime();
            if let Some(wake) = &viewer {
                wake();
            }
        }));
    }

    /// Tell the runtime whether a viewer is displaying the active tab.
    /// Detached runtimes withhold active-tab clipboard and keyboard access.
    pub fn set_viewer_present(&mut self, present: bool) {
        self.viewer_present = present;
        self.sync_tabs();
    }

    /// Wait for PTY or native socket readiness, bounded by maintenance deadlines.
    /// The caller must not hold a terminal guard while waiting.
    pub fn wait(&mut self, timeout: Duration) -> io::Result<()> {
        let mut timeout = timeout.min(Duration::from_secs(1));
        if let Some(deadline) = self
            .protocol
            .as_ref()
            .and_then(ProtocolServer::next_pairing_deadline)
        {
            timeout = timeout.min(deadline.saturating_duration_since(Instant::now()));
        }
        if let Some(server) = self.protocol.as_mut() {
            server.poll(Some(timeout))
        } else if let Some(poller) = self.fallback_poller.as_ref() {
            poller.wait(&mut Events::new(), Some(timeout)).map(|_| ())
        } else {
            Err(io::Error::other("missing runtime poller"))
        }
    }

    /// Close listeners for removed tabs and synchronize the active viewer.
    pub fn sync_tabs(&mut self) {
        let Some(server) = self.protocol.as_mut() else {
            return;
        };
        let closed: Vec<_> = server
            .open_tabs()
            .filter(|open| !self.manager.tabs().iter().any(|tab| tab.id == *open))
            .collect();
        for tab in closed {
            server.close_tab(tab);
        }
        server.retain_pane_sessions(|pane| self.manager.terminal(pane).is_some());
        let active = if self.viewer_present {
            self.manager.active_tab().map(|tab| tab.id)
        } else {
            None
        };
        server.set_active_tab(active);
    }

    /// Poll native apps and drive their transport, layout, and keepalives.
    /// Returns whether animation or a visible scene needs drawing.
    pub fn poll_native(&mut self) -> io::Result<bool> {
        self.sync_tabs();
        let mut redraw = false;
        let mut bound = 0;
        if let Some(server) = self.protocol.as_mut() {
            server.poll(Some(Duration::ZERO))?;
            redraw |= server.update_animations();
            server.check_keepalives();
            if !self.viewer_present {
                for key in server.pending_clipboard_gets() {
                    server.send_clipboard_content(key, None);
                }
                let _ = server.pending_clipboard_sets();
            }
            bound = server.bind_unbound_sessions(|tab, peer| {
                self.manager
                    .pane_for_process_group(tab, peer.and_then(process_group))
            });
            redraw |= server
                .active_tab_sessions()
                .any(|session| session.is_render_dirty());
        }
        if bound > 0 || Some(self.manager.layout_generation()) != self.native_layout_generation {
            self.sync_native_viewports();
        }
        self.pump_in_band();
        Ok(redraw)
    }

    /// Size native sessions to their bound pane or the fallback grid.
    pub fn sync_native_viewports(&mut self) {
        self.native_layout_generation = Some(self.manager.layout_generation());
        let Some(server) = self.protocol.as_mut() else {
            return;
        };
        server.retain_pane_sessions(|pane| self.manager.terminal(pane).is_some());
        server.resize_sessions(self.fallback_grid, |pane| self.manager.pane_grid_size(pane));
    }

    /// Poll native connections, transfer in-band frames and drain terminal events.
    ///
    /// The caller handles the returned events before calling [`Self::cleanup_exited`],
    /// so completion events and final inline content are not lost on child exit.
    pub fn step(&mut self, now: Instant) -> io::Result<RuntimeStep> {
        let redraw = self.poll_native()?;
        let terminal_events = self.manager.process_all()?;
        let with_output: Vec<_> = terminal_events
            .iter()
            .filter_map(|(pane, events)| {
                events
                    .iter()
                    .any(|event| matches!(event, TerminalEvent::Redraw))
                    .then_some(*pane)
            })
            .collect();
        let focused_inline_changed = self.pair_inline_surfaces(&with_output, now);
        let native_dirty = redraw
            || (self.viewer_present
                && self.protocol.as_ref().is_some_and(|server| {
                    server
                        .active_tab_sessions()
                        .any(|session| session.is_render_dirty())
                }));
        Ok(RuntimeStep {
            terminal_events,
            focused_inline_changed,
            native_dirty,
        })
    }

    /// Remove exited panes after their final events and inline state were processed.
    pub fn cleanup_exited(&mut self) {
        self.manager.cleanup_exited();
        self.sync_tabs();
    }

    fn pump_in_band(&mut self) {
        let tabs: Vec<_> = self
            .manager
            .tabs()
            .iter()
            .map(|tab| (tab.id, tab.terminals.clone()))
            .collect();
        for (tab, panes) in tabs {
            for pane in panes {
                let Some(terminal) = self.manager.terminal(pane) else {
                    continue;
                };
                let event_loop = &terminal.event_loop;
                let Some(input) = event_loop.take_in_band() else {
                    continue;
                };
                let reply = if let Some(server) = self.protocol.as_mut() {
                    server.in_band_input(
                        tab,
                        pane,
                        self.manager
                            .pane_grid_size(pane)
                            .unwrap_or(self.fallback_grid),
                        event_loop.foreground_pgrp(),
                        || event_loop.foreground_pids(),
                        &input.bytes,
                        input.broken,
                    );
                    server.take_pane_in_band_output(pane)
                } else {
                    Vec::new()
                };
                if let Err(error) = event_loop.finish_in_band(&reply) {
                    tracing::warn!(%error, "failed to answer in-band input");
                }
            }
        }
        let Some(server) = self.protocol.as_mut() else {
            return;
        };
        for pane in server.in_band_panes() {
            let Some(terminal) = self.manager.terminal(pane) else {
                server.end_in_band_session(pane);
                continue;
            };
            let event_loop = &terminal.event_loop;
            server.end_in_band_if_app_gone(pane, event_loop.foreground_pgrp());
            let output = server.take_pane_in_band_output(pane);
            if let Err(error) = event_loop.send_in_band(&output) {
                tracing::warn!(%error, "failed to send in-band output");
                server.end_in_band_session(pane);
            }
        }
    }

    /// Pair pane-local anchors and native inline surfaces and refresh copy text.
    pub fn pair_inline_surfaces(&mut self, with_output: &[TerminalId], now: Instant) -> bool {
        let Some(server) = self.protocol.as_mut() else {
            return false;
        };
        let focused = self.manager.focused_terminal_id();
        let active = self.manager.active_tab().map(|tab| tab.id);
        let mut focused_changed = false;
        for tab in self.manager.tabs() {
            for &pane in &tab.terminals {
                let Some(terminal) = self.manager.terminal(pane) else {
                    continue;
                };
                let on_screen = self.viewer_present
                    && Some(tab.id) == active
                    && !server
                        .pane_sessions(pane)
                        .any(|session| session.is_full_scene());
                let revision = terminal.event_loop.snapshot_generation();
                let mut sessions: Vec<_> = server
                    .all_pane_sessions(pane)
                    .map(|session| (session.session_id(), session.shown_generation()))
                    .collect();
                sessions.sort_unstable();
                let observation_changed = self.pairing_observations.get(&pane)
                    != Some(&PairingObservation {
                        revision,
                        on_screen,
                        sessions: sessions.clone(),
                    });
                let native_changed = server
                    .all_pane_sessions(pane)
                    .any(|session| session.is_render_dirty() || session.is_scroll_dirty());
                let deadline_due = server
                    .next_pairing_deadline()
                    .is_some_and(|deadline| deadline <= now);
                if !with_output.contains(&pane)
                    && (!server.needs_pairing(tab.id, pane)
                        || !(observation_changed || native_changed || deadline_due))
                {
                    continue;
                }
                let mut state = terminal.event_loop.terminal();
                let wrote = server.pair_inline_surfaces(tab.id, pane, &mut state, on_screen, now);
                let mut sessions: Vec<_> = server
                    .all_pane_sessions(pane)
                    .map(|session| (session.session_id(), session.shown_generation()))
                    .collect();
                sessions.sort_unstable();
                self.pairing_observations.insert(
                    pane,
                    PairingObservation {
                        revision: state.snapshot_generation(),
                        on_screen,
                        sessions,
                    },
                );
                focused_changed |= wrote && Some(pane) == focused;
            }
        }
        server.expire_unplaced_surfaces(now);
        server.retain_pairings(|pane| self.manager.terminal(pane).is_some());
        self.pairing_observations
            .retain(|pane, _| self.manager.terminal(*pane).is_some());
        focused_changed
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use ultty_protocol::{
        AppMessage, Constraints, Decode, Encode, FrameId, RegionId, Style, TerminalMessage,
        PROTOCOL_VERSION,
    };

    use super::*;

    #[test]
    fn detached_runtime_drives_pty_and_native_socket_without_a_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = ProtocolServer::new(dir.path()).unwrap();
        let manager = TerminalManager::with_command_tab(
            80,
            24,
            "sh",
            &["-c".into(), "printf PTY_READY; sleep 3".into()],
            &mut |tab| {
                Ok(vec![(
                    "ULTTY_SOCKET".into(),
                    server.open_tab(tab)?.into_os_string(),
                )])
            },
        )
        .unwrap();
        let tab = manager.active_tab().unwrap().id;
        let pane = manager.focused_terminal_id().unwrap();
        let path = server.tab_socket_path(tab).unwrap().to_path_buf();
        let mut runtime = SessionRuntime::new(manager, Some(server), (80, 24)).unwrap();
        let mut client = UnixStream::connect(path).unwrap();
        let messages = [
            AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
            },
            AppMessage::BeginFrame {
                frame_id: FrameId(1),
            },
            AppMessage::CreateRegion {
                id: RegionId::new(1),
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
            AppMessage::EndFrame {
                frame_id: FrameId(1),
            },
            AppMessage::ClipboardGet,
        ];
        for message in messages {
            client.write_all(&message.encode_to_bytes()).unwrap();
        }
        client.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        let until = Instant::now() + Duration::from_secs(3);
        let mut saw_pty = false;
        let mut saw_scene = false;
        let mut saw_denial = false;
        while Instant::now() < until {
            runtime.wait(Duration::from_millis(100)).unwrap();
            let step = runtime.step(Instant::now()).unwrap();
            saw_pty |= step.terminal_events.iter().any(|(id, events)| {
                *id == pane && events.iter().any(|e| matches!(e, TerminalEvent::Redraw))
            });
            saw_scene |= runtime.protocol().unwrap().sessions().any(|session| {
                session.pane() == Some(pane) && session.scene().get(RegionId::new(1)).is_some()
            });
            let mut chunk = [0; 4096];
            loop {
                match client.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("native socket read: {error}"),
                }
            }
            let mut cursor = bytes.as_slice();
            while let Ok(message) = TerminalMessage::decode(&mut cursor) {
                saw_denial |= matches!(message, TerminalMessage::ClipboardContent { text: None });
            }
            if saw_pty && saw_scene && saw_denial {
                break;
            }
        }
        assert!(saw_pty, "PTY output did not wake the detached runtime");
        assert!(
            saw_scene,
            "native committed scene did not progress without a viewer"
        );
        assert!(saw_denial, "detached native clipboard read was not denied");
        assert_eq!(runtime.protocol().unwrap().active_tab(), None);
    }

    #[test]
    fn detached_inline_frame_updates_without_new_pty_output() {
        use ultty_protocol::{Content, TextContent};
        let dir = tempfile::tempdir().unwrap();
        let mut server = ProtocolServer::new(dir.path()).unwrap();
        let manager =
            TerminalManager::with_command_tab(80, 24, "/bin/sleep", &["30".into()], &mut |tab| {
                Ok(vec![(
                    "ULTTY_SOCKET".into(),
                    server.open_tab(tab)?.into_os_string(),
                )])
            })
            .unwrap();
        let tab = manager.active_tab().unwrap().id;
        let pane = manager.focused_terminal_id().unwrap();
        let path = server.tab_socket_path(tab).unwrap().to_path_buf();
        let mut runtime = SessionRuntime::new(manager, Some(server), (80, 24)).unwrap();
        let mut peer = UnixStream::connect(path).unwrap();
        let send = |peer: &mut UnixStream, messages: Vec<AppMessage>| {
            for message in messages {
                peer.write_all(&message.encode_to_bytes()).unwrap();
            }
        };
        send(
            &mut peer,
            vec![AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
            }],
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let session_id = loop {
            runtime.poll_native().unwrap();
            if let Some(id) = runtime
                .protocol()
                .unwrap()
                .all_pane_sessions(pane)
                .next()
                .map(|session| session.session_id())
            {
                break id;
            }
            assert!(Instant::now() < deadline, "native session did not bind");
            runtime.wait(Duration::from_millis(20)).unwrap();
        };
        send(
            &mut peer,
            vec![
                AppMessage::BeginFrame {
                    frame_id: FrameId(1),
                },
                AppMessage::CreateSurface {
                    id: RegionId::new(5),
                    rows: 1,
                    constraints: Constraints::default(),
                    style: Style::default(),
                },
                AppMessage::SetContent {
                    id: RegionId::new(5),
                    content: Content::Text(TextContent::new("old-card")),
                },
                AppMessage::EndFrame {
                    frame_id: FrameId(1),
                },
            ],
        );
        runtime
            .manager
            .terminal(pane)
            .unwrap()
            .event_loop
            .terminal()
            .place_anchor(crate::grid::Anchor {
                session_id,
                surface_id: 5,
                rows: 1,
            });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.poll_native().unwrap();
            runtime.pair_inline_surfaces(&[pane], Instant::now());
            let state = runtime
                .manager
                .terminal(pane)
                .unwrap()
                .event_loop
                .terminal();
            if state.main_buffer().row_at(0).unwrap().text_trimmed() == "old-card" {
                break;
            }
            drop(state);
            assert!(
                Instant::now() < deadline,
                "initial inline content did not pair"
            );
            runtime.wait(Duration::from_millis(20)).unwrap();
        }
        for session in runtime.protocol_mut().unwrap().sessions_mut() {
            session.clear_render_dirty();
            session.clear_scroll_dirty();
        }
        let before = runtime
            .manager
            .terminal(pane)
            .unwrap()
            .event_loop
            .snapshot_generation();
        send(
            &mut peer,
            vec![
                AppMessage::BeginFrame {
                    frame_id: FrameId(2),
                },
                AppMessage::SetContent {
                    id: RegionId::new(5),
                    content: Content::Text(TextContent::new("new-card")),
                },
                AppMessage::EndFrame {
                    frame_id: FrameId(2),
                },
            ],
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime.poll_native().unwrap();
            assert_eq!(
                runtime
                    .manager
                    .terminal(pane)
                    .unwrap()
                    .event_loop
                    .snapshot_generation(),
                before,
                "test requires no new PTY mutation"
            );
            if runtime
                .protocol()
                .unwrap()
                .all_pane_sessions(pane)
                .any(|session| session.is_render_dirty())
            {
                break;
            }
            assert!(Instant::now() < deadline, "native frame was not received");
            runtime.wait(Duration::from_millis(20)).unwrap();
        }
        runtime.pair_inline_surfaces(&[], Instant::now());
        let state = runtime
            .manager
            .terminal(pane)
            .unwrap()
            .event_loop
            .terminal();
        assert_eq!(
            state.main_buffer().row_at(0).unwrap().text_trimmed(),
            "new-card"
        );
        assert_eq!(runtime.protocol().unwrap().active_tab(), None);
    }
}
