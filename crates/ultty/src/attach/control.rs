//! Local owner-admin automation for persistent sessions.
//!
//! This endpoint uses the existing control credential, independently of scoped
//! attachment grants and their controller lease. It is never relayed over SSH.

use std::io;
use std::time::{Duration, Instant};

use ultty_protocol::control::{
    ControlData, ControlRequest, ControlResponse, OwnerRef, TabInfo, VersionInfo, WaitCondition,
};

use super::host::HostState;
use super::wire::{Command, LayoutCommand, Reply};
use crate::control::auth::{
    control_request_requires_auth, read_auth_token, token_matches, write_auth_token,
};
use crate::control::common::{
    buffer_text, build_command_line, control_pane_snapshot, input_acceptance, job_id,
    job_is_running, key_to_bytes, pane_infos, process_job_response, program_status_response,
    resolve_control_target, search_response, send_pty_input, shell_command_response,
    submit_control_shell_command, terminal_info, terminal_working_directory,
};
use crate::control::events::{control_event, open_panes, pane_changes};
use crate::control::wait::{Condition, PendingControlWait, WaitMatcher};
use crate::control::ControlServer;
use crate::event_loop::{ShellCommandOutcome, TerminalEvent};
use crate::terminal_manager::TerminalId;

const MAX_PENDING: usize = 128;
const OPERATIONS: &[&str] = &[
    "describe",
    "with_owner",
    "with_token",
    "authenticate",
    "ping",
    "version",
    "batch",
    "list_panes",
    "list_tabs",
    "get_info",
    "get_text",
    "get_selection",
    "last_command",
    "program_status",
    "regions",
    "search",
    "send_text",
    "send_raw",
    "send_key",
    "new_tab",
    "split_pane",
    "close_tab",
    "switch_tab",
    "focus_pane",
    "zoom_pane",
    "swap_panes",
    "run_command",
    "run_command_and_wait",
    "job_status",
    "wait_job",
    "wait_for",
    "invoke_action",
    "subscribe",
    "subscribe_v2",
    "set_pane_status",
    "workspace_list",
    "workspace_save",
];

struct PendingAction {
    client: usize,
    session: u64,
    request: u32,
    expires: Instant,
}

/// Existing local control socket backed by a persistent runtime.
pub struct HostControl {
    server: ControlServer,
    waits: Vec<PendingControlWait>,
    actions: Vec<PendingAction>,
    panes: Vec<TerminalId>,
}

impl HostControl {
    /// Bind this process's control endpoint with the attachment owner's epoch.
    /// Create before launching children so inherited `ULTTY_PID` is usable.
    pub fn new(owner: OwnerRef) -> io::Result<Self> {
        let server = ControlServer::new_with_owner(owner)?;
        write_auth_token(server.token_path())?;
        Ok(Self {
            server,
            waits: Vec::new(),
            actions: Vec::new(),
            panes: Vec::new(),
        })
    }

    /// Publish observed events, complete bounded waits, and dispatch ready clients.
    /// Call after runtime processing and before exited-pane cleanup.
    pub fn poll(
        &mut self,
        host: &mut HostState,
        events: &[(TerminalId, Vec<TerminalEvent>)],
    ) -> io::Result<()> {
        self.server.poll(Some(Duration::ZERO))?;
        for (pane, events) in events {
            for event in events {
                if let Some(event) = control_event(*pane, event) {
                    self.server.publish(&event);
                }
            }
        }
        self.publish_panes(host);
        self.complete(host, events);
        for (client, request) in self.server.take_pending_requests() {
            if let Some(response) = self.dispatch(host, client, request, false) {
                self.server.send_response(client, &response);
            }
            self.publish_panes(host);
        }
        Ok(())
    }

    fn publish_panes(&mut self, host: &HostState) {
        let now = open_panes(&host.runtime.manager);
        for event in pane_changes(&self.panes, &now) {
            self.server.publish(&event);
        }
        self.panes = now.into_iter().map(|(pane, _)| pane).collect();
    }

    fn authenticate(&mut self, client: usize, token: &str) -> bool {
        let valid = read_auth_token(self.server.token_path())
            .is_ok_and(|expected| token_matches(token, &expected));
        self.server.set_client_authenticated(client, valid);
        valid
    }

