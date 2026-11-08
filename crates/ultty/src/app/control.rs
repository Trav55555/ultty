//! Control server request handling.
//!
//! Processes CLI automation requests from ulttyctl and other tools.

use super::control_wait::{find_ignore_case, Condition, PendingControlWait, WaitMatcher};
use super::App;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};
use ultty::terminal_manager::TerminalId;
use winit::dpi::PhysicalSize;

/// Number of random bytes in a freshly generated control auth token.
const AUTH_TOKEN_BYTES: usize = 32;

impl App {
    pub(super) fn handle_control_requests(&mut self) {
        use ultty_protocol::control::{ControlRequest, ControlResponse};

        let requests = {
            let Some(ref mut server) = self.control_server else {
                return;
            };
            server.take_pending_requests()
        };

        let mut responses: Vec<(usize, ControlResponse)> = Vec::new();
        let mut subscriptions: Vec<(usize, Option<u64>)> = Vec::new();

        for (client_key, request) in requests {
            // `WithToken` authenticates the connection and then behaves
            // exactly like its inner request (including wait-for deferral).
            let request = match request {
                ControlRequest::WithToken { token, request } => {
                    if let Err(response) = self.authenticate_client(client_key, &token) {
                        responses.push((client_key, response));
                        continue;
                    }
                    *request
                }
                other => other,
            };

            if let ControlRequest::Subscribe { target } = request {
                match self.subscription_target(client_key, &request, target) {
                    Ok(target) => subscriptions.push((client_key, target)),
                    Err(response) => responses.push((client_key, response)),
                }
                continue;
            }

            if let Some(response) = self.defer_wait_for_request(client_key, &request) {
                responses.push((client_key, response));
                continue;
            }

            if self.is_wait_for_request(&request) {
                continue;
            }

            let response = self.process_control_request(client_key, request);
            responses.push((client_key, response));
        }

        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
            for (client_key, target) in subscriptions {
                server.start_subscription(client_key, target);
            }
        }
    }

    /// Check a `subscribe` request: it needs the token, and a `target` must
    /// name an open pane. Returns the pane filter to subscribe with.
    fn subscription_target(
        &self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
        target: Option<u64>,
    ) -> Result<Option<u64>, ultty_protocol::control::ControlResponse> {
        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Err(response);
        }
        if target.is_some() {
            resolve_control_target(&self.terminal_manager, target)?;
        }
        Ok(target)
    }

    /// The earliest instant a pending wait needs the UI awake for.
    pub(super) fn next_control_wait_deadline(&self) -> Option<Instant> {
        self.pending_control_waits
            .iter()
            .map(PendingControlWait::deadline)
            .min()
    }

    /// Answer the waits on `pane` that were waiting for `event`, and restart
    /// quiet periods on output.
    pub(super) fn observe_control_wait_event(
        &mut self,
        pane: TerminalId,
        event: &ultty::event_loop::TerminalEvent,
    ) {
        use ultty_protocol::control::{ControlData, ControlResponse};

        if self.pending_control_waits.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut answered = Vec::new();
        self.pending_control_waits.retain_mut(|wait| {
            if wait.pane() != pane || !wait.observe(event, now) {
                return true;
            }
            if let Some(event) = super::control_events::control_event(pane, event) {
                answered.push((
                    wait.client_key(),
                    ControlResponse::Data(ControlData::WaitEvent { event }),
                ));
            }
            false
        });
        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in answered {
                server.send_response(client_key, &response);
            }
        }
    }

    pub(super) fn complete_pending_control_waits(&mut self) {
        let mut still_pending = Vec::new();
        let mut responses = Vec::new();
        let pending = std::mem::take(&mut self.pending_control_waits);

        for wait in pending {
            if let Some(response) = self.pending_wait_response(&wait) {
                responses.push((wait.client_key(), response));
            } else {
                still_pending.push(wait);
            }
        }

        self.pending_control_waits = still_pending;

        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
        }
    }

    fn is_wait_for_request(&self, request: &ultty_protocol::control::ControlRequest) -> bool {
        matches!(
            request,
            ultty_protocol::control::ControlRequest::WaitFor { .. }
        )
    }

    fn defer_wait_for_request(
        &mut self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{
            ControlData, ControlRequest, ControlResponse, WaitCondition,
        };

        let ControlRequest::WaitFor {
            pattern,
            regex: use_regex,
            timeout_ms,
            until,
            target,
        } = request
        else {
            return None;
        };

        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Some(response);
        }

        let pane = match resolve_control_target(&self.terminal_manager, *target) {
            Ok(pane) => pane,
            Err(response) => return Some(response),
        };

        let now = Instant::now();
        let condition = match until {
            WaitCondition::Text => {
                if pattern.is_empty() {
                    return Some(ControlResponse::Error {
                        code: 4,
                        message: "wait-for needs a pattern or an event to wait for".to_string(),
                    });
                }
                let matcher = match WaitMatcher::new(pattern, *use_regex) {
                    Ok(matcher) => matcher,
                    Err(message) => return Some(ControlResponse::Error { code: 4, message }),
                };
                if let Some((line, col)) = self.find_wait_match(pane, &matcher) {
                    return Some(ControlResponse::Data(ControlData::WaitMatched {
                        line,
                        col,
                    }));
                }
                Condition::Text(matcher)
            }
            WaitCondition::CommandFinished => Condition::CommandFinished,
            WaitCondition::Bell => Condition::Bell,
            WaitCondition::Quiet { ms } => Condition::quiet(Duration::from_millis(*ms), now),
        };

        self.pending_control_waits.push(PendingControlWait::new(
            client_key,
            pane,
            condition,
            Duration::from_millis(*timeout_ms),
            now,
        ));
        None
    }

    fn pending_wait_response(
        &self,
        wait: &PendingControlWait,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{ControlData, ControlResponse};

        if self.terminal_manager.terminal(wait.pane()).is_none() {
            return Some(ControlResponse::Error {
                code: PANE_NOT_FOUND,
                message: format!("Pane {} closed while waiting", wait.pane().raw()),
            });
        }

        if let Some((line, col)) = wait
            .matcher()
            .and_then(|matcher| self.find_wait_match(wait.pane(), matcher))
        {
            return Some(ControlResponse::Data(ControlData::WaitMatched {
                line,
                col,
            }));
        }

        let now = Instant::now();
        if wait.is_quiet(now) {
            return Some(ControlResponse::Data(ControlData::WaitQuiet));
        }

        if wait.is_expired(now) {
            return Some(ControlResponse::Data(ControlData::WaitTimeout));
        }

        None
    }

    /// Process a single control request and return the response.
    /// This is extracted to allow recursive calls for batch commands.
    fn process_control_request(
        &mut self,
        client_key: usize,
        request: ultty_protocol::control::ControlRequest,
    ) -> ultty_protocol::control::ControlResponse {
        use ultty_protocol::control::{
            ControlData, ControlRequest, ControlResponse, SearchMatch, TabInfo, VersionInfo,
        };

        if let Some(response) = self.auth_error_for_request(client_key, &request) {
            return response;
        }

        match request {
            ControlRequest::Ping => ControlResponse::Data(ControlData::Pong),

            ControlRequest::Version => ControlResponse::Data(ControlData::Version(VersionInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: ultty_protocol::PROTOCOL_VERSION,
                git_hash: option_env!("GIT_HASH").map(String::from),
            })),

            ControlRequest::GetInfo => {
                if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let active_idx = self
                        .terminal_manager
                        .tabs()
                        .iter()
                        .position(|t| {
                            self.terminal_manager
                                .active_tab()
                                .is_some_and(|a| a.id == t.id)
                        })
                        .unwrap_or(0);
                    ControlResponse::Data(ControlData::Info(terminal_info(
                        terminal,
                        active_idx,
                        self.terminal_manager.tab_count(),
                    )))
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::ListTabs => {
                let active_tab_id = self.terminal_manager.active_tab().map(|t| t.id);
                let tabs: Vec<TabInfo> = self
                    .terminal_manager
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| TabInfo {
                        index: i,
                        title: self.terminal_manager.tab_display_title(tab.id),
                        active: Some(tab.id) == active_tab_id,
                        pid: self
                            .terminal_manager
                            .terminal(tab.focused_terminal)
                            .map(|terminal| terminal.event_loop.child_pid()),
                        cwd: self
                            .terminal_manager
                            .terminal(tab.focused_terminal)
                            .and_then(terminal_working_directory),
                    })
                    .collect();
                ControlResponse::Data(ControlData::Tabs { tabs })
            }

            ControlRequest::ListPanes => ControlResponse::Data(ControlData::Panes {
                panes: pane_infos(&self.terminal_manager),
            }),

            ControlRequest::GetText {
                include_scrollback,
                scrollback_lines,
                target,
            } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if let Some(terminal) = self.terminal_manager.terminal(pane) {
                    let term = terminal.event_loop.terminal();
                    let history = if include_scrollback {
                        scrollback_lines.unwrap_or(usize::MAX)
                    } else {
                        0
                    };
                    ControlResponse::Text {
                        text: buffer_text(term.buffer(), history),
                    }
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::LastCommand { tail_lines, target } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let last = self.terminal_manager.terminal(pane).and_then(|terminal| {
                    let tail = tail_lines.map(|lines| lines.min(usize::MAX as u64) as usize);
                    terminal.event_loop.terminal().last_command(tail)
                });
                match last {
                    Some(last) => ControlResponse::Data(ControlData::LastCommand {
                        command: last.command,
                        output: last.output,
                        exit_code: last.exit_code,
                        running: last.running,
                        output_available: last.output_available,
                    }),
                    None => ControlResponse::Error {
                        code: NO_COMMAND_RECORDED,
                        message: format!(
                            "No command recorded in pane {}; it needs a shell with OSC 133 \
                             integration",
                            pane.raw()
                        ),
                    },
                }
            }

            ControlRequest::GetSelection { target } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let text = self
                    .terminal_manager
                    .terminal(pane)
                    .and_then(|terminal| terminal.event_loop.terminal().selected_text());
                ControlResponse::Data(ControlData::Selection { text })
            }

            ControlRequest::SendText { text, target } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let foreground = self.pane_foreground_pgrp(pane);
                if let Some(ref mut server) = self.protocol_server {
                    if server.has_key_subscribers(foreground) {
                        server.broadcast_text_input(text, foreground);
                        return ControlResponse::Ok;
                    }
                }

                if let Some(terminal) = self.terminal_manager.terminal_mut(pane) {
                    let _ = terminal.event_loop.send_input(text.as_bytes());
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::SendKey {
                key,
                modifiers,
                target,
            } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let foreground = self.pane_foreground_pgrp(pane);
                if let Some(ref mut server) = self.protocol_server {
                    if server.has_key_subscribers(foreground) {
                        server.broadcast_key(key, modifiers, foreground);
                        return ControlResponse::Ok;
                    }
                }

                self.forward_key_to_pty(pane, &key, modifiers);
                ControlResponse::Ok
            }

            ControlRequest::SendRaw { data, target } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if let Some(terminal) = self.terminal_manager.terminal_mut(pane) {
                    let _ = terminal.event_loop.send_input(&data);
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::NewTab {
                title,
                command,
                args,
                working_dir,
            } => {
                let result = if let Some(cmd) = command {
                    self.create_tab_with_command(
                        &cmd,
                        &args,
                        working_dir.as_deref(),
                        title.as_deref(),
                    )
                } else {
                    let result = self.create_tab();
                    if let (Ok(tab_id), Some(title)) = (result.as_ref(), title.as_deref()) {
                        self.terminal_manager.set_tab_title(*tab_id, title);
                    }
                    result
                };

                match result {
                    Ok(tab_id) => {
                        self.clear_tab_width_freeze();
                        self.update_tab_bar();
                        self.needs_redraw = true;
                        let tabs = self.terminal_manager.tabs();
                        match tabs.iter().position(|tab| tab.id == tab_id) {
                            Some(tab_index) => ControlResponse::Data(ControlData::TabOpened {
                                tab_index,
                                pane: tabs[tab_index].focused_terminal.raw() as u64,
                            }),
                            // The command exited at once and the tab closed.
                            None => ControlResponse::Error {
                                code: 2,
                                message: "The tab closed as soon as it opened".to_string(),
                            },
                        }
                    }
                    Err(e) => ControlResponse::Error {
                        code: 2,
                        message: format!("Failed to create tab: {}", e),
                    },
                }
            }

            ControlRequest::SwitchTab { index } => {
                if index < self.terminal_manager.tab_count() {
                    self.terminal_manager.switch_to_tab(index);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 3,
                        message: format!("Tab index {} out of range", index),
                    }
                }
            }

            ControlRequest::CloseTab { index } => {
                let tabs = self.terminal_manager.tabs();
                let active_idx = tabs
                    .iter()
                    .position(|t| {
                        self.terminal_manager
                            .active_tab()
                            .is_some_and(|a| a.id == t.id)
                    })
                    .unwrap_or(0);
                let idx = index.unwrap_or(active_idx);
                if self.terminal_manager.tab_count() > 1 && idx < tabs.len() {
                    let tab_id = tabs[idx].id;
                    self.freeze_tab_width_after_close();
                    self.terminal_manager.close_tab(tab_id);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 4,
                        message: "Cannot close last tab or invalid index".to_string(),
                    }
                }
            }

            ControlRequest::RunCommand {
                command,
                args,
                target,
            } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if let Some(terminal) = self.terminal_manager.terminal_mut(pane) {
                    let cmd_line = if args.is_empty() {
                        format!("{}\n", command)
                    } else {
                        format!("{} {}\n", command, args.join(" "))
                    };
                    let _ = terminal.event_loop.send_input(cmd_line.as_bytes());
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::FocusWindow => {
                self.window.focus_window();
                ControlResponse::Ok
            }

            ControlRequest::ToggleFullscreen => {
                use winit::window::Fullscreen;
                let current = self.window.fullscreen();
                if current.is_some() {
                    self.window.set_fullscreen(None);
                } else {
                    self.window
                        .set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
                ControlResponse::Ok
            }

            ControlRequest::Resize { width, height } => {
                let requested = PhysicalSize::new(width, height);
                let result = self.window.request_inner_size(requested);
                let response = resize_control_response(result, requested, self.window.inner_size());
                if matches!(response, ControlResponse::Ok) {
                    self.handle_resize(requested);
                }
                response
            }

            ControlRequest::SetOpacity { opacity } => {
                self.renderer.set_opacity(opacity.clamp(0.0, 1.0));
                self.needs_redraw = true;
                ControlResponse::Ok
            }

            ControlRequest::SetPaneStatus { status, target } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let status = status.map(|status| status.sanitized());
                let wants_user = status.as_ref().is_some_and(|status| status.attention);
                if let Some(terminal) = self.terminal_manager.terminal_mut(pane) {
                    terminal.status = status;
                }
                if wants_user && !self.window.has_focus() {
                    // Cleared when the window gains focus, as for the bell.
                    self.window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
                }
                self.update_tab_bar();
                self.needs_redraw = true;
                ControlResponse::Ok
            }

            ControlRequest::Screenshot { path } => {
                let path = match path {
                    Some(path_str) => std::path::PathBuf::from(path_str),
                    None => {
                        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
                        let filename = format!("ultty-screenshot-{}.png", timestamp);
                        dirs::picture_dir()
                            .map(|p| p.join("Screenshots").join(&filename))
                            .unwrap_or_else(|| std::path::PathBuf::from(&filename))
                    }
                };
                // Multi-pane tabs capture every pane as the window shows
                // them. This must run before the focused terminal is locked
                // below: the capture locks each pane itself.
                let result = if self.terminal_manager.pane_count() > 1 {
                    Some(self.capture_panes_screenshot(&path))
                } else if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let term = terminal.event_loop.terminal();
                    let overlay_state = self.build_screenshot_overlay_state();
                    let overlay = overlay_state.as_overlay();

                    let native_sessions: Vec<&ultty::protocol::NativeSession> = self
                        .protocol_server
                        .as_ref()
                        .map(|s| s.sessions().collect())
                        .unwrap_or_default();

                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    Some(self.renderer.capture_screenshot_with_native(
                        &term,
                        &overlay,
                        &path,
                        &native_sessions,
                    ))
                } else {
                    None
                };
                match result {
                    Some(Ok(())) => ControlResponse::Data(ControlData::ScreenshotSaved {
                        path: path.display().to_string(),
                    }),
                    Some(Err(e)) => ControlResponse::Error {
                        code: 5,
                        message: format!("Screenshot failed: {}", e),
                    },
                    None => ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    },
                }
            }

            ControlRequest::Search {
                pattern,
                regex: use_regex,
                case_sensitive,
                include_scrollback,
                target,
            } => {
                let pane = match resolve_control_target(&self.terminal_manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if let Some(terminal) = self.terminal_manager.terminal(pane) {
                    let compiled_regex = if use_regex {
                        let regex_pattern = if case_sensitive {
                            pattern.clone()
                        } else {
                            format!("(?i){}", pattern)
                        };
                        match regex::Regex::new(&regex_pattern) {
                            Ok(re) => Some(re),
                            Err(e) => {
                                return ControlResponse::Error {
                                    code: 4,
                                    message: format!("Invalid regex: {}", e),
                                };
                            }
                        }
                    } else {
                        None
                    };

                    let term = terminal.event_loop.terminal();
                    let buffer = term.buffer();
                    let mut matches = Vec::new();

                    let line_count = if include_scrollback {
                        buffer.total_lines()
                    } else {
                        term.dimensions().rows as usize
                    };

                    for row in 0..line_count {
                        let row_opt = if include_scrollback {
                            buffer.row_at(row)
                        } else {
                            buffer.visible_row(row as u16)
                        };

                        if let Some(grid_row) = row_opt {
                            let text = grid_row.text_trimmed();

                            if let Some(ref re) = compiled_regex {
                                for m in re.find_iter(&text) {
                                    let byte_start = m.start();
                                    let byte_end = m.end();
                                    let match_text = m.as_str().to_string();
                                    // Find char-safe context boundaries
                                    let ctx_start = text[..byte_start]
                                        .char_indices()
                                        .rev()
                                        .nth(19)
                                        .map(|(i, _)| i)
                                        .unwrap_or(0);
                                    let ctx_end = text[byte_end..]
                                        .char_indices()
                                        .nth(20)
                                        .map(|(i, _)| byte_end + i)
                                        .unwrap_or(text.len());
                                    let col = text[..byte_start].chars().count();
                                    matches.push(SearchMatch {
                                        line: row,
                                        col,
                                        text: match_text,
                                        context_before: text[ctx_start..byte_start].to_string(),
                                        context_after: text[byte_end..ctx_end].to_string(),
                                    });
                                }
                            } else {
                                let spans: Vec<(usize, usize, usize)> = if case_sensitive {
                                    text.match_indices(pattern.as_str())
                                        .map(|(start, matched)| {
                                            (
                                                text[..start].chars().count(),
                                                start,
                                                start + matched.len(),
                                            )
                                        })
                                        .collect()
                                } else {
                                    let pattern_lower = pattern.to_lowercase();
                                    find_ignore_case(&text, &pattern_lower)
                                        .map(|m| (m.char_col, m.byte_start, m.byte_end))
                                        .collect()
                                };

                                for (col, byte_start, byte_end) in spans {
                                    matches.push(plain_search_match(
                                        &text, row, col, byte_start, byte_end,
                                    ));
                                }
                            }
                        }
                    }

                    ControlResponse::Data(ControlData::SearchResults { matches })
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::Subscribe { .. } => ControlResponse::Error {
                code: 5,
                message: "subscribe cannot be used inside batch requests".to_string(),
            },

            ControlRequest::WaitFor { .. } => ControlResponse::Error {
                code: 5,
                message: "wait-for cannot be used inside batch requests".to_string(),
            },

            ControlRequest::Batch { commands } => {
                let results: Vec<ControlResponse> = commands
                    .into_iter()
                    .map(|cmd| self.process_control_request(client_key, cmd))
                    .collect();
                ControlResponse::Data(ControlData::BatchResults { results })
            }

            ControlRequest::Authenticate { token } => {
                match self.authenticate_client(client_key, &token) {
                    Ok(()) => ControlResponse::Data(ControlData::Authenticated),
                    Err(response) => response,
                }
            }

            ControlRequest::WithToken { token, request } => {
                // Reached only when nested inside a batch; top-level
                // WithToken is unwrapped in `handle_control_requests`.
                match self.authenticate_client(client_key, &token) {
                    Ok(()) => self.process_control_request(client_key, *request),
                    Err(response) => response,
                }
            }
        }
    }

    /// Check `token` against the token file and mark the client authenticated.
    ///
    /// Fails closed: a missing or unreadable token file rejects every token.
    fn authenticate_client(
        &mut self,
        client_key: usize,
        token: &str,
    ) -> Result<(), ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        let expected = self
            .control_server
            .as_ref()
            .ok_or_else(|| io::Error::other("control server not running"))
            .and_then(|server| read_auth_token(server.token_path()));
        let expected = match expected {
            Ok(expected) => expected,
            Err(e) => {
                tracing::warn!("Control auth token unavailable: {}", e);
                return Err(ControlResponse::Error {
                    code: 500,
                    message: "Control auth token unavailable".to_string(),
                });
            }
        };

        if !token_matches(token, &expected) {
            return Err(ControlResponse::Error {
                code: 403,
                message: "Invalid token".to_string(),
            });
        }

        if let Some(ref mut server) = self.control_server {
            server.set_client_authenticated(client_key, true);
        }
        Ok(())
    }

    /// Process group in the foreground of `pane`'s PTY.
    fn pane_foreground_pgrp(&self, pane: TerminalId) -> Option<i32> {
        self.terminal_manager
            .terminal(pane)
            .and_then(|terminal| terminal.event_loop.foreground_pgrp())
    }

    fn forward_key_to_pty(
        &mut self,
        pane: TerminalId,
        key: &ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) {
        let Some(terminal) = self.terminal_manager.terminal_mut(pane) else {
            return;
        };

        let ctrl = modifiers.contains(ultty_protocol::Modifiers::CTRL);
        let alt = modifiers.contains(ultty_protocol::Modifiers::ALT);
        let app_cursor = terminal
            .event_loop
            .terminal()
            .modes()
            .application_cursor_keys;

        let bytes = key_to_bytes(key, ctrl, alt, app_cursor);

        if !bytes.is_empty() {
            let _ = terminal.event_loop.send_input(&bytes);
        }
    }

    fn auth_error_for_request(
        &self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        if !control_request_requires_auth(request) {
            return None;
        }

        let authenticated = self
            .control_server
            .as_ref()
            .is_some_and(|server| server.is_client_authenticated(client_key));

        if authenticated {
            None
        } else {
            Some(ControlResponse::Error {
                code: 403,
                message: "Authentication required".to_string(),
            })
        }
    }

    fn find_wait_match(&self, pane: TerminalId, matcher: &WaitMatcher) -> Option<(usize, usize)> {
        let terminal = self.terminal_manager.terminal(pane)?;
        let term = terminal.event_loop.terminal();
        let buffer = term.buffer();
        let total_lines = buffer.total_lines();

        for row in 0..total_lines {
            let Some(grid_row) = buffer.row_at(row) else {
                continue;
            };
            let text = grid_row.text_trimmed();

            if let Some(col) = matcher.find_in(&text) {
                return Some((row, col));
            }
        }

        None
    }
}

