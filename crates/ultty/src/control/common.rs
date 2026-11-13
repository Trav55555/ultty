//! Shared, window-independent control request helpers.

use std::io;

use crate::terminal_manager::TerminalId;

const PANE_NOT_FOUND: u16 = 6;
const INPUT_UNAVAILABLE: u16 = 11;
const INPUT_QUEUE_FULL: u16 = 12;
const INPUT_FAILED: u16 = 13;

/// What `ulttyctl info` reports for `terminal`.
///
/// The terminal lock is released before the working directory is looked up,
/// because that lookup takes the lock again; holding it deadlocked the UI
/// thread on every `info` request.
pub fn terminal_info(
    terminal: &crate::terminal_manager::ManagedTerminal,
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

/// The pane a request acts on: `target` (an id from `ListPanes`), or the
/// focused pane when it is omitted.
pub fn resolve_control_target(
    manager: &crate::terminal_manager::TerminalManager,
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
pub fn pane_infos(
    manager: &crate::terminal_manager::TerminalManager,
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

/// Shared control-plane helper.
pub fn terminal_working_directory(
    terminal: &crate::terminal_manager::ManagedTerminal,
) -> Option<String> {
    terminal
        .event_loop
        .terminal()
        .working_directory()
        .map(str::to_string)
        .or_else(|| terminal.event_loop.child_working_directory())
}

/// The text of `buffer`: up to `history_lines` of the most recent scrollback,
/// then the screen, one line per row. Each row has one char per column
/// (wide-char spacers print as spaces) plus the extra codepoints of each
/// grapheme. This ignores the viewport's scroll position.
pub fn buffer_text(buffer: &crate::grid::Buffer, history_lines: usize) -> String {
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

/// Convert a protocol key (with modifier flags) to the byte sequence expected by a PTY.
///
/// `app_cursor` controls whether application cursor key mode (DECCKM) sequences
/// are emitted for arrow/Home/End keys.
pub fn key_to_bytes(key: &ultty_protocol::Key, ctrl: bool, alt: bool, app_cursor: bool) -> Vec<u8> {
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

/// Legacy shell text for `RunCommand`: arguments are joined with spaces,
/// without shell escaping. Use new-tab or split to preserve process argv.
pub fn build_command_line(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        format!("{}\n", command)
    } else {
        format!("{} {}\n", command, args.join(" "))
    }
}

/// Queue bytes through the same PTY path used by control text, raw input,
/// and shell typing. Success means accepted into the input queue only.
pub fn send_pty_input(
    manager: &mut crate::terminal_manager::TerminalManager,
    pane: TerminalId,
    bytes: &[u8],
) -> ultty_protocol::control::ControlResponse {
    let result = manager
        .terminal_mut(pane)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the target pane is gone"))
        .and_then(|terminal| terminal.event_loop.send_input(bytes));
    input_acceptance(result)
}

/// Shared control-plane helper.
pub fn submit_control_shell_command(
    manager: &mut crate::terminal_manager::TerminalManager,
    pane: TerminalId,
    command: &str,
) -> Result<u64, ultty_protocol::control::ControlResponse> {
    let result = manager
        .terminal_mut(pane)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the target pane is gone"))
        .and_then(|terminal| terminal.event_loop.submit_shell_command(command));
    result.map_err(|error| input_acceptance(Err(error)))
}

/// Shared control-plane helper.
pub fn shell_command_response(
    pane: TerminalId,
    sequence: u64,
    outcome: crate::event_loop::ShellCommandOutcome,
) -> ultty_protocol::control::ControlResponse {
    use crate::event_loop::ShellCommandOutcome;
    use ultty_protocol::control::{ControlData, ControlResponse};

    match outcome {
        ShellCommandOutcome::Finished { exit_code } => ControlResponse::Data(
            ControlData::ShellCommandFinished { pane: pane.raw() as u64, sequence, exit_code },
        ),
        ShellCommandOutcome::Cancelled => ControlResponse::Error {
            code: 14,
            message: format!("Shell submission {sequence} in pane {} was cancelled by prompt, reset or process exit", pane.raw()),
        },
    }
}

/// Shared control-plane helper.
pub fn job_id(job: u64) -> Result<TerminalId, ultty_protocol::control::ControlResponse> {
    usize::try_from(job).map(TerminalId::new).map_err(|_| {
        ultty_protocol::control::ControlResponse::Error {
            code: 404,
            message: format!("Unknown or expired job {job}"),
        }
    })
}

/// Shared control-plane helper.
pub fn job_is_running(response: &ultty_protocol::control::ControlResponse) -> bool {
    matches!(
        response,
        ultty_protocol::control::ControlResponse::Data(
            ultty_protocol::control::ControlData::ProcessJob {
                outcome: ultty_protocol::control::ProcessJobOutcome::Running,
                ..
            }
        )
    )
}

/// Shared control-plane helper.
pub fn process_job_response(
    manager: &crate::terminal_manager::TerminalManager,
    id: TerminalId,
) -> ultty_protocol::control::ControlResponse {
    use crate::terminal_manager::ProcessJobOutcome as Source;
    use ultty_protocol::control::{ControlData, ControlResponse, ProcessJobOutcome};
    let Some(job) = manager.job(id) else {
        return ControlResponse::Error {
            code: 404,
            message: format!("Unknown or expired job {}", id.raw()),
        };
    };
    ControlResponse::Data(ControlData::ProcessJob {
        job: id.raw() as u64,
        pid: job.pid,
        outcome: match job.outcome {
            Source::Running => ProcessJobOutcome::Running,
            Source::Exited { status } => ProcessJobOutcome::Exited { status },
            Source::Closed => ProcessJobOutcome::Closed,
        },
    })
}

/// Shared control-plane helper.
pub fn program_status_response(
    manager: &crate::terminal_manager::TerminalManager,
    pane: TerminalId,
) -> ultty_protocol::control::ControlResponse {
    use ultty_protocol::control::{ControlData, ControlResponse};
    let Some(terminal) = manager.terminal(pane) else {
        return input_acceptance(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the target pane is gone",
        )));
    };
    let snapshot = map_program_status(terminal.event_loop.terminal().program_status_snapshot());
    ControlResponse::Data(ControlData::ProgramStatus {
        generation: snapshot.generation,
        reason: snapshot.reason,
        records: snapshot.records,
    })
}

/// Shared control-plane helper.
pub fn map_program_status(
    snapshot: ultty_vt::program_status::ProgramStatusSnapshot,
) -> ultty_protocol::control::ProgramStatusSnapshot {
    use ultty_protocol::control::{
        BlockedKind, ProgramState, ProgramStatusRecord, ProgramStatusSnapshot,
    };
    use ultty_vt::program_status::{BlockedKind as SourceKind, ProgramState as SourceState};
    ProgramStatusSnapshot {
        generation: snapshot.generation,
        reason: super::events::status_reason(snapshot.reason),
        records: snapshot
            .records
            .into_iter()
            .map(|record| ProgramStatusRecord {
                id: record.id,
                state: match record.state {
                    SourceState::Idle => ProgramState::Idle,
                    SourceState::Working => ProgramState::Working,
                    SourceState::Done => ProgramState::Done,
                    SourceState::Blocked => ProgramState::Blocked,
                    SourceState::Error => ProgramState::Error,
                },
                kind: record.kind.map(|kind| match kind {
                    SourceKind::Permission => BlockedKind::Permission,
                    SourceKind::Question => BlockedKind::Question,
                    SourceKind::Auth => BlockedKind::Auth,
                }),
                progress: record.progress,
                app: record.app,
                title: record.title,
                msg: record.msg,
                updated: record.updated,
            })
            .collect(),
    }
}

/// Shared control-plane helper.
pub fn control_pane_snapshot(
    manager: &crate::terminal_manager::TerminalManager,
    target: Option<u64>,
) -> Vec<ultty_protocol::control::ControlPaneSnapshot> {
    pane_infos(manager)
        .into_iter()
        .filter(|pane| target.is_none_or(|id| id == pane.id))
        .filter_map(|pane| {
            let terminal = manager.terminal(TerminalId::new(usize::try_from(pane.id).ok()?))?;
            Some(ultty_protocol::control::ControlPaneSnapshot {
                pane,
                program_status: map_program_status(terminal.observed_program_status().clone()),
            })
        })
        .collect()
}

/// Shared control-plane helper.
pub fn input_acceptance(result: io::Result<()>) -> ultty_protocol::control::ControlResponse {
    use ultty_protocol::control::ControlResponse;

    match result {
        Ok(()) => ControlResponse::Ok,
        Err(error) => {
            let code = match error.kind() {
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported => 4,
                io::ErrorKind::WouldBlock => INPUT_QUEUE_FULL,
                io::ErrorKind::NotFound
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::NotConnected => INPUT_UNAVAILABLE,
                _ => INPUT_FAILED,
            };
            ControlResponse::Error {
                code,
                message: format!("Input was not fully accepted: {error}"),
            }
        }
    }
}

/// Search a pane with the same row indexing and bounded context as the GUI.
pub fn search_response(
    manager: &crate::terminal_manager::TerminalManager,
    pattern: String,
    use_regex: bool,
    case_sensitive: bool,
    include_scrollback: bool,
    target: Option<u64>,
) -> ultty_protocol::control::ControlResponse {
    use super::wait::find_ignore_case;
    use ultty_protocol::control::{ControlData, ControlResponse, SearchMatch};
    let pane = match resolve_control_target(manager, target) {
        Ok(pane) => pane,
        Err(response) => return response,
    };
    if let Some(terminal) = manager.terminal(pane) {
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
                                (text[..start].chars().count(), start, start + matched.len())
                            })
                            .collect()
                    } else {
                        let pattern_lower = pattern.to_lowercase();
                        find_ignore_case(&text, &pattern_lower)
                            .map(|m| (m.char_col, m.byte_start, m.byte_end))
                            .collect()
                    };

                    for (col, byte_start, byte_end) in spans {
                        matches.push(plain_search_match(&text, row, col, byte_start, byte_end));
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

/// Build a search match for `text[byte_start..byte_end]` with up to 20 chars
/// of context on each side. Offsets must be char boundaries of `text`.
pub fn plain_search_match(
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
