//! Control server request handling.
//!
//! Processes CLI automation requests from ulttyctl and other tools.

use super::App;
use ultty::renderer::OverlayState;
use winit::dpi::PhysicalSize;

impl App {
    pub(super) fn handle_control_requests(&mut self) {
        use ultty_protocol::control::ControlResponse;

        let requests = {
            let Some(ref mut server) = self.control_server else {
                return;
            };
            server.take_pending_requests()
        };

        let mut responses: Vec<(usize, ControlResponse)> = Vec::new();

        for (client_key, request) in requests {
            let response = self.process_control_request(client_key, request);
            responses.push((client_key, response));
        }

        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
        }
    }

    /// Process a single control request and return the response.
    /// This is extracted to allow recursive calls for batch commands.
    fn process_control_request(
        &mut self,
        client_key: usize,
        request: ultty_protocol::control::ControlRequest,
    ) -> ultty_protocol::control::ControlResponse {
        use ultty_protocol::control::{
            ControlData, ControlRequest, ControlResponse, SearchMatch, TabInfo, TerminalInfo,
            VersionInfo,
        };

        match request {
            ControlRequest::Ping => ControlResponse::Data(ControlData::Pong),

            ControlRequest::Version => ControlResponse::Data(ControlData::Version(VersionInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: ultty_protocol::PROTOCOL_VERSION,
                git_hash: option_env!("GIT_HASH").map(String::from),
            })),

            ControlRequest::GetInfo => {
                if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let term = terminal.event_loop.terminal();
                    let dims = term.grid().dimensions();
                    let cursor = term.grid().cursor();
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
                    ControlResponse::Data(ControlData::Info(TerminalInfo {
                        cols: dims.cols,
                        rows: dims.rows,
                        cursor_col: cursor.position.col,
                        cursor_row: cursor.position.row,
                        current_tab: active_idx,
                        tab_count: self.terminal_manager.tab_count(),
                        focused: true,
                        cwd: None,
                    }))
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
                        title: tab.title.clone(),
                        active: Some(tab.id) == active_tab_id,
                        pid: None,
                        cwd: None,
                    })
                    .collect();
                ControlResponse::Data(ControlData::Tabs(tabs))
            }

            ControlRequest::GetText {
                include_scrollback: _,
                scrollback_lines: _,
            } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let term = terminal.event_loop.terminal();
                    let dims = term.grid().dimensions();
                    let mut text = String::new();
                    for row in 0..dims.rows {
                        for col in 0..dims.cols {
                            if let Some(cell) = term.grid().cell(col, row) {
                                text.push(cell.char());
                            }
                        }
                        text.push('\n');
                    }
                    ControlResponse::Text(text)
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::GetSelection => ControlResponse::Data(ControlData::Selection(None)),

            ControlRequest::SendText { text } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
                    let _ = terminal.event_loop.send_input(text.as_bytes());
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::SendKey { key, modifiers } => {
                self.forward_key_to_pty(&key, modifiers);
                ControlResponse::Ok
            }

            ControlRequest::SendRaw { data } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
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
                    self.terminal_manager.create_tab_with_command(
                        &cmd,
                        &args,
                        working_dir.as_deref(),
                        title.as_deref(),
                    )
                } else {
                    self.terminal_manager.create_tab()
                };

                match result {
                    Ok(_) => {
                        self.update_tab_bar();
                        self.needs_redraw = true;
                        ControlResponse::Ok
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

            ControlRequest::RunCommand { command, args } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal_mut() {
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
                let _ = self
                    .window
                    .request_inner_size(PhysicalSize::new(width, height));
                ControlResponse::Ok
            }

            ControlRequest::SetOpacity { opacity } => {
                self.renderer.set_opacity(opacity.clamp(0.0, 1.0));
                self.needs_redraw = true;
                ControlResponse::Ok
            }

            ControlRequest::Screenshot { path } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let term = terminal.event_loop.terminal();
                    let overlay = OverlayState::default();

                    if let Some(path_str) = path {
                        let path = std::path::PathBuf::from(&path_str);
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        match self.renderer.capture_screenshot(term, &overlay, &path) {
                            Ok(()) => ControlResponse::Data(ControlData::ScreenshotSaved(
                                path.display().to_string(),
                            )),
                            Err(e) => ControlResponse::Error {
                                code: 5,
                                message: format!("Screenshot failed: {}", e),
                            },
                        }
                    } else {
                        match self.renderer.capture_screenshot_bytes(term, &overlay) {
                            Ok(data) => ControlResponse::Data(ControlData::ScreenshotData(data)),
                            Err(e) => ControlResponse::Error {
                                code: 5,
                                message: format!("Screenshot failed: {}", e),
                            },
                        }
                    }
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::Search {
                pattern,
                regex: use_regex,
                case_sensitive,
                include_scrollback,
            } => {
                if let Some(terminal) = self.terminal_manager.focused_terminal() {
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

                    let buffer = terminal.event_loop.terminal().buffer();
                    let mut matches = Vec::new();

                    let line_count = if include_scrollback {
                        buffer.total_lines()
                    } else {
                        terminal.event_loop.terminal().dimensions().rows as usize
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
                                    let col = m.start();
                                    let match_len = m.len();
                                    let context_start = col.saturating_sub(20);
                                    let context_end = (col + match_len + 20).min(text.len());
                                    matches.push(SearchMatch {
                                        line: row,
                                        col,
                                        text: m.as_str().to_string(),
                                        context_before: text[context_start..col].to_string(),
                                        context_after: text[col + match_len..context_end]
                                            .to_string(),
                                    });
                                }
                            } else {
                                let pattern_search = if case_sensitive {
                                    pattern.clone()
                                } else {
                                    pattern.to_lowercase()
                                };
                                let search_text = if case_sensitive {
                                    text.clone()
                                } else {
                                    text.to_lowercase()
                                };

                                for (col, _) in search_text.match_indices(&pattern_search) {
                                    let context_start = col.saturating_sub(20);
                                    let context_end = (col + pattern.len() + 20).min(text.len());
                                    matches.push(SearchMatch {
                                        line: row,
                                        col,
                                        text: text[col..col + pattern.len()].to_string(),
                                        context_before: text[context_start..col].to_string(),
                                        context_after: text[col + pattern.len()..context_end]
                                            .to_string(),
                                    });
                                }
                            }
                        }
                    }

                    ControlResponse::Data(ControlData::SearchResults(matches))
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::WaitFor {
                pattern,
                regex: use_regex,
                timeout_ms,
            } => {
                let compiled_regex = if use_regex {
                    let regex_pattern = format!("(?i){}", pattern);
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

                if let Some(terminal) = self.terminal_manager.focused_terminal() {
                    let start = std::time::Instant::now();
                    let timeout = std::time::Duration::from_millis(timeout_ms);
                    let pattern_lower = pattern.to_lowercase();

                    let mut found: Option<(usize, usize)> = None;

                    while start.elapsed() < timeout && found.is_none() {
                        let buffer = terminal.event_loop.terminal().buffer();
                        let total_lines = buffer.total_lines();

                        for row in 0..total_lines {
                            if let Some(grid_row) = buffer.row_at(row) {
                                let text = grid_row.text_trimmed();

                                if let Some(ref re) = compiled_regex {
                                    if let Some(m) = re.find(&text) {
                                        found = Some((row, m.start()));
                                        break;
                                    }
                                } else {
                                    let text_lower = text.to_lowercase();
                                    if let Some(col) = text_lower.find(&pattern_lower) {
                                        found = Some((row, col));
                                        break;
                                    }
                                }
                            }
                        }

                        if found.is_none() {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }

                    match found {
                        Some((line, col)) => {
                            ControlResponse::Data(ControlData::WaitMatched { line, col })
                        }
                        None => ControlResponse::Data(ControlData::WaitTimeout),
                    }
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::Batch { commands } => {
                let results: Vec<ControlResponse> = commands
                    .into_iter()
                    .map(|cmd| self.process_control_request(client_key, cmd))
                    .collect();
                ControlResponse::Data(ControlData::BatchResults(results))
            }

            ControlRequest::Authenticate { token } => {
                let expected =
                    std::fs::read_to_string(ultty_protocol::control::auth_token_path()).ok();

                if let Some(expected_token) = expected {
                    if token.trim() == expected_token.trim() {
                        if let Some(ref mut server) = self.control_server {
                            server.set_client_authenticated(client_key, true);
                        }
                        ControlResponse::Data(ControlData::Authenticated)
                    } else {
                        ControlResponse::Error {
                            code: 403,
                            message: "Invalid token".to_string(),
                        }
                    }
                } else {
                    ControlResponse::Data(ControlData::Authenticated)
                }
            }
        }
    }

    fn forward_key_to_pty(
        &mut self,
        key: &ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) {
        let Some(terminal) = self.terminal_manager.focused_terminal_mut() else {
            return;
        };

        let ctrl = modifiers.contains(ultty_protocol::Modifiers::CTRL);
        let alt = modifiers.contains(ultty_protocol::Modifiers::ALT);
        let app_cursor = terminal.event_loop.terminal().modes().application_cursor_keys;

        let bytes: Vec<u8> = match key {
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
                if app_cursor { b"\x1bOH".to_vec() } else { b"\x1b[H".to_vec() }
            }
            ultty_protocol::Key::End => {
                if app_cursor { b"\x1bOF".to_vec() } else { b"\x1b[F".to_vec() }
            }
            ultty_protocol::Key::PageUp => b"\x1b[5~".to_vec(),
            ultty_protocol::Key::PageDown => b"\x1b[6~".to_vec(),
            ultty_protocol::Key::Up => {
                if app_cursor { b"\x1bOA".to_vec() } else { b"\x1b[A".to_vec() }
            }
            ultty_protocol::Key::Down => {
                if app_cursor { b"\x1bOB".to_vec() } else { b"\x1b[B".to_vec() }
            }
            ultty_protocol::Key::Right => {
                if app_cursor { b"\x1bOC".to_vec() } else { b"\x1b[C".to_vec() }
            }
            ultty_protocol::Key::Left => {
                if app_cursor { b"\x1bOD".to_vec() } else { b"\x1b[D".to_vec() }
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
        };

        if !bytes.is_empty() {
            let _ = terminal.event_loop.send_input(&bytes);
        }
    }
}