/// What `ulttyctl info` reports for `terminal`.
///
/// The terminal lock is released before the working directory is looked up,
/// because that lookup takes the lock again; holding it deadlocked the UI
/// thread on every `info` request.
fn terminal_info(
    terminal: &ultty::terminal_manager::ManagedTerminal,
    current_tab: usize,
    tab_count: usize,
) -> ultty_protocol::control::TerminalInfo {
    let (dims, cursor) = {
        let term = terminal.event_loop.terminal();
        (term.grid().dimensions(), term.grid().cursor().position)
    };
    ultty_protocol::control::TerminalInfo {
        cols: dims.cols,
        rows: dims.rows,
        cursor_col: cursor.col,
        cursor_row: cursor.row,
        current_tab,
        tab_count,
        focused: true,
        cwd: terminal_working_directory(terminal),
    }
}

/// Error code for a `target` that names no open pane.
const PANE_NOT_FOUND: u16 = 6;

/// Error code for `last_command` in a pane whose shell marked no command.
const NO_COMMAND_RECORDED: u16 = 7;

/// The pane a request acts on: `target` (an id from `ListPanes`), or the
/// focused pane when it is omitted.
fn resolve_control_target(
    manager: &ultty::terminal_manager::TerminalManager,
    target: Option<u64>,
) -> Result<TerminalId, ultty_protocol::control::ControlResponse> {
    use ultty_protocol::control::ControlResponse;

    let Some(id) = target else {
        return manager
            .focused_terminal_id()
            .ok_or_else(|| ControlResponse::Error {
                code: 1,
                message: "No active terminal".to_string(),
            });
    };
    usize::try_from(id)
        .ok()
        .map(TerminalId::new)
        .filter(|pane| manager.terminal(*pane).is_some())
        .ok_or_else(|| ControlResponse::Error {
            code: PANE_NOT_FOUND,
            message: format!("No pane {id}; `ulttyctl panes` lists them"),
        })
}