    fn dispatch(
        &mut self,
        host: &mut HostState,
        client: usize,
        mut request: ControlRequest,
        batch: bool,
    ) -> Option<ControlResponse> {
        // Bound wrapper traversal independently of the serialized request limit.
        for depth in 0..=32 {
            if depth == 32 {
                return Some(error(4, "Control envelope nesting exceeds 32"));
            }
            match request {
                ControlRequest::WithToken {
                    token,
                    request: inner,
                } => {
                    if !self.authenticate(client, &token) {
                        return Some(error(403, "Invalid control token"));
                    }
                    request = *inner;
                }
                ControlRequest::WithOwner {
                    owner,
                    request: inner,
                } => {
                    if &owner != self.server.owner() {
                        return Some(error(
                            409,
                            "Control owner changed; rediscover explicitly before retrying",
                        ));
                    }
                    request = *inner;
                }
                _ => break,
            }
        }
        if control_request_requires_auth(&request) && !self.server.is_client_authenticated(client) {
            return Some(error(403, "Authentication required"));
        }
        if batch
            && matches!(
                request,
                ControlRequest::Batch { .. }
                    | ControlRequest::Subscribe { .. }
                    | ControlRequest::SubscribeV2 { .. }
                    | ControlRequest::WaitFor { .. }
                    | ControlRequest::WaitJob { .. }
                    | ControlRequest::RunCommandAndWait { .. }
                    | ControlRequest::InvokeAction { .. }
            )
        {
            return Some(error(
                5,
                "Streams, deferred requests and nested batches cannot be used in batches",
            ));
        }
        if matches!(
            request,
            ControlRequest::WaitFor { .. }
                | ControlRequest::WaitJob { .. }
                | ControlRequest::RunCommandAndWait { .. }
                | ControlRequest::InvokeAction { .. }
        ) && self.waits.len() + self.actions.len() >= MAX_PENDING
        {
            return Some(error(12, "Pending control capacity is full"));
        }
        let response = self.execute(host, client, request, batch);
        Some(match response {
            Ok(Some(response)) | Err(response) => response,
            Ok(None) => return None,
        })
    }

