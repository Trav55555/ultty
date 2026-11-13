//! Authoritative session operations, independent of viewer connections.

use std::io;
use std::time::{Duration, Instant};

use ultty_protocol::control::PaneStatus;
use ultty_protocol::control::{OwnerRef, SplitDirection as WireDirection};

use super::auth::{Authority, Decision};
use super::model::SessionSnapshot;
use super::transport::Server;
use super::wire::{self, Command, ErrorCode, LayoutCommand, Reply};
use crate::event_loop::TerminalEvent;
use crate::runtime::SessionRuntime;
use crate::terminal_manager::{progress_status, PaneArea, SplitDirection, TerminalId};

/// Persistent runtime and canonical snapshot publication state.
pub struct HostState {
    /// Runtime is accessed exclusively by the host owner thread.
    pub runtime: SessionRuntime,
    owner: OwnerRef,
    viewport: [u16; 2],
    cell_pixels: [u16; 2],
    sequence: u64,
    cached: Option<SessionSnapshot>,
    cached_stamp: Option<SnapshotStamp>,
}

#[derive(PartialEq)]
struct SnapshotStamp {
    viewport: [u16; 2],
    cell_pixels: [u16; 2],
    layout_generation: u64,
    active_tab: Option<u64>,
    tabs: Vec<(u64, String, u64, Vec<u64>)>,
    panes: Vec<PaneStamp>,
}

#[derive(PartialEq)]
struct PaneStamp {
    id: u64,
    revision: u64,
    title: String,
    status: Option<PaneStatus>,
    native: Vec<(u64, u64)>,
}

impl SnapshotStamp {
    fn captured(snapshot: &SessionSnapshot, layout_generation: u64) -> Self {
        Self {
            viewport: snapshot.viewport,
            cell_pixels: snapshot.cell_pixels,
            layout_generation,
            active_tab: snapshot.active_tab,
            tabs: snapshot
                .tabs
                .iter()
                .map(|tab| (tab.id, tab.title.clone(), tab.focused, tab.panes.clone()))
                .collect(),
            panes: snapshot
                .panes
                .iter()
                .map(|pane| PaneStamp {
                    id: pane.id,
                    revision: pane.revision,
                    title: pane.title.clone(),
                    status: pane.status.clone(),
                    native: pane
                        .native
                        .iter()
                        .map(|native| (native.session_id, native.generation))
                        .collect(),
                })
                .collect(),
        }
    }
}

impl HostState {
    /// Adopt a runtime and assign its fixed owner identity and viewport.
    pub fn new(
        mut runtime: SessionRuntime,
        owner: OwnerRef,
        viewport: [u16; 2],
    ) -> io::Result<Self> {
        runtime.set_viewer_present(false);
        let mut host = Self {
            runtime,
            owner,
            viewport,
            cell_pixels: [8, 16],
            sequence: 0,
            cached: None,
            cached_stamp: None,
        };
        host.resize(viewport[0], viewport[1], 8, 16)?;
        Ok(host)
    }

    /// Drive all children and native apps once, even without viewers or tabs.
    /// Returns the observed events for control subscriptions and completion waits.
    pub fn step(
        &mut self,
        now: Instant,
        viewers_present: bool,
    ) -> io::Result<Vec<(TerminalId, Vec<TerminalEvent>)>> {
        self.runtime.set_viewer_present(viewers_present);
        let step = self.runtime.step(now)?;
        for (pane, events) in &step.terminal_events {
            for event in events {
                match event {
                    TerminalEvent::Progress(progress) => {
                        if let Some(terminal) = self.runtime.manager.terminal_mut(*pane) {
                            terminal.progress = progress_status(*progress);
                        }
                    }
                    TerminalEvent::CommandFinished { .. } => {
                        if let Some(terminal) = self.runtime.manager.terminal_mut(*pane) {
                            terminal.progress = None;
                        }
                    }
                    _ => {}
                }
            }
        }
        // A host has no viewer clipboard, regardless of attachment status.
        if let Some(server) = self.runtime.protocol.as_mut() {
            for key in server.pending_clipboard_gets() {
                server.send_clipboard_content(key, None);
            }
            let _ = server.pending_clipboard_sets();
        }
        Ok(step.terminal_events)
    }