/// Every pane in every tab: tabs in order, each tab's panes in layout order.
fn pane_infos(
    manager: &ultty::terminal_manager::TerminalManager,
) -> Vec<ultty_protocol::control::PaneInfo> {
    let active_tab_id = manager.active_tab().map(|tab| tab.id);
    let mut panes = Vec::new();
    for (tab_index, tab) in manager.tabs().iter().enumerate() {
        for &pane_id in &tab.terminals {
            let Some(terminal) = manager.terminal(pane_id) else {
                continue;
            };
            let active = pane_id == tab.focused_terminal;
            // Hold the terminal lock only for these reads: the working
            // directory lookup below takes it again.
            let (title, dims) = {
                let term = terminal.event_loop.terminal();
                (term.title().to_string(), term.grid().dimensions())
            };
            panes.push(ultty_protocol::control::PaneInfo {
                id: pane_id.raw() as u64,
                tab_index,
                active,
                focused: active && Some(tab.id) == active_tab_id,
                title,
                foreground: terminal.event_loop.foreground_process_name(),
                pid: Some(terminal.event_loop.child_pid()),
                cwd: terminal_working_directory(terminal),
                cols: dims.cols,
                rows: dims.rows,
                status: terminal.shown_status().cloned(),
            });
        }
    }
    panes
}