    fn execute(
        &mut self,
        host: &mut HostState,
        client: usize,
        request: ControlRequest,
        _batch: bool,
    ) -> Result<Option<ControlResponse>, ControlResponse> {
        let response = match request {
            ControlRequest::Ping => ControlResponse::Data(ControlData::Pong),
            ControlRequest::Version => ControlResponse::Data(ControlData::Version(VersionInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                protocol_version: ultty_protocol::PROTOCOL_VERSION,
                git_hash: option_env!("GIT_HASH").map(String::from),
            })),
            ControlRequest::Describe => self.server.description_with_operations(OPERATIONS),
            ControlRequest::Authenticate { token } => {
                if !self.authenticate(client, &token) {
                    return Err(error(403, "Invalid control token"));
                }
                ControlResponse::Data(ControlData::Authenticated)
            }
            ControlRequest::Batch { commands } => {
                ControlResponse::Data(ControlData::BatchResults {
                    results: commands
                        .into_iter()
                        .map(|request| {
                            self.dispatch(host, client, request, true)
                                .unwrap_or_else(|| error(5, "Deferred batch member"))
                        })
                        .collect(),
                })
            }
            ControlRequest::ListPanes => ControlResponse::Data(ControlData::Panes {
                panes: pane_infos(&host.runtime.manager),
            }),
            ControlRequest::ListTabs => {
                let manager = &host.runtime.manager;
                let active = manager.active_tab().map(|tab| tab.id);
                ControlResponse::Data(ControlData::Tabs {
                    tabs: manager
                        .tabs()
                        .iter()
                        .enumerate()
                        .map(|(index, tab)| TabInfo {
                            index,
                            title: manager.tab_display_title(tab.id),
                            active: Some(tab.id) == active,
                            pid: manager
                                .terminal(tab.focused_terminal)
                                .map(|terminal| terminal.event_loop.child_pid()),
                            cwd: manager
                                .terminal(tab.focused_terminal)
                                .and_then(terminal_working_directory),
                        })
                        .collect(),
                })
            }
            ControlRequest::GetInfo => {
                let manager = &host.runtime.manager;
                let terminal = manager
                    .focused_terminal()
                    .ok_or_else(|| error(1, "No active terminal"))?;
                let index = manager
                    .tabs()
                    .iter()
                    .position(|tab| Some(tab.id) == manager.active_tab().map(|tab| tab.id))
                    .unwrap_or(0);
                ControlResponse::Data(ControlData::Info(terminal_info(
                    terminal,
                    index,
                    manager.tab_count(),
                )))
            }
            ControlRequest::GetText {
                include_scrollback,
                scrollback_lines,
                target,
            } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let terminal = host
                    .runtime
                    .manager
                    .terminal(pane)
                    .ok_or_else(|| error(6, "Pane closed"))?;
                let history = if include_scrollback {
                    scrollback_lines.unwrap_or(usize::MAX)
                } else {
                    0
                };
                ControlResponse::Text {
                    text: buffer_text(terminal.event_loop.terminal().buffer(), history),
                }
            }
            ControlRequest::GetSelection { target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                ControlResponse::Data(ControlData::Selection {
                    text: host
                        .runtime
                        .manager
                        .terminal(pane)
                        .and_then(|terminal| terminal.event_loop.terminal().selected_text()),
                })
            }
            ControlRequest::ProgramStatus { target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                program_status_response(&host.runtime.manager, pane)
            }
            ControlRequest::LastCommand { tail_lines, target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let last = host
                    .runtime
                    .manager
                    .terminal(pane)
                    .and_then(|terminal| {
                        terminal.event_loop.terminal().last_command(
                            tail_lines.map(|lines| lines.min(usize::MAX as u64) as usize),
                        )
                    })
                    .ok_or_else(|| {
                        error(
                            7,
                            "No command recorded; OSC 133 shell integration is required",
                        )
                    })?;
                ControlResponse::Data(ControlData::LastCommand {
                    command: last.command,
                    output: last.output,
                    exit_code: last.exit_code,
                    running: last.running,
                    output_available: last.output_available,
                })
            }
            ControlRequest::Regions { target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let tab = host
                    .runtime
                    .manager
                    .terminal(pane)
                    .map(|terminal| terminal.tab_id);
                let scenes = host
                    .runtime
                    .protocol
                    .as_ref()
                    .map(|server| {
                        server
                            .sessions()
                            .filter(|session| {
                                Some(session.tab_id()) == tab
                                    && session.pane().is_none_or(|id| id == pane)
                            })
                            .map(|session| session.describe())
                            .collect()
                    })
                    .unwrap_or_default();
                ControlResponse::Data(ControlData::Regions { scenes })
            }
            ControlRequest::SendRaw { data, target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                send_pty_input(&mut host.runtime.manager, pane, &data)
            }
            ControlRequest::SendText { text, target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let terminal = host
                    .runtime
                    .manager
                    .terminal(pane)
                    .ok_or_else(|| error(6, "Pane closed"))?;
                let foreground = terminal.event_loop.foreground_pgrp();
                let tab = terminal.tab_id;
                if let Some(server) = host.runtime.protocol.as_mut() {
                    if server.has_pane_key_subscribers(tab, pane, foreground) {
                        return Ok(Some(input_acceptance(
                            server.send_text_input_to_pane(tab, pane, text, foreground),
                        )));
                    }
                }
                send_pty_input(&mut host.runtime.manager, pane, text.as_bytes())
            }
            ControlRequest::SendKey {
                key,
                modifiers,
                target,
            } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let terminal = host
                    .runtime
                    .manager
                    .terminal(pane)
                    .ok_or_else(|| error(6, "Pane closed"))?;
                let foreground = terminal.event_loop.foreground_pgrp();
                let tab = terminal.tab_id;
                if let Some(server) = host.runtime.protocol.as_mut() {
                    if server.has_pane_key_subscribers(tab, pane, foreground) {
                        return Ok(Some(input_acceptance(
                            server.send_key_to_pane(tab, pane, key, modifiers, foreground),
                        )));
                    }
                }
                let terminal = host
                    .runtime
                    .manager
                    .terminal_mut(pane)
                    .ok_or_else(|| error(6, "Pane closed"))?;
                let app_cursor = terminal
                    .event_loop
                    .terminal()
                    .modes()
                    .application_cursor_keys;
                let bytes = key_to_bytes(
                    &key,
                    modifiers.contains(ultty_protocol::Modifiers::CTRL),
                    modifiers.contains(ultty_protocol::Modifiers::ALT),
                    app_cursor,
                );
                if bytes.is_empty() {
                    return Err(error(4, "This key has no supported PTY encoding"));
                }
                input_acceptance(terminal.event_loop.send_input(&bytes))
            }
            ControlRequest::RunCommand {
                command,
                args,
                target,
            } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                send_pty_input(
                    &mut host.runtime.manager,
                    pane,
                    build_command_line(&command, &args).as_bytes(),
                )
            }
            ControlRequest::NewTab {
                title,
                command,
                args,
                working_dir,
            } => {
                validate_spawn(&command, &args, &working_dir)?;
                let program = command.unwrap_or_else(crate::pty::default_shell);
                let reply = host.execute(&Command::Spawn {
                    program,
                    args,
                    cwd: working_dir,
                });
                let (pane, tab_index, warning) = spawned(host, reply)?;
                if let Some(title) = title {
                    let tab = host.runtime.manager.tabs()[tab_index].id;
                    host.runtime.manager.set_tab_title(tab, &title);
                }
                ControlResponse::Data(ControlData::TabOpened {
                    pane,
                    tab_index,
                    warning,
                })
            }
            ControlRequest::SplitPane {
                direction,
                command,
                args,
                working_dir,
                target,
            } => {
                validate_spawn(&command, &args, &working_dir)?;
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let reply = host.execute(&Command::Layout {
                    command: LayoutCommand::Split {
                        pane: pane.raw() as u64,
                        direction,
                        program: command,
                        args,
                        cwd: working_dir,
                    },
                });
                let (pane, tab_index, warning) = spawned(host, reply)?;
                ControlResponse::Data(ControlData::PaneOpened {
                    pane,
                    tab_index,
                    warning,
                })
            }
            ControlRequest::FocusPane { target } => {
                layout(host, LayoutCommand::Focus { pane: target })?
            }
            ControlRequest::ZoomPane { target, zoomed } => layout(
                host,
                LayoutCommand::Zoom {
                    pane: target,
                    zoomed,
                },
            )?,
            ControlRequest::SwapPanes { first, second } => {
                layout(host, LayoutCommand::Swap { first, second })?
            }
            ControlRequest::SwitchTab { index } => {
                let pane = host
                    .runtime
                    .manager
                    .tabs()
                    .get(index)
                    .map(|tab| tab.focused_terminal)
                    .ok_or_else(|| error(6, "No tab at this index"))?;
                layout(
                    host,
                    LayoutCommand::Focus {
                        pane: pane.raw() as u64,
                    },
                )?
            }
            ControlRequest::CloseTab { index } => {
                let tab = index
                    .and_then(|index| host.runtime.manager.tabs().get(index).map(|tab| tab.id))
                    .or_else(|| {
                        if index.is_none() {
                            host.runtime.manager.active_tab().map(|tab| tab.id)
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| error(6, "No tab at this index"))?;
                host.runtime.manager.close_tab(tab);
                host.runtime.sync_tabs();
                ControlResponse::Ok
            }
            ControlRequest::SetPaneStatus { status, target } => {
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                host.runtime
                    .manager
                    .terminal_mut(pane)
                    .ok_or_else(|| error(6, "Pane closed"))?
                    .status = status.map(|status| status.sanitized());
                ControlResponse::Ok
            }
            ControlRequest::WorkspaceList => ControlResponse::Data(ControlData::Workspaces {
                names: crate::session::SessionData::workspace_names()
                    .map_err(|err| error(2, err.to_string()))?,
            }),
            ControlRequest::WorkspaceSave { name } => {
                host.runtime
                    .manager
                    .to_session_data()
                    .save_named(&name)
                    .map_err(|err| error(2, err.to_string()))?;
                ControlResponse::Ok
            }
            ControlRequest::JobStatus { job } => {
                process_job_response(&host.runtime.manager, job_id(job)?)
            }
            ControlRequest::WaitJob { job, timeout_ms } => {
                let timeout = timeout(timeout_ms)?;
                let pane = job_id(job)?;
                let response = process_job_response(&host.runtime.manager, pane);
                if !job_is_running(&response) {
                    return Ok(Some(response));
                }
                self.waits.push(PendingControlWait::new(
                    client,
                    pane,
                    Condition::ProcessJob,
                    timeout,
                    Instant::now(),
                ));
                return Ok(None);
            }
            ControlRequest::RunCommandAndWait {
                command,
                timeout_ms,
                target,
            } => {
                let timeout = timeout(timeout_ms)?;
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let sequence =
                    submit_control_shell_command(&mut host.runtime.manager, pane, &command)?;
                self.waits.push(PendingControlWait::new(
                    client,
                    pane,
                    Condition::ShellCommand { sequence },
                    timeout,
                    Instant::now(),
                ));
                return Ok(None);
            }
            ControlRequest::WaitFor {
                pattern,
                regex,
                timeout_ms,
                until,
                target,
            } => {
                let timeout = timeout(timeout_ms)?;
                let pane = resolve_control_target(&host.runtime.manager, target)?;
                let now = Instant::now();
                let condition = match until {
                    WaitCondition::Text => Condition::Text(
                        WaitMatcher::new(&pattern, regex).map_err(|err| error(4, err))?,
                    ),
                    WaitCondition::CommandFinished => Condition::CommandFinished,
                    WaitCondition::Bell => Condition::Bell,
                    WaitCondition::Quiet { ms } => Condition::quiet(self::timeout(ms)?, now),
                };
                let wait = PendingControlWait::new(client, pane, condition, timeout, now);
                if let Some(response) = wait_response(host, &wait, now) {
                    return Ok(Some(response));
                }
                self.waits.push(wait);
                return Ok(None);
            }
            ControlRequest::Subscribe { target } => {
                if target.is_some() {
                    resolve_control_target(&host.runtime.manager, target)?;
                }
                self.server.start_subscription(client, target);
                return Ok(None);
            }
            ControlRequest::SubscribeV2 { cursor, target } => {
                if cursor.is_none() && target.is_some() {
                    resolve_control_target(&host.runtime.manager, target)?;
                }
                let snapshot = if cursor.is_none() {
                    control_pane_snapshot(&host.runtime.manager, target)
                } else {
                    Vec::new()
                };
                self.server
                    .start_subscription_v2(client, target, cursor, snapshot);
                return Ok(None);
            }
            ControlRequest::InvokeAction {
                target,
                session,
                expected_generation,
                region,
                widget_id,
                action,
            } => {
                if !ultty_protocol::valid_action_widget_id(&widget_id) {
                    return Err(error(4, "Invalid semantic widget identity"));
                }
                let pane = resolve_control_target(&host.runtime.manager, Some(target))?;
                let server = host
                    .runtime
                    .protocol
                    .as_mut()
                    .ok_or_else(|| error(8, "No native protocol server"))?;
                let request = server
                    .request_action(
                        session,
                        pane,
                        expected_generation,
                        ultty_protocol::RegionId(region),
                        &widget_id,
                        action,
                    )
                    .map_err(|err| {
                        error(
                            if err.kind() == io::ErrorKind::WouldBlock {
                                12
                            } else {
                                15
                            },
                            format!(
                                "Semantic action dispatch failed: {err}; reinspect before retrying"
                            ),
                        )
                    })?;
                self.actions.push(PendingAction {
                    client,
                    session,
                    request,
                    expires: Instant::now() + Duration::from_secs(5),
                });
                return Ok(None);
            }
            ControlRequest::Search {
                pattern,
                regex,
                case_sensitive,
                include_scrollback,
                target,
            } => search_response(
                &host.runtime.manager,
                pattern,
                regex,
                case_sensitive,
                include_scrollback,
                target,
            ),
            // Window operations and live override relaying require a GUI adapter.
            _ => {
                return Err(error(
                    5,
                    "Operation unsupported by persistent host; inspect Describe",
                ))
            }
        };
        Ok(Some(response))
    }

    fn complete(&mut self, host: &mut HostState, events: &[(TerminalId, Vec<TerminalEvent>)]) {
        let now = Instant::now();
        for mut wait in std::mem::take(&mut self.waits) {
            if !self.server.has_client(wait.client_key()) {
                continue;
            }
            let mut response = None;
            let waiting_pane = wait.pane();
            for (_, events) in events.iter().filter(|(pane, _)| *pane == waiting_pane) {
                for event in events {
                    if wait.observe(event, now) {
                        response = if let TerminalEvent::ShellCommandFinished {
                            sequence,
                            exit_code,
                        } = event
                        {
                            Some(shell_command_response(
                                wait.pane(),
                                *sequence,
                                ShellCommandOutcome::Finished {
                                    exit_code: *exit_code,
                                },
                            ))
                        } else {
                            control_event(wait.pane(), event).map(|event| {
                                ControlResponse::Data(ControlData::WaitEvent { event })
                            })
                        };
                        if response.is_some() {
                            break;
                        }
                    }
                }
            }
            if let Some(response) = response.or_else(|| wait_response(host, &wait, now)) {
                self.server.send_response(wait.client_key(), &response);
            } else {
                self.waits.push(wait);
            }
        }
        let results = host
            .runtime
            .protocol
            .as_mut()
            .map(|server| server.take_action_results())
            .unwrap_or_default();
        for action in std::mem::take(&mut self.actions) {
            if !self.server.has_client(action.client) {
                if let Some(server) = host.runtime.protocol.as_mut() {
                    server.cancel_action(action.session, action.request);
                }
                continue;
            }
            let response = if let Some((_, _, refusal)) =
                results.iter().find(|(session, request, _)| {
                    *session == action.session && *request == action.request
                }) {
                Some(match refusal {
                    Some(message) => error(
                        15,
                        format!("Application refused semantic action: {message}"),
                    ),
                    None => ControlResponse::Data(ControlData::ActionApplied {
                        session: action.session,
                        request_id: action.request,
                    }),
                })
            } else if now >= action.expires
                || !host
                    .runtime
                    .protocol
                    .as_ref()
                    .is_some_and(|server| server.has_session_id(action.session))
            {
                Some(error(15, "Semantic action outcome unknown; it may have applied. Reinspect before retrying"))
            } else {
                None
            };
            if let Some(response) = response {
                if let Some(server) = host.runtime.protocol.as_mut() {
                    server.cancel_action(action.session, action.request);
                }
                self.server.send_response(action.client, &response);
            } else {
                self.actions.push(action);
            }
        }
    }
}