    /// Execute a command already authenticated and authorized by the caller.
    pub fn execute(&mut self, command: &Command) -> Reply {
        match self.execute_inner(command) {
            Ok(reply) => reply,
            Err(error) => Reply::error(ErrorCode::Failed, error.to_string()),
        }
    }

    fn execute_inner(&mut self, command: &Command) -> io::Result<Reply> {
        match command {
            Command::Snapshot { after_sequence } => {
                if after_sequence.is_some_and(|value| value > self.sequence) {
                    return Ok(Reply::error(
                        ErrorCode::Conflict,
                        "snapshot sequence is ahead of this owner",
                    ));
                }
                let (stamp, native_dirty) = self.probe_stamp();
                if !native_dirty && self.cached_stamp.as_ref() == Some(&stamp) {
                    if *after_sequence == Some(self.sequence) {
                        return Ok(Reply::Unchanged {
                            sequence: self.sequence,
                        });
                    }
                    if let Some(snapshot) = &self.cached {
                        return Ok(Reply::Snapshot {
                            snapshot: Box::new(snapshot.clone()),
                        });
                    }
                }
                let sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("snapshot sequence exhausted"))?;
                let snapshot = SessionSnapshot::capture(
                    &self.runtime,
                    self.owner.clone(),
                    sequence,
                    self.viewport,
                    self.cell_pixels,
                )
                .map_err(io::Error::other)?;
                self.sequence = sequence;
                self.cached_stamp = Some(SnapshotStamp::captured(
                    &snapshot,
                    self.runtime.manager.layout_generation(),
                ));
                self.cached = Some(snapshot.clone());
                if let Some(server) = self.runtime.protocol.as_mut() {
                    for session in server.sessions_mut() {
                        session.clear_render_dirty();
                        session.clear_scroll_dirty();
                    }
                }
                Ok(Reply::Snapshot {
                    snapshot: Box::new(snapshot),
                })
            }
            Command::Input { pane, data } => {
                let id = pane_id(*pane)?;
                self.runtime
                    .manager
                    .terminal_mut(id)
                    .ok_or_else(missing_pane)?
                    .event_loop
                    .send_input(data)?;
                Ok(applied(Some(id), None))
            }
            Command::NativeInput {
                session,
                generation,
                payload,
            } => {
                let event = wire::decode_native_input(payload)?;
                if !event.x.is_finite()
                    || !event.y.is_finite()
                    || matches!(&event.event_type, ultty_protocol::InputType::MouseScroll { delta_x, delta_y } if !delta_x.is_finite() || !delta_y.is_finite())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "non-finite native input coordinates",
                    ));
                }
                let pane = self
                    .runtime
                    .protocol
                    .as_ref()
                    .and_then(|server| {
                        server
                            .active_tab_sessions()
                            .find(|target| target.session_id() == *session)
                    })
                    .and_then(|target| target.pane());
                let pane = pane.filter(|pane| self.runtime.manager.is_on_screen(*pane));
                let Some(pane) = pane else {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "native session is not visible",
                    ));
                };
                let foreground = self
                    .runtime
                    .manager
                    .terminal(pane)
                    .and_then(|terminal| terminal.event_loop.foreground_pgrp());
                let focused = self.runtime.manager.focused_terminal_id() == Some(pane);
                let server = self.runtime.protocol.as_mut().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "native server unavailable")
                })?;
                let target = server
                    .active_tab_sessions_mut()
                    .find(|target| target.session_id() == *session)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "visible native session not found")
                    })?;
                if !target.is_full_scene() || !target.is_in_foreground(foreground) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "native session is not an interactive foreground scene",
                    ));
                }
                if matches!(
                    event.event_type,
                    ultty_protocol::InputType::KeyDown { .. }
                        | ultty_protocol::InputType::KeyUp { .. }
                        | ultty_protocol::InputType::TextInput { .. }
                ) {
                    let key_target = target
                        .focused_region()
                        .filter(|id| {
                            target.is_subscribed(*id, ultty_protocol::EVENT_KEY)
                                && !target.scene().is_in_surface(*id)
                        })
                        .unwrap_or(ultty_protocol::RegionId::ROOT);
                    if !focused
                        || !target.has_key_subscription()
                        || !target.is_interactive_target(key_target)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "native session cannot receive keys",
                        ));
                    }
                    match event.event_type {
                        ultty_protocol::InputType::KeyDown { key, modifiers } => {
                            target.send_key(key, modifiers)?
                        }
                        ultty_protocol::InputType::KeyUp { key, modifiers } => {
                            target.send_key_up(key, modifiers)?
                        }
                        ultty_protocol::InputType::TextInput { text } => {
                            target.send_text_input(text)?
                        }
                        _ => unreachable!(),
                    }
                    target.flush().map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("input may have been delivered: {error}"),
                        )
                    })?;
                    return Ok(Reply::Ok);
                }
                if target.shown_generation() != *generation {
                    return Ok(Reply::error(
                        ErrorCode::Conflict,
                        "native scene changed; refresh before input",
                    ));
                }
                let mask = match &event.event_type {
                    ultty_protocol::InputType::KeyDown { .. }
                    | ultty_protocol::InputType::KeyUp { .. }
                    | ultty_protocol::InputType::TextInput { .. } => ultty_protocol::EVENT_KEY,
                    ultty_protocol::InputType::MouseScroll { .. } => ultty_protocol::EVENT_SCROLL,
                    _ => ultty_protocol::EVENT_MOUSE,
                };
                if !target.is_interactive_target(event.region_id)
                    || !(target.is_subscribed(event.region_id, mask)
                        || target.is_subscribed(ultty_protocol::RegionId::ROOT, mask))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "native target is not subscribed",
                    ));
                }
                target.send_input(event)?;
                target.flush().map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("input may have been delivered: {error}"),
                    )
                })?;
                Ok(Reply::Ok)
            }
            Command::Resize {
                cols,
                rows,
                cell_width,
                cell_height,
            } => {
                self.resize(*cols, *rows, *cell_width, *cell_height)?;
                Ok(Reply::Ok)
            }
            Command::Spawn { program, args, cwd } => {
                let server = &mut self.runtime.protocol;
                let tab = self.runtime.manager.create_tab_with_command(
                    program,
                    args,
                    cwd.as_deref(),
                    None,
                    &mut |tab| {
                        let mut env = vec![("TERM".into(), "xterm-256color".into())];
                        if let Some(server) = server.as_mut() {
                            let path = server.open_tab(tab)?;
                            env.push(("ULTTY_SOCKET".into(), path.into_os_string()));
                        }
                        Ok(env)
                    },
                );
                self.runtime.sync_tabs();
                let tab = tab?;
                let layout = self.resize(
                    self.viewport[0],
                    self.viewport[1],
                    self.cell_pixels[0],
                    self.cell_pixels[1],
                );
                let pane = self.runtime.manager.focused_terminal_id();
                self.runtime.sync_native_viewports();
                Ok(spawn_receipt(pane, tab, layout))
            }
            Command::Layout { command } => {
                let pane = match command {
                    LayoutCommand::Focus { pane } => {
                        let pane = pane_id(*pane)?;
                        if !self.runtime.manager.focus_pane_by_id(pane) {
                            return Err(missing_pane());
                        }
                        Some(pane)
                    }
                    LayoutCommand::Split {
                        pane,
                        direction,
                        program,
                        args,
                        cwd,
                    } => {
                        let pane = pane_id(*pane)?;
                        let direction = match direction {
                            WireDirection::Right => SplitDirection::Horizontal,
                            WireDirection::Down => SplitDirection::Vertical,
                        };
                        let result = if let Some(program) = program {
                            self.runtime.manager.split_pane_with_command(
                                pane,
                                direction,
                                program,
                                args,
                                cwd.as_deref(),
                            )?
                        } else {
                            self.runtime.manager.split_pane(pane, direction)?
                        };
                        Some(result.ok_or_else(missing_pane)?)
                    }
                    LayoutCommand::Close { pane } => {
                        let pane = pane_id(*pane)?;
                        if !self.runtime.manager.focus_pane_by_id(pane) {
                            return Err(missing_pane());
                        }
                        let tab = self.runtime.manager.active_tab().ok_or_else(missing_pane)?;
                        if tab.terminals.len() == 1 {
                            let tab = tab.id;
                            self.runtime.manager.close_tab(tab);
                        } else if !self.runtime.manager.close_focused_pane() {
                            return Err(missing_pane());
                        }
                        None
                    }
                    LayoutCommand::Zoom { pane, zoomed } => {
                        let pane = pane_id(*pane)?;
                        if !self.runtime.manager.set_pane_zoom(pane, *zoomed) {
                            return Err(missing_pane());
                        }
                        Some(pane)
                    }
                    LayoutCommand::Swap { first, second } => {
                        let first = pane_id(*first)?;
                        if !self.runtime.manager.swap_panes(first, pane_id(*second)?) {
                            return Err(missing_pane());
                        }
                        Some(first)
                    }
                };
                self.runtime.sync_tabs();
                self.runtime.sync_native_viewports();
                Ok(applied(pane, None))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command is handled by attachment authority",
            )),
        }
    }

    fn probe_stamp(&self) -> (SnapshotStamp, bool) {
        let manager = &self.runtime.manager;
        let tabs = manager
            .tabs()
            .iter()
            .map(|tab| {
                (
                    tab.id.raw() as u64,
                    tab.title.clone(),
                    tab.focused_terminal.raw() as u64,
                    tab.terminals.iter().map(|id| id.raw() as u64).collect(),
                )
            })
            .collect();
        let mut dirty = false;
        let panes = manager
            .tabs()
            .iter()
            .flat_map(|tab| &tab.terminals)
            .filter_map(|id| manager.terminal(*id))
            .map(|pane| {
                let native = self
                    .runtime
                    .protocol
                    .as_ref()
                    .map(|server| {
                        server
                            .all_pane_sessions(pane.id)
                            .map(|session| {
                                dirty |= session.is_render_dirty() || session.is_scroll_dirty();
                                (session.session_id(), session.shown_generation())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                PaneStamp {
                    id: pane.id.raw() as u64,
                    revision: pane.event_loop.snapshot_generation(),
                    title: pane.title.clone(),
                    status: pane.shown_status().cloned(),
                    native,
                }
            })
            .collect();
        (
            SnapshotStamp {
                viewport: self.viewport,
                cell_pixels: self.cell_pixels,
                layout_generation: manager.layout_generation(),
                active_tab: manager.active_tab().map(|tab| tab.id.raw() as u64),
                tabs,
                panes,
            },
            dirty,
        )
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width: u16,
        cell_height: u16,
    ) -> io::Result<()> {
        if cols == 0
            || rows == 0
            || cols > 4096
            || rows > 4096
            || !(1..=512).contains(&cell_width)
            || !(1..=512).contains(&cell_height)
            || u32::from(cols) * u32::from(cell_width) > u32::from(u16::MAX)
            || u32::from(rows) * u32::from(cell_height) > u32::from(u16::MAX)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid viewport",
            ));
        }
        self.runtime.manager.layout_panes(PaneArea {
            width: f32::from(cols) * f32::from(cell_width),
            height: f32::from(rows) * f32::from(cell_height),
            cell_width: f32::from(cell_width),
            cell_height: f32::from(cell_height),
        })?;
        self.viewport = [cols, rows];
        self.cell_pixels = [cell_width, cell_height];
        self.runtime.set_fallback_grid((cols, rows));
        self.runtime.sync_native_viewports();
        Ok(())
    }
}