pub(super) fn terminal_working_directory(
    terminal: &ultty::terminal_manager::ManagedTerminal,
) -> Option<String> {
    terminal
        .event_loop
        .terminal()
        .working_directory()
        .map(str::to_string)
        .or_else(|| terminal.event_loop.child_working_directory())
}

/// Generate a fresh random control auth token and write it to `path`.
///
/// The file is created with mode 0600 (and its parent directory 0700 when
/// it has to be created), so only the owning user can talk to the control
/// socket. Called once per control-server start; a running instance owns
/// the socket, so rotating the token on start never races another instance.
pub(super) fn write_auth_token(path: &Path) -> io::Result<String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let mut random = [0u8; AUTH_TOKEN_BYTES];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
    }

    // Replace atomically-ish: truncate an existing file rather than leaving
    // a stale token readable while we write.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies on creation; enforce it for pre-existing files too.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(token)
}

/// Read the expected control auth token from `path`.
fn read_auth_token(path: &Path) -> io::Result<String> {
    let token = std::fs::read_to_string(path)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "auth token file is empty",
        ));
    }
    Ok(token.to_string())
}

/// Compare a presented token with the expected one without early exit on
/// the first differing byte.
fn token_matches(presented: &str, expected: &str) -> bool {
    let presented = presented.trim().as_bytes();
    let expected = expected.as_bytes();
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn control_request_requires_auth(request: &ultty_protocol::control::ControlRequest) -> bool {
    use ultty_protocol::control::ControlRequest;

    match request {
        // Harmless liveness/version probes and the auth handshake itself.
        ControlRequest::Ping
        | ControlRequest::Version
        | ControlRequest::Authenticate { .. }
        | ControlRequest::WithToken { .. } => false,
        // Batch members are checked individually when they run.
        ControlRequest::Batch { .. } => false,
        // Everything that reads screen content or drives the terminal.
        ControlRequest::GetInfo
        | ControlRequest::ListTabs
        | ControlRequest::ListPanes
        | ControlRequest::GetText { .. }
        | ControlRequest::GetSelection { .. }
        | ControlRequest::LastCommand { .. }
        | ControlRequest::Search { .. }
        | ControlRequest::WaitFor { .. }
        | ControlRequest::Subscribe { .. }
        | ControlRequest::SendText { .. }
        | ControlRequest::SendKey { .. }
        | ControlRequest::SendRaw { .. }
        | ControlRequest::NewTab { .. }
        | ControlRequest::SwitchTab { .. }
        | ControlRequest::CloseTab { .. }
        | ControlRequest::RunCommand { .. }
        | ControlRequest::FocusWindow
        | ControlRequest::ToggleFullscreen
        | ControlRequest::Resize { .. }
        | ControlRequest::SetOpacity { .. }
        | ControlRequest::SetPaneStatus { .. }
        | ControlRequest::Screenshot { .. } => true,
    }
}

/// The text of `buffer`: up to `history_lines` of the most recent scrollback,
/// then the screen, one line per row. Each row has one char per column
/// (wide-char spacers print as spaces) plus the extra codepoints of each
/// grapheme. This ignores the viewport's scroll position.
fn buffer_text(buffer: &ultty::grid::Buffer, history_lines: usize) -> String {
    let history = buffer.history_len();
    let first = history - history.min(history_lines);
    let mut text = String::new();
    for index in first..buffer.total_lines() {
        if let Some(row) = buffer.row_at(index) {
            text.push_str(&row.text());
        }
        text.push('\n');
    }
    text
}

/// Build a search match for `text[byte_start..byte_end]` with up to 20 chars
/// of context on each side. Offsets must be char boundaries of `text`.
fn plain_search_match(
    text: &str,
    line: usize,
    col: usize,
    byte_start: usize,
    byte_end: usize,
) -> ultty_protocol::control::SearchMatch {
    let ctx_start = text[..byte_start]
        .char_indices()
        .rev()
        .nth(19)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let ctx_end = text[byte_end..]
        .char_indices()
        .nth(20)
        .map(|(i, _)| byte_end + i)
        .unwrap_or(text.len());
    ultty_protocol::control::SearchMatch {
        line,
        col,
        text: text[byte_start..byte_end].to_string(),
        context_before: text[ctx_start..byte_start].to_string(),
        context_after: text[byte_end..ctx_end].to_string(),
    }
}

fn resize_control_response(
    result: Option<PhysicalSize<u32>>,
    requested: PhysicalSize<u32>,
    current: PhysicalSize<u32>,
) -> ultty_protocol::control::ControlResponse {
    match (result, current == requested) {
        (Some(_), true) => ultty_protocol::control::ControlResponse::Ok,
        (Some(accepted), false) => ultty_protocol::control::ControlResponse::Error {
            code: 8,
            message: format!(
                "Window resize request not applied by platform/compositor: requested {}x{}, accepted {}x{}, current {}x{}",
                requested.width,
                requested.height,
                accepted.width,
                accepted.height,
                current.width,
                current.height
            ),
        },
        (None, _) => ultty_protocol::control::ControlResponse::Error {
            code: 8,
            message: "Window resize is not supported by this platform/compositor".to_string(),
        },
    }
}

/// Convert a protocol key (with modifier flags) to the byte sequence expected by a PTY.
///
/// `app_cursor` controls whether application cursor key mode (DECCKM) sequences
/// are emitted for arrow/Home/End keys.
fn key_to_bytes(key: &ultty_protocol::Key, ctrl: bool, alt: bool, app_cursor: bool) -> Vec<u8> {
    match key {
        ultty_protocol::Key::Char(c) => {
            if ctrl && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_lowercase() as u8) - b'a' + 1]
            } else if alt {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        ultty_protocol::Key::Enter => vec![b'\r'],
        ultty_protocol::Key::Tab => vec![b'\t'],
        ultty_protocol::Key::Escape => vec![0x1b],
        ultty_protocol::Key::Backspace => vec![0x7f],
        ultty_protocol::Key::Delete => b"\x1b[3~".to_vec(),
        ultty_protocol::Key::Insert => b"\x1b[2~".to_vec(),
        ultty_protocol::Key::Home => {
            if app_cursor {
                b"\x1bOH".to_vec()
            } else {
                b"\x1b[H".to_vec()
            }
        }
        ultty_protocol::Key::End => {
            if app_cursor {
                b"\x1bOF".to_vec()
            } else {
                b"\x1b[F".to_vec()
            }
        }
        ultty_protocol::Key::PageUp => b"\x1b[5~".to_vec(),
        ultty_protocol::Key::PageDown => b"\x1b[6~".to_vec(),
        ultty_protocol::Key::Up => {
            if app_cursor {
                b"\x1bOA".to_vec()
            } else {
                b"\x1b[A".to_vec()
            }
        }
        ultty_protocol::Key::Down => {
            if app_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        ultty_protocol::Key::Right => {
            if app_cursor {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }
        }
        ultty_protocol::Key::Left => {
            if app_cursor {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }
        }
        ultty_protocol::Key::Function(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
    }
}

/// Format a command and its arguments into a newline-terminated string
/// suitable for sending to a PTY (as used by RunCommand).
#[cfg(test)]
fn build_command_line(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        format!("{}\n", command)
    } else {
        format!("{} {}\n", command, args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6x2 buffer with five lines written: three in history, two on screen.
    fn buffer_with_history() -> ultty::grid::Buffer {
        let mut buffer = ultty::grid::Buffer::new(ultty::grid::Dimensions::new(6, 2));
        for (i, line) in ["one", "two", "three", "four", "five"].iter().enumerate() {
            buffer.write_ascii_span(line.as_bytes());
            if i < 4 {
                buffer.grid_mut().carriage_return();
                buffer.newline();
            }
        }
        buffer
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().map(str::trim_end).collect()
    }

    #[test]
    fn text_without_scrollback_is_the_screen() {
        let buffer = buffer_with_history();
        assert_eq!(buffer.history_len(), 3);
        assert_eq!(lines(&buffer_text(&buffer, 0)), ["four", "five"]);
    }

    #[test]
    fn text_with_scrollback_includes_history_oldest_first() {
        let buffer = buffer_with_history();
        assert_eq!(
            lines(&buffer_text(&buffer, usize::MAX)),
            ["one", "two", "three", "four", "five"]
        );
    }

    #[test]
    fn text_with_a_scrollback_limit_keeps_the_most_recent_lines() {
        let buffer = buffer_with_history();
        assert_eq!(lines(&buffer_text(&buffer, 1)), ["three", "four", "five"]);
    }
    use ultty_protocol::control::{ControlRequest, ControlResponse};
    use ultty_protocol::Key;

    // ---------------------------------------------------------------
    // key_to_bytes: simple keys (no modifiers, no app-cursor)
    // ---------------------------------------------------------------

    #[test]
    fn mutating_control_requests_require_auth() {
        assert!(control_request_requires_auth(&ControlRequest::SendText {
            text: "hello".to_string(),
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::NewTab {
            title: None,
            command: None,
            args: Vec::new(),
            working_dir: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::RunCommand {
            command: "make".to_string(),
            args: vec!["test".to_string()],
            target: None,
        }));
    }

    #[test]
    fn screen_reading_control_requests_require_auth() {
        assert!(control_request_requires_auth(&ControlRequest::GetText {
            include_scrollback: true,
            scrollback_lines: None,
            target: None,
        }));
        assert!(control_request_requires_auth(
            &ControlRequest::GetSelection { target: None }
        ));
        assert!(control_request_requires_auth(&ControlRequest::Search {
            pattern: "needle".to_string(),
            regex: false,
            case_sensitive: false,
            include_scrollback: true,
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::WaitFor {
            pattern: "ready".to_string(),
            regex: false,
            timeout_ms: 100,
            until: Default::default(),
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::GetInfo));
        assert!(control_request_requires_auth(&ControlRequest::ListPanes));
    }

    #[test]
    fn only_probes_and_auth_are_exempt_from_auth() {
        assert!(!control_request_requires_auth(&ControlRequest::Ping));
        assert!(!control_request_requires_auth(&ControlRequest::Version));
        assert!(!control_request_requires_auth(
            &ControlRequest::Authenticate {
                token: "t".to_string(),
            }
        ));
        assert!(!control_request_requires_auth(&ControlRequest::WithToken {
            token: "t".to_string(),
            request: Box::new(ControlRequest::Ping),
        }));
    }

    #[test]
    fn write_auth_token_creates_private_random_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ultty-auth.token");

        let first = write_auth_token(&path).unwrap();
        let second = write_auth_token(&path).unwrap();

        assert_eq!(first.len(), AUTH_TOKEN_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "token must be regenerated");
        assert_eq!(read_auth_token(&path).unwrap(), second);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn read_auth_token_fails_closed_when_missing_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.token");
        assert!(read_auth_token(&missing).is_err());

        let empty = dir.path().join("empty.token");
        std::fs::write(&empty, "\n").unwrap();
        assert!(read_auth_token(&empty).is_err());
    }

    #[test]
    fn token_matches_ignores_surrounding_whitespace_only() {
        assert!(token_matches("abc123\n", "abc123"));
        assert!(!token_matches("abc124", "abc123"));
        assert!(!token_matches("abc12", "abc123"));
        assert!(!token_matches("", "abc123"));
    }

    #[test]
    fn plain_search_match_uses_char_columns_on_multibyte_rows() {
        let text = "aİé ready";
        let pattern_lower = "ready";
        let found: Vec<_> = find_ignore_case(text, pattern_lower).collect();
        assert_eq!(found.len(), 1);
        let m = found[0];
        let result = plain_search_match(text, 3, m.char_col, m.byte_start, m.byte_end);

        assert_eq!(result.line, 3);
        assert_eq!(result.col, 4);
        assert_eq!(result.text, "ready");
        assert_eq!(result.context_before, "aİé ");
        assert_eq!(result.context_after, "");
    }

    #[test]
    fn info_does_not_lock_the_terminal_twice() {
        // Debug builds panic on a re-entrant terminal lock (release builds
        // deadlock), so this fails if terminal_info holds the lock while
        // looking up the working directory.
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        manager
            .focused_terminal_mut()
            .unwrap()
            .event_loop
            .terminal_mut()
            .set_working_directory("/tmp/ultty-info-cwd".to_string());
        let info = terminal_info(manager.focused_terminal().unwrap(), 0, 1);
        assert_eq!((info.cols, info.rows), (80, 24));
        assert_eq!(info.cwd.as_deref(), Some("/tmp/ultty-info-cwd"));
    }

    fn pane_ids(panes: &[ultty_protocol::control::PaneInfo]) -> Vec<(u64, usize)> {
        panes.iter().map(|pane| (pane.id, pane.tab_index)).collect()
    }

    fn focused_pane(panes: &[ultty_protocol::control::PaneInfo]) -> Vec<u64> {
        panes
            .iter()
            .filter(|pane| pane.focused)
            .map(|pane| pane.id)
            .collect()
    }

    #[test]
    fn panes_list_every_pane_in_layout_order_with_stable_ids() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let left = manager.focused_terminal_id().unwrap().raw() as u64;
        let right = manager.split_horizontal().unwrap().unwrap().raw() as u64;
        manager.create_tab().unwrap();
        let other = manager.focused_terminal_id().unwrap().raw() as u64;

        let panes = pane_infos(&manager);
        assert_eq!(pane_ids(&panes), vec![(left, 0), (right, 0), (other, 1)]);
        // Only the active pane of the active tab has keyboard focus; each tab
        // still reports its own active pane (a split keeps focus where it was).
        assert_eq!(focused_pane(&panes), vec![other]);
        let active: Vec<u64> = panes.iter().filter(|p| p.active).map(|p| p.id).collect();
        assert_eq!(active, vec![left, other]);
        assert!(panes
            .iter()
            .all(|p| p.pid.is_some() && p.cols > 0 && p.rows > 0));

        // Switching tabs, moving focus and reordering tabs move the focus
        // and the tab index, never the ids.
        manager.switch_to_tab(0);
        manager.focus_next_pane();
        assert_eq!(focused_pane(&pane_infos(&manager)), vec![right]);
        assert!(manager.reorder_tabs(0, 1));
        assert_eq!(
            pane_ids(&pane_infos(&manager)),
            vec![(other, 0), (left, 1), (right, 1)]
        );
    }

    #[test]
    fn target_resolves_to_the_named_pane_or_the_focused_one() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let focused = manager.focused_terminal_id().unwrap();
        let other = manager.split_horizontal().unwrap().unwrap();

        assert_eq!(resolve_control_target(&manager, None), Ok(focused));
        assert_eq!(
            resolve_control_target(&manager, Some(other.raw() as u64)),
            Ok(other)
        );
    }

    #[test]
    fn target_naming_no_open_pane_is_an_error() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let closed = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(closed);
        assert!(manager.close_focused_pane());

        for target in [closed.raw() as u64, 999, u64::MAX] {
            match resolve_control_target(&manager, Some(target)) {
                Err(ultty_protocol::control::ControlResponse::Error { code, message }) => {
                    assert_eq!(code, PANE_NOT_FOUND);
                    assert!(message.contains(&target.to_string()), "{message}");
                }
                other => panic!("target {target}: expected an error, got {other:?}"),
            }
        }
    }

    #[test]
    fn closed_pane_ids_are_not_reused() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let split = manager.split_horizontal().unwrap().unwrap();
        let closed = split.raw() as u64;
        manager.focus_terminal(split);
        assert!(manager.close_focused_pane());
        let next = manager.split_horizontal().unwrap().unwrap().raw() as u64;
        let ids: Vec<u64> = pane_infos(&manager).iter().map(|p| p.id).collect();
        assert!(!ids.contains(&closed));
        assert!(ids.contains(&next));
    }

    #[test]
    fn terminal_working_directory_prefers_terminal_reported_cwd() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        manager
            .focused_terminal_mut()
            .unwrap()
            .event_loop
            .terminal_mut()
            .set_working_directory("/tmp/ultty-control-cwd".to_string());

        assert_eq!(
            terminal_working_directory(manager.focused_terminal().unwrap()).as_deref(),
            Some("/tmp/ultty-control-cwd")
        );
    }

    #[test]
    fn key_char_plain_ascii() {
        assert_eq!(key_to_bytes(&Key::Char('a'), false, false, false), b"a");
        assert_eq!(key_to_bytes(&Key::Char('Z'), false, false, false), b"Z");
        assert_eq!(key_to_bytes(&Key::Char('5'), false, false, false), b"5");
    }

    #[test]
    fn resize_control_response_reports_unsupported_platform() {
        let response = resize_control_response(
            None,
            PhysicalSize::new(800, 600),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("not supported")
        ));
    }

    #[test]
    fn resize_control_response_reports_ok_when_platform_accepts_request() {
        let response = resize_control_response(
            Some(PhysicalSize::new(800, 600)),
            PhysicalSize::new(800, 600),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(response, ControlResponse::Ok));
    }

    #[test]
    fn resize_control_response_reports_adjusted_size() {
        let response = resize_control_response(
            Some(PhysicalSize::new(968, 709)),
            PhysicalSize::new(1280, 720),
            PhysicalSize::new(968, 709),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("not applied")
                && message.contains("1280x720")
                && message.contains("968x709")
        ));
    }

    #[test]
    fn resize_control_response_reports_unapplied_even_when_platform_echoes_request() {
        let response = resize_control_response(
            Some(PhysicalSize::new(1280, 720)),
            PhysicalSize::new(1280, 720),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("not applied")
                && message.contains("current 800x600")
        ));
    }

    #[test]
    fn key_char_multibyte_utf8() {
        // A non-ASCII character should encode as its UTF-8 bytes.
        let bytes = key_to_bytes(&Key::Char('\u{00e9}'), false, false, false); // 'e' with accent
        assert_eq!(bytes, "\u{00e9}".as_bytes());

        let bytes = key_to_bytes(&Key::Char('\u{4e2d}'), false, false, false); // CJK character
        assert_eq!(bytes, "\u{4e2d}".as_bytes());
    }

    #[test]
    fn key_enter() {
        assert_eq!(key_to_bytes(&Key::Enter, false, false, false), b"\r");
    }

    #[test]
    fn key_tab() {
        assert_eq!(key_to_bytes(&Key::Tab, false, false, false), b"\t");
    }

    #[test]
    fn key_escape() {
        assert_eq!(key_to_bytes(&Key::Escape, false, false, false), b"\x1b");
    }

    #[test]
    fn key_backspace() {
        assert_eq!(key_to_bytes(&Key::Backspace, false, false, false), b"\x7f");
    }

    #[test]
    fn key_delete() {
        assert_eq!(key_to_bytes(&Key::Delete, false, false, false), b"\x1b[3~");
    }

    #[test]
    fn key_insert() {
        assert_eq!(key_to_bytes(&Key::Insert, false, false, false), b"\x1b[2~");
    }

    #[test]
    fn key_page_up_down() {
        assert_eq!(key_to_bytes(&Key::PageUp, false, false, false), b"\x1b[5~");
        assert_eq!(
            key_to_bytes(&Key::PageDown, false, false, false),
            b"\x1b[6~"
        );
    }

    // ---------------------------------------------------------------
    // key_to_bytes: ctrl modifier
    // ---------------------------------------------------------------

    #[test]
    fn key_ctrl_a_through_z() {
        // Ctrl-A = 0x01, Ctrl-Z = 0x1A
        assert_eq!(key_to_bytes(&Key::Char('a'), true, false, false), vec![1]);
        assert_eq!(key_to_bytes(&Key::Char('c'), true, false, false), vec![3]);
        assert_eq!(key_to_bytes(&Key::Char('z'), true, false, false), vec![26]);
    }

    #[test]
    fn key_ctrl_uppercase_is_same_as_lowercase() {
        // Ctrl-A and Ctrl-a should produce the same byte.
        assert_eq!(
            key_to_bytes(&Key::Char('A'), true, false, false),
            key_to_bytes(&Key::Char('a'), true, false, false)
        );
        assert_eq!(
            key_to_bytes(&Key::Char('C'), true, false, false),
            key_to_bytes(&Key::Char('c'), true, false, false)
        );
    }

    #[test]
    fn key_ctrl_non_alpha_passes_through() {
        // Ctrl + non-alpha char should not produce a control character;
        // it falls through to the alt/plain branch.
        assert_eq!(key_to_bytes(&Key::Char('5'), true, false, false), b"5");
    }

    // ---------------------------------------------------------------
    // key_to_bytes: alt modifier
    // ---------------------------------------------------------------

    #[test]
    fn key_alt_char() {
        // Alt-a should be ESC followed by 'a'
        assert_eq!(
            key_to_bytes(&Key::Char('a'), false, true, false),
            vec![0x1b, b'a']
        );
        assert_eq!(
            key_to_bytes(&Key::Char('x'), false, true, false),
            vec![0x1b, b'x']
        );
    }

    #[test]
    fn key_alt_multibyte_char() {
        // Alt + non-ASCII should be ESC + UTF-8 bytes
        let bytes = key_to_bytes(&Key::Char('\u{00e9}'), false, true, false);
        let mut expected = vec![0x1b];
        expected.extend_from_slice("\u{00e9}".as_bytes());
        assert_eq!(bytes, expected);
    }

    #[test]
    fn key_ctrl_takes_precedence_over_alt_for_alpha() {
        // When both ctrl and alt are set for an alpha char, ctrl wins.
        assert_eq!(key_to_bytes(&Key::Char('c'), true, true, false), vec![3]);
    }

    // ---------------------------------------------------------------
    // key_to_bytes: arrow keys with and without app-cursor mode
    // ---------------------------------------------------------------

    #[test]
    fn arrow_keys_normal_mode() {
        assert_eq!(key_to_bytes(&Key::Up, false, false, false), b"\x1b[A");
        assert_eq!(key_to_bytes(&Key::Down, false, false, false), b"\x1b[B");
        assert_eq!(key_to_bytes(&Key::Right, false, false, false), b"\x1b[C");
        assert_eq!(key_to_bytes(&Key::Left, false, false, false), b"\x1b[D");
    }

    #[test]
    fn arrow_keys_app_cursor_mode() {
        assert_eq!(key_to_bytes(&Key::Up, false, false, true), b"\x1bOA");
        assert_eq!(key_to_bytes(&Key::Down, false, false, true), b"\x1bOB");
        assert_eq!(key_to_bytes(&Key::Right, false, false, true), b"\x1bOC");
        assert_eq!(key_to_bytes(&Key::Left, false, false, true), b"\x1bOD");
    }

    #[test]
    fn home_end_normal_mode() {
        assert_eq!(key_to_bytes(&Key::Home, false, false, false), b"\x1b[H");
        assert_eq!(key_to_bytes(&Key::End, false, false, false), b"\x1b[F");
    }

    #[test]
    fn home_end_app_cursor_mode() {
        assert_eq!(key_to_bytes(&Key::Home, false, false, true), b"\x1bOH");
        assert_eq!(key_to_bytes(&Key::End, false, false, true), b"\x1bOF");
    }

    // ---------------------------------------------------------------
    // key_to_bytes: function keys
    // ---------------------------------------------------------------

    #[test]
    fn function_keys_f1_through_f4() {
        assert_eq!(
            key_to_bytes(&Key::Function(1), false, false, false),
            b"\x1bOP"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(2), false, false, false),
            b"\x1bOQ"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(3), false, false, false),
            b"\x1bOR"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(4), false, false, false),
            b"\x1bOS"
        );
    }

    #[test]
    fn function_keys_f5_through_f12() {
        assert_eq!(
            key_to_bytes(&Key::Function(5), false, false, false),
            b"\x1b[15~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(6), false, false, false),
            b"\x1b[17~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(7), false, false, false),
            b"\x1b[18~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(8), false, false, false),
            b"\x1b[19~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(9), false, false, false),
            b"\x1b[20~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(10), false, false, false),
            b"\x1b[21~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(11), false, false, false),
            b"\x1b[23~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(12), false, false, false),
            b"\x1b[24~"
        );
    }

    #[test]
    fn function_key_out_of_range_returns_empty() {
        assert_eq!(
            key_to_bytes(&Key::Function(0), false, false, false),
            Vec::<u8>::new()
        );
        assert_eq!(
            key_to_bytes(&Key::Function(13), false, false, false),
            Vec::<u8>::new()
        );
        assert_eq!(
            key_to_bytes(&Key::Function(255), false, false, false),
            Vec::<u8>::new()
        );
    }

    // ---------------------------------------------------------------
    // build_command_line
    // ---------------------------------------------------------------

    #[test]
    fn command_line_no_args() {
        assert_eq!(build_command_line("ls", &[]), "ls\n");
    }

    #[test]
    fn command_line_with_args() {
        let args = vec!["--all".to_string(), "-l".to_string()];
        assert_eq!(build_command_line("ls", &args), "ls --all -l\n");
    }

    #[test]
    fn command_line_single_arg() {
        let args = vec!["/tmp".to_string()];
        assert_eq!(build_command_line("cd", &args), "cd /tmp\n");
    }

    #[test]
    fn command_line_always_ends_with_newline() {
        assert!(build_command_line("echo", &[]).ends_with('\n'));
        let args = vec!["hello".to_string()];
        assert!(build_command_line("echo", &args).ends_with('\n'));
    }

    // ---------------------------------------------------------------
    // Protocol type round-trip / construction tests
    // ---------------------------------------------------------------

    #[test]
    fn control_request_ping_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let req = ControlRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn control_response_error_has_correct_fields() {
        use ultty_protocol::control::ControlResponse;
        let resp = ControlResponse::Error {
            code: 1,
            message: "No active terminal".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"code\":1"));
        assert!(json.contains("No active terminal"));
    }

    #[test]
    fn control_response_pong_roundtrip() {
        use ultty_protocol::control::{ControlData, ControlResponse};
        let resp = ControlResponse::Data(ControlData::Pong);
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, resp);
    }

    #[test]
    fn version_info_construction() {
        use ultty_protocol::control::VersionInfo;
        let info = VersionInfo {
            version: "0.1.0".to_string(),
            protocol_version: 1,
            git_hash: Some("abc123".to_string()),
        };
        assert_eq!(info.version, "0.1.0");
        assert_eq!(info.protocol_version, 1);
        assert_eq!(info.git_hash, Some("abc123".to_string()));
    }

    #[test]
    fn batch_request_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let batch = ControlRequest::Batch {
            commands: vec![
                ControlRequest::Ping,
                ControlRequest::Version,
                ControlRequest::GetSelection { target: None },
            ],
        };
        let json = serde_json::to_string(&batch).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, batch);
    }

    #[test]
    fn search_request_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let req = ControlRequest::Search {
            pattern: "hello.*world".to_string(),
            regex: true,
            case_sensitive: false,
            include_scrollback: true,
            target: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }
}