fn wait_response(
    host: &HostState,
    wait: &PendingControlWait,
    now: Instant,
) -> Option<ControlResponse> {
    if wait.is_process_job() {
        let response = process_job_response(&host.runtime.manager, wait.pane());
        if !job_is_running(&response) {
            return Some(response);
        }
    } else {
        let Some(terminal) = host.runtime.manager.terminal(wait.pane()) else {
            return Some(error(6, "Pane closed while waiting"));
        };
        if let Some(sequence) = wait.command_sequence() {
            if let Some(outcome) = terminal.event_loop.shell_command_result(sequence) {
                return Some(shell_command_response(wait.pane(), sequence, outcome));
            }
        }
        if let Some(matcher) = wait.matcher() {
            let terminal = terminal.event_loop.terminal();
            let buffer = terminal.buffer();
            for line in 0..buffer.total_lines() {
                if let Some(col) = buffer
                    .row_at(line)
                    .and_then(|row| matcher.find_in(&row.text_trimmed()))
                {
                    return Some(ControlResponse::Data(ControlData::WaitMatched {
                        line,
                        col,
                    }));
                }
            }
        }
        if wait.is_quiet(now) {
            return Some(ControlResponse::Data(ControlData::WaitQuiet));
        }
    }
    wait.is_expired(now)
        .then_some(ControlResponse::Data(ControlData::WaitTimeout))
}