fn applied(pane: Option<TerminalId>, tab: Option<crate::terminal_manager::TabId>) -> Reply {
    Reply::Applied {
        pane: pane.map(|id| id.raw() as u64),
        tab: tab.map(|id| id.raw() as u64),
        warning: None,
    }
}

fn spawn_receipt(
    pane: Option<TerminalId>,
    tab: crate::terminal_manager::TabId,
    layout: io::Result<()>,
) -> Reply {
    Reply::Applied {
        pane: pane.map(|id| id.raw() as u64),
        tab: Some(tab.raw() as u64),
        warning: layout.err().map(|error| {
            format!("Child started, but layout resize was not fully applied: {error}")
        }),
    }
}
fn pane_id(id: u64) -> io::Result<TerminalId> {
    usize::try_from(id)
        .map(TerminalId::new)
        .map_err(|_| missing_pane())
}
fn missing_pane() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "pane not found")
}

/// Serve until an authenticated owner explicitly terminates the host.
/// Connection loss and an empty runtime never terminate it.
pub fn serve(
    mut host: HostState,
    mut server: Server,
    mut authority: Authority,
    mut control: super::control::HostControl,
) -> io::Result<()> {
    server.write_admin_token(&authority.admin_token())?;
    let mut stopping = None;
    loop {
        let now = Instant::now();
        for client in authority.expire(now) {
            server.disconnect(client);
        }
        let events = server.poll(now)?;
        for client in events.disconnected {
            authority.disconnect(client);
        }
        let observed = host.step(now, authority.authenticated_clients() != 0)?;
        control.poll(&mut host, &observed)?;
        host.runtime.cleanup_exited();
        for (client, request) in events.requests {
            if stopping.is_some() || !server.is_connected(client) {
                continue;
            }
            let request_time = Instant::now();
            let response = match authority.process(client, &request, request_time) {
                Decision::Reply(response) => response,
                Decision::Execute { command, .. } => {
                    let reply = if matches!(command, Command::Terminate) {
                        stopping = Some(Instant::now() + Duration::from_millis(250));
                        Reply::Ok
                    } else {
                        host.execute(&command)
                    };
                    authority.complete(client, &request, reply, Instant::now())
                }
            };
            match server.send(client, &response) {
                Ok(true) => {}
                Ok(false) => authority.disconnect(client),
                Err(error) => {
                    // An oversized response must not stop the owner or leave
                    // the requesting client waiting for an answer.
                    let mut refusal = response;
                    refusal.reply = Reply::error(ErrorCode::Capacity, error.to_string());
                    if !server.send(client, &refusal).unwrap_or(false) {
                        server.disconnect(client);
                        authority.disconnect(client);
                    }
                }
            }
        }
        if stopping.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(());
        }
        host.runtime.wait(Duration::from_millis(10))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_manager::TerminalManager;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use ultty_protocol::{
        AppMessage, Constraints, Content, Encode, FrameId, RegionId, Style, TextContent,
        CLIENT_DEFAULT_CAPABILITIES, PROTOCOL_VERSION,
    };

    #[test]
    fn spawned_child_identity_survives_post_launch_resize_failure() {
        let reply = spawn_receipt(
            Some(TerminalId::new(4)),
            crate::terminal_manager::TabId::new(2),
            Err(io::Error::other("resize failed")),
        );
        assert!(
            matches!(reply, Reply::Applied { pane: Some(4), tab: Some(2), warning: Some(message) } if message.contains("Child started") && message.contains("resize failed"))
        );
    }

    #[test]
    fn detached_snapshots_preserve_hidden_native_sessions_and_updates() {
        let dir = tempfile::tempdir().unwrap();
        let server = crate::protocol::server::ProtocolServer::new(dir.path()).unwrap();
        let runtime =
            SessionRuntime::new(TerminalManager::empty(80, 24), Some(server), (80, 24)).unwrap();
        let owner = OwnerRef {
            pid: std::process::id(),
            epoch: "ef".repeat(32),
        };
        let mut host = HostState::new(runtime, owner, [80, 24]).unwrap();
        let mut peers = Vec::new();
        let mut panes = Vec::new();
        for value in ["first", "second"] {
            let (pane, tab) = match host.execute(&Command::Spawn {
                program: "/bin/sleep".into(),
                args: vec!["30".into()],
                cwd: None,
            }) {
                Reply::Applied {
                    pane: Some(pane),
                    tab: Some(tab),
                    ..
                } => (pane, tab),
                other => panic!("unexpected spawn response: {other:?}"),
            };
            let path = host
                .runtime
                .protocol
                .as_ref()
                .unwrap()
                .tab_socket_path(crate::terminal_manager::TabId::new(tab as usize))
                .unwrap();
            let mut peer = UnixStream::connect(path).unwrap();
            for message in [
                AppMessage::ClientHello {
                    version: PROTOCOL_VERSION,
                    requested_capabilities: CLIENT_DEFAULT_CAPABILITIES,
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
                AppMessage::SetContent {
                    id: RegionId::new(1),
                    content: Content::Text(TextContent::new(value)),
                },
                AppMessage::EndFrame {
                    frame_id: FrameId(1),
                },
            ] {
                peer.write_all(&message.encode_to_bytes()).unwrap();
            }
            peers.push(peer);
            panes.push(pane);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            host.step(Instant::now(), false).unwrap();
            if host
                .runtime
                .protocol
                .as_ref()
                .unwrap()
                .sessions()
                .filter(|session| session.is_confirmed() && session.pane().is_some())
                .count()
                == 2
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "both native sessions did not bind"
            );
            host.runtime.wait(Duration::from_millis(20)).unwrap();
        }
        let snapshot = match host.execute(&Command::Snapshot {
            after_sequence: None,
        }) {
            Reply::Snapshot { snapshot } => snapshot,
            other => panic!("unexpected snapshot: {other:?}"),
        };
        assert_eq!(host.runtime.protocol.as_ref().unwrap().active_tab(), None);
        let ids: Vec<_> = panes
            .iter()
            .map(|id| {
                let pane = snapshot.panes.iter().find(|pane| pane.id == *id).unwrap();
                assert_eq!(pane.native.len(), 1);
                pane.native[0].session_id
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
        for message in [
            AppMessage::BeginFrame {
                frame_id: FrameId(2),
            },
            AppMessage::SetContent {
                id: RegionId::new(1),
                content: Content::Text(TextContent::new("hidden-updated")),
            },
            AppMessage::EndFrame {
                frame_id: FrameId(2),
            },
        ] {
            peers[0].write_all(&message.encode_to_bytes()).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            host.step(Instant::now(), false).unwrap();
            if let Reply::Snapshot { snapshot: next } = host.execute(&Command::Snapshot {
                after_sequence: Some(snapshot.sequence),
            }) {
                let hidden = next.panes.iter().find(|pane| pane.id == panes[0]).unwrap();
                let visible = next.panes.iter().find(|pane| pane.id == panes[1]).unwrap();
                assert_eq!(hidden.native[0].session_id, ids[0]);
                assert_eq!(visible.native[0].session_id, ids[1]);
                if hidden.native[0].regions.iter().any(|region| matches!(&region.text, Some(crate::attach::native::NativeTextSnapshot::Plain(text)) if text == "hidden-updated")) {
                    assert!(next.sequence > snapshot.sequence);
                    assert!(matches!(host.execute(&Command::Snapshot { after_sequence: Some(next.sequence) }), Reply::Unchanged { sequence } if sequence == next.sequence));
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "hidden native update did not invalidate snapshot"
            );
            host.runtime.wait(Duration::from_millis(20)).unwrap();
        }
    }

    #[test]
    fn host_keeps_owner_alive_after_last_pane_and_can_spawn_again() {
        let runtime = SessionRuntime::new(TerminalManager::empty(80, 24), None, (80, 24)).unwrap();
        let owner = OwnerRef {
            pid: std::process::id(),
            epoch: "ab".repeat(32),
        };
        let mut host = HostState::new(runtime, owner.clone(), [80, 24]).unwrap();
        let launched = host.execute(&Command::Spawn {
            program: "/bin/sleep".into(),
            args: vec!["30".into()],
            cwd: None,
        });
        let pane = match launched {
            Reply::Applied {
                pane: Some(pane), ..
            } => pane,
            other => panic!("unexpected spawn result: {other:?}"),
        };
        let first = match host.execute(&Command::Snapshot {
            after_sequence: None,
        }) {
            Reply::Snapshot { snapshot } => snapshot,
            other => panic!("unexpected snapshot result: {other:?}"),
        };
        assert_eq!(first.owner, owner);
        assert_eq!(first.panes[0].id, pane);
        assert!(matches!(
            host.execute(&Command::Snapshot { after_sequence: Some(first.sequence) }),
            Reply::Unchanged { sequence } if sequence == first.sequence
        ));
        assert!(matches!(
            host.execute(&Command::Snapshot { after_sequence: None }),
            Reply::Snapshot { snapshot } if snapshot.sequence == first.sequence
        ));
        assert!(matches!(
            host.execute(&Command::Layout {
                command: LayoutCommand::Close { pane }
            }),
            Reply::Applied { .. }
        ));
        host.step(Instant::now(), false).unwrap();
        let empty = match host.execute(&Command::Snapshot {
            after_sequence: Some(first.sequence),
        }) {
            Reply::Snapshot { snapshot } => snapshot,
            other => panic!("unexpected empty result: {other:?}"),
        };
        assert_eq!(empty.owner, owner);
        assert!(empty.sequence > first.sequence);
        assert!(empty.panes.is_empty());
        assert!(matches!(
            host.execute(&Command::Spawn {
                program: "/bin/sleep".into(),
                args: vec!["30".into()],
                cwd: None
            }),
            Reply::Applied { pane: Some(_), .. }
        ));
    }

    #[test]
    fn snapshot_cache_invalidates_after_real_pty_output() {
        let runtime = SessionRuntime::new(TerminalManager::empty(80, 24), None, (80, 24)).unwrap();
        let owner = OwnerRef {
            pid: std::process::id(),
            epoch: "cd".repeat(32),
        };
        let mut host = HostState::new(runtime, owner, [80, 24]).unwrap();
        let pane = match host.execute(&Command::Spawn {
            program: "/bin/cat".into(),
            args: vec![],
            cwd: None,
        }) {
            Reply::Applied {
                pane: Some(pane), ..
            } => pane,
            other => panic!("unexpected spawn result: {other:?}"),
        };
        let first = match host.execute(&Command::Snapshot {
            after_sequence: None,
        }) {
            Reply::Snapshot { snapshot } => snapshot,
            other => panic!("unexpected snapshot result: {other:?}"),
        };
        assert!(matches!(
            host.execute(&Command::Input {
                pane,
                data: b"cache-change\n".to_vec()
            }),
            Reply::Applied { .. }
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            host.runtime.wait(Duration::from_millis(20)).unwrap();
            host.step(Instant::now(), false).unwrap();
            match host.execute(&Command::Snapshot {
                after_sequence: Some(first.sequence),
            }) {
                Reply::Snapshot { snapshot } if snapshot.sequence > first.sequence => break,
                Reply::Unchanged { .. } => {}
                other => panic!("unexpected snapshot result: {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "PTY output did not invalidate cached snapshot"
            );
        }
    }
}