fn timeout(ms: u64) -> Result<Duration, ControlResponse> {
    if ms == 0 || ms > 86_400_000 {
        return Err(error(4, "Timeout must be between 1 and 86400000 ms"));
    }
    Ok(Duration::from_millis(ms))
}

fn error(code: u16, message: impl Into<String>) -> ControlResponse {
    ControlResponse::Error {
        code,
        message: message.into(),
    }
}

fn validate_spawn(
    command: &Option<String>,
    args: &[String],
    cwd: &Option<String>,
) -> Result<(), ControlResponse> {
    if command.is_none() && (!args.is_empty() || cwd.is_some()) {
        return Err(error(4, "args and working_dir require an explicit command"));
    }
    Ok(())
}

fn layout(
    host: &mut HostState,
    command: LayoutCommand,
) -> Result<ControlResponse, ControlResponse> {
    match host.execute(&Command::Layout { command }) {
        Reply::Error { message, .. } => Err(error(2, message)),
        _ => Ok(ControlResponse::Ok),
    }
}

fn spawned(
    host: &HostState,
    reply: Reply,
) -> Result<(u64, usize, Option<String>), ControlResponse> {
    match reply {
        Reply::Applied {
            pane: Some(pane),
            warning,
            ..
        } => host
            .runtime
            .manager
            .tabs()
            .iter()
            .position(|tab| tab.terminals.iter().any(|id| id.raw() as u64 == pane))
            .map(|index| (pane, index, warning))
            .ok_or_else(|| error(2, "Spawned pane no longer exists")),
        Reply::Error { message, .. } => Err(error(2, message)),
        _ => Err(error(2, "Spawn returned no pane identity")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    use super::*;
    use crate::runtime::SessionRuntime;
    use crate::terminal_manager::TerminalManager;
    use ultty_protocol::control::{ControlEnvelope, OutputFormat};

    fn exchange(
        bridge: &mut HostControl,
        host: &mut HostState,
        request: ControlRequest,
    ) -> ControlResponse {
        let mut stream = UnixStream::connect(bridge.server.socket_path()).unwrap();
        let command = serde_json::to_vec(&ControlEnvelope {
            output_format: OutputFormat::Json,
            request,
        })
        .unwrap();
        stream.write_all(&command).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        loop {
            let events = host.step(Instant::now(), false).unwrap();
            bridge.poll(host, &events).unwrap();
            host.runtime.cleanup_exited();
            let mut part = [0u8; 4096];
            match stream.read(&mut part) {
                Ok(0) => panic!("control stream closed before response"),
                Ok(len) => bytes.extend_from_slice(&part[..len]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("control read failed: {error}"),
            }
            if bytes.contains(&b'\n') {
                return serde_json::from_slice(&bytes).unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "control response deadline elapsed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn test_host_control_real_dispatch_owner_capabilities_and_retained_job() {
        let owner = super::super::auth::new_owner().unwrap();
        let runtime = SessionRuntime::new(TerminalManager::empty(80, 24), None, (80, 24)).unwrap();
        let mut host = HostState::new(runtime, owner.clone(), [80, 24]).unwrap();
        let mut bridge = HostControl::new(owner.clone()).unwrap();
        let token = read_auth_token(bridge.server.token_path()).unwrap();
        let authenticated = |request| ControlRequest::WithToken {
            token: token.clone(),
            request: Box::new(ControlRequest::WithOwner {
                owner: owner.clone(),
                request: Box::new(request),
            }),
        };
        assert!(matches!(
            exchange(&mut bridge, &mut host, ControlRequest::Describe),
            ControlResponse::Error { code: 403, .. }
        ));
        let description = exchange(
            &mut bridge,
            &mut host,
            authenticated(ControlRequest::Describe),
        );
        let ControlResponse::Data(ControlData::Description {
            owner: discovered,
            supported_operations,
            ..
        }) = description
        else {
            panic!("missing description");
        };
        assert_eq!(discovered, owner);
        assert!(supported_operations
            .iter()
            .any(|operation| operation == "invoke_action"));
        assert!(!supported_operations
            .iter()
            .any(|operation| operation == "screenshot"));
        let wrong = ControlRequest::WithToken {
            token: token.clone(),
            request: Box::new(ControlRequest::WithOwner {
                owner: OwnerRef {
                    epoch: "wrong".into(),
                    ..owner.clone()
                },
                request: Box::new(ControlRequest::NewTab {
                    title: None,
                    command: Some("/bin/true".into()),
                    args: Vec::new(),
                    working_dir: None,
                }),
            }),
        };
        assert!(matches!(
            exchange(&mut bridge, &mut host, wrong),
            ControlResponse::Error { code: 409, .. }
        ));
        assert_eq!(host.runtime.manager.tab_count(), 0);
        let opened = exchange(
            &mut bridge,
            &mut host,
            authenticated(ControlRequest::NewTab {
                title: Some("instant".into()),
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 7".into()],
                working_dir: None,
            }),
        );
        let ControlResponse::Data(ControlData::TabOpened { pane, .. }) = opened else {
            panic!("spawn failed: {opened:?}");
        };
        let response = exchange(
            &mut bridge,
            &mut host,
            authenticated(ControlRequest::WaitJob {
                job: pane,
                timeout_ms: 1000,
            }),
        );
        assert!(
            matches!(response, ControlResponse::Data(ControlData::ProcessJob { job, outcome: ultty_protocol::control::ProcessJobOutcome::Exited { status: 7 }, .. }) if job == pane)
        );
        let repeated = exchange(
            &mut bridge,
            &mut host,
            authenticated(ControlRequest::JobStatus { job: pane }),
        );
        assert_eq!(response, repeated);
        assert!(matches!(
            exchange(
                &mut bridge,
                &mut host,
                authenticated(ControlRequest::Screenshot { path: None })
            ),
            ControlResponse::Error { code: 5, .. }
        ));
    }
}
