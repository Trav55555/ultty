use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::{parse_unified_diff, DiffKind, RichText, Stack};

use crate::sidebar::sidebar_view;
use crate::types::{ApprovalRequest, FileTreeEntry};
use crate::{Msg, PorygonTui};

const SIDEBAR_WIDTH_CELLS: u16 = 34;
const MESSAGE_CHROME_CELLS: u16 = 4;
const MIN_TRANSCRIPT_CONTENT_CELLS: u16 = 80;
const SIDEBAR_MIN_TERMINAL_WIDTH: u16 =
    SIDEBAR_WIDTH_CELLS + MESSAGE_CHROME_CELLS + MIN_TRANSCRIPT_CONTENT_CELLS;

pub(crate) fn format_file_tree(entry: &FileTreeEntry, prefix: &str, is_last: bool) -> String {
    let connector = if is_last { "└── " } else { "├── " };
    let icon = if entry.is_dir { "📁 " } else { "📄 " };
    let mut result = format!("{prefix}{connector}{icon}{}\n", entry.name);
    let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
    for (i, child) in entry.children.iter().enumerate() {
        let child_is_last = i == entry.children.len() - 1;
        result.push_str(&format_file_tree(child, &child_prefix, child_is_last));
    }
    result
}

pub(crate) fn format_tool_result_with_diff(result: &str) -> String {
    if result.contains("@@") && (result.contains("---") || result.contains("+++")) {
        let diff_lines = parse_unified_diff(result);
        let mut formatted = String::new();
        for line in &diff_lines {
            let prefix = match line.kind {
                DiffKind::Added => "+ ",
                DiffKind::Removed => "- ",
                DiffKind::Header => "  ",
                DiffKind::Context => "  ",
            };
            formatted.push_str(&format!("{prefix}{}\n", line.content));
        }
        formatted
    } else {
        result.to_string()
    }
}

/// Seconds since the Unix epoch, saturating to zero if the clock is before it.
pub(crate) fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Local broken-down time for a Unix timestamp.
pub(crate) fn local_time(secs: u64) -> libc::tm {
    let time_t = secs as libc::time_t;
    // SAFETY: `libc::tm` is a plain C struct of integers and a raw pointer,
    // for which the all-zero bit pattern is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers refer to live, exclusively borrowed stack values
    // for the duration of the call; `localtime_r` only writes into `tm`.
    unsafe { libc::localtime_r(&time_t, &mut tm) };
    tm
}

pub(crate) fn format_clock() -> String {
    let tm = local_time(unix_now());
    let h = tm.tm_hour as u64;
    let m = tm.tm_min as u64;
    let s = tm.tm_sec as u64;
    format!("{h:02}:{m:02}:{s:02}")
}

pub(crate) fn format_timestamp() -> String {
    let tm = local_time(unix_now());
    let h = tm.tm_hour as u64;
    let m = tm.tm_min as u64;
    format!("{h:02}:{m:02}")
}

impl PorygonTui {
    fn message_content_width_cols(&self) -> usize {
        let sidebar_width = if self.should_show_sidebar() {
            SIDEBAR_WIDTH_CELLS
        } else {
            0
        };
        self.terminal_width
            .saturating_sub(sidebar_width)
            .saturating_sub(MESSAGE_CHROME_CELLS)
            .max(1) as usize
    }

    fn transcript_height(&self) -> f32 {
        let mut transcript = self.transcript.borrow_mut();
        transcript.sync(
            self.message_content_width_cols(),
            &self.messages,
            &self.tool_calls,
            self.conversation_id.as_deref(),
        );
        transcript.height(self.waiting_response) as f32
    }

    fn composer_rows(&self) -> u16 {
        let width = self
            .terminal_width
            .saturating_sub(if self.should_show_sidebar() {
                SIDEBAR_WIDTH_CELLS
            } else {
                0
            })
            .saturating_sub(1)
            .max(1);
        let max_rows = self.terminal_height.saturating_sub(10).clamp(1, 8);
        (self
            .input
            .visual_line_count(width as usize)
            .max(3)
            .min(max_rows as usize)) as u16
    }

    fn suggestion_rows(&self) -> u16 {
        if self.terminal_height < 20 {
            0
        } else {
            self.suggestions.len().min(3) as u16
        }
    }

    fn bottom_height(&self) -> u16 {
        self.composer_rows() + self.suggestion_rows() + 4
    }

    pub(crate) fn update_scroll_dimensions(&mut self) {
        let viewport = self
            .terminal_height
            .saturating_sub(1 + self.bottom_height() + 2)
            .max(1);
        self.chat_scroll
            .set_content_height(self.transcript_height());
        self.chat_scroll.set_viewport_height(viewport as f32);
    }

    pub(crate) fn auto_scroll_chat(&mut self) {
        self.update_scroll_dimensions();
        if self.follow_output {
            self.chat_scroll.scroll_to_bottom();
        } else {
            self.unread_output = true;
        }
    }

    fn refresh_composer(&mut self) {
        self.persist_conversation();
        let value = self.input.current_value();
        self.suggestions = if value.starts_with('/') && !value.contains('\n') {
            self.command_registry
                .completions(value)
                .iter()
                .take(3)
                .map(|cmd| (format!("/{}", cmd.name), cmd.description.to_string()))
                .collect()
        } else {
            Vec::new()
        };
        self.quit_armed = false;
        self.update_scroll_dimensions();
        if self.follow_output {
            self.chat_scroll.scroll_to_bottom();
        }
    }

    fn approval_rows(&self, request: &ApprovalRequest) -> Vec<String> {
        let width = usize::from(self.terminal_width.saturating_sub(2).max(1));
        let risk = if request.risk_score < 0.0 {
            "unknown".into()
        } else {
            format!("{:.0}%", request.risk_score * 100.0)
        };
        let durable_tool_call =
            !self.approval_is_stream && request.scope.as_deref() == Some("tool_call");
        let durable_context = durable_tool_call.then_some(&request.params);
        let scope = if self.approval_is_stream {
            "current stream"
        } else if durable_context
            .and_then(|context| context.get("reason"))
            .and_then(serde_json::Value::as_str)
            == Some("missing_policy_binding")
        {
            "this invocation only"
        } else {
            "durable request"
        };
        let context_line = |key: &str, label: &str| {
            durable_context
                .and_then(|context| context.get(key))
                .and_then(serde_json::Value::as_str)
                .map(|value| format!("{label}: {value}\n"))
                .unwrap_or_default()
        };
        let arguments = durable_context
            .and_then(|context| context.get("params"))
            .unwrap_or(&request.params);
        let content = format!("Tool: {}\nArguments:\n{}\nScope: {scope}\n{}{}{}Request: {}\nReason: {}\nRisk: {risk}\nAffected scope: {}\nReversible: {}",
            request.tool_name,
            serde_json::to_string_pretty(arguments).unwrap_or_else(|error| format!("Could not display arguments: {error}")),
            context_line("run_id", "Run"),
            context_line("principal_id", "Principal"),
            context_line("approval_expires_at", "Expires"),
            request.id, request.reasoning, request.blast_radius,
            match request.reversible {Some(true)=>"yes",Some(false)=>"no",None=>"unknown"},
        );
        crate::work_views::wrap(&content, width)
    }

    fn approval_max_scroll(&self) -> usize {
        self.pending_approval
            .as_ref()
            .map(|request| self.approval_rows(request).len())
            .unwrap_or(0)
            .saturating_sub(usize::from(self.terminal_height.saturating_sub(5).max(1)))
    }

    fn approval_page_view(&self, request: &ApprovalRequest) -> Column {
        let available = usize::from(self.terminal_height.saturating_sub(5).max(1));
        let rows = self.approval_rows(request);
        let start = self
            .approval_scroll
            .min(rows.len().saturating_sub(available));
        let mut page = Column::new()
            .width(Dimension::Percent(100.0))
            .height(Dimension::Percent(100.0))
            .padding(1.0)
            .child(RichText::new().text("Approval details | Esc back"))
            .child(
                RichText::new().text(if let Some(reason) = self.approval_decision_blocker() {
                    reason
                } else if self.approval_in_flight {
                    "Decision unconfirmed | r refresh"
                } else {
                    "y approve once | n deny | r refresh"
                }),
            )
            .child(RichText::new().text(format!(
                "Up/Down PgUp/PgDn | rows {}-{} of {}",
                start + 1,
                (start + available).min(rows.len()),
                rows.len()
            )));
        for row in rows.into_iter().skip(start).take(available) {
            page = page.child(RichText::new().text(row));
        }
        page
    }

    pub(crate) fn hex_gauge(&self) -> String {
        let count = 5usize;
        let idx = self.spinner_frame % (count * 2);
        let filled = if idx < count {
            idx + 1
        } else {
            count * 2 - idx - 1
        };
        let mut s = String::new();
        for i in 0..count {
            s.push_str(if i < filled { "⬢" } else { "⬡" });
        }
        s
    }

    fn header_view(&self) -> Row {
        let connection = if self.connected {
            "connected"
        } else {
            "disconnected"
        };
        let stream_decisions =
            usize::from(self.approval_is_stream && self.pending_approval.is_some())
                + self.queued_stream_approvals.len();
        let attention = self
            .work
            .notice
            .map(|count| (count + stream_decisions).to_string())
            .unwrap_or_else(|| {
                if stream_decisions > 0 {
                    format!("{stream_decisions}+")
                } else {
                    "?".into()
                }
            });
        let shortcuts = if self.terminal_width < 80 {
            format!(" F1 help F2 F3 F4({attention}) ")
        } else {
            format!(" F1 help F2 sessions F3 work F4 attention {attention} ")
        };
        Row::new()
            .height(Dimension::Cells(1.0))
            .flex_shrink(0.0)
            .child(
                RichText::new()
                    .span(
                        " Porygon ",
                        Style::new().bold().fg(Color::Rgb(58, 224, 221)),
                    )
                    .span(
                        connection,
                        Style::new().fg(if self.connected {
                            Color::Rgb(166, 227, 161)
                        } else {
                            Color::Rgb(249, 226, 175)
                        }),
                    ),
            )
            .child(Spacer)
            .child(RichText::new().span(shortcuts, Style::default()))
    }

    fn messages_view(&self) -> Column {
        let mut transcript = self.transcript.borrow_mut();
        transcript.sync(
            self.message_content_width_cols(),
            &self.messages,
            &self.tool_calls,
            self.conversation_id.as_deref(),
        );
        transcript.view(
            self.chat_scroll.scroll_position().1.max(0.0) as usize,
            self.terminal_height
                .saturating_sub(1 + self.bottom_height() + 2)
                .max(1) as usize,
            self.waiting_response,
            self.current_response,
            self.follow_output,
        )
    }

    pub(crate) fn composer_hint(&self) -> &'static str {
        if self.newline_prefix {
            "Enter: insert newline | Esc: cancel chord"
        } else if self.pending_approval.is_some() {
            "Decision pending | F4: inspect | draft kept here"
        } else if self.waiting_response || self.stopping {
            "Working | draft kept here | Ctrl+C: stop"
        } else if self.run_unresolved && !self.run_can_continue {
            "Run unresolved | draft kept here | /reconnect"
        } else if !self.connected {
            "Disconnected | draft kept here | /reconnect"
        } else if self.run_can_continue {
            "Enter: continue waiting run | Ctrl+X Enter: newline"
        } else {
            "Enter: send | Ctrl+X Enter: newline"
        }
    }

    fn input_view(&self) -> Column {
        let mut col = Column::new()
            .height(Dimension::Cells(
                (self.composer_rows() + self.suggestion_rows() + 2) as f32,
            ))
            .flex_shrink(0.0)
            .child(Text::new(self.composer_hint()))
            .child(self.input.clone().height(self.composer_rows()));
        for (name, description) in self
            .suggestions
            .iter()
            .take(self.suggestion_rows() as usize)
        {
            col = col.child(Text::new(format!("{name}  {description}")));
        }
        col.child(Text::new(if self.suggestions.is_empty() {
            "Shift+Enter: newline where supported | / commands"
        } else {
            "Ctrl+Space: complete | Tab: change focus"
        }))
    }

    fn status_bar_view(&self) -> Row {
        let text = if self.unread_output {
            "New output | Ctrl+End: latest".to_string()
        } else if !self.follow_output {
            "Reading history | Ctrl+End: latest".to_string()
        } else {
            self.status_step_text()
        };
        Row::new()
            .height(Dimension::Cells(1.0))
            .flex_shrink(0.0)
            .child(Text::new(text))
    }

    pub(crate) fn status_step_text(&self) -> String {
        if self.pending_approval.is_some() {
            return if self.status.step.contains("approval required") {
                self.status.step.clone()
            } else {
                "approval required (F4)".into()
            };
        }
        if self.stopping {
            return "stop requested; awaiting daemon".into();
        }
        if self.waiting_response && self.stream_token_count > 0 {
            format!("streaming: {} tokens", self.stream_token_count)
        } else {
            self.status.step.clone()
        }
    }

    pub(crate) fn should_show_sidebar(&self) -> bool {
        self.terminal_width >= SIDEBAR_MIN_TERMINAL_WIDTH
    }

    pub(crate) fn handle_event(&mut self, event: &Event) -> Option<Msg> {
        if let Event::Resize { width, height } = event {
            self.terminal_width = *width;
            self.terminal_height = *height;
            self.update_scroll_dimensions();
            if self.follow_output {
                self.chat_scroll.scroll_to_bottom();
            }
            return None;
        }
        if !self.show_approval_modal {
            if let Event::Key(key) = event {
                match key.key {
                    Key::F(1) => {
                        self.conversations.open = false;
                        self.work.page = crate::work::Page::Closed;
                        self.show_help = !self.show_help;
                        return None;
                    }
                    Key::F(2) => {
                        self.show_help = false;
                        self.work.page = crate::work::Page::Closed;
                        return Some(Msg::OpenConversations);
                    }
                    Key::F(3) | Key::F(4) => {
                        self.conversations.open = false;
                        self.show_help = false;
                        return Some(Msg::OpenWork(if key.key == Key::F(3) {
                            crate::work::Page::Work
                        } else {
                            crate::work::Page::Attention
                        }));
                    }
                    _ => {}
                }
            }
        }
        if self.conversations.open && !matches!(event, Event::Quit) {
            return self.conversation_event(event);
        }
        if self.show_help && !matches!(event, Event::Quit) {
            if matches!(event, Event::Key(ke) if matches!(ke.key, Key::Escape | Key::F(1))) {
                self.show_help = false;
            }
            return None;
        }
        if self.show_approval_modal {
            self.approval_scroll = self.approval_scroll.min(self.approval_max_scroll());
            return match event {
                Event::Quit => Some(Msg::AbortStream),
                Event::Key(ke) => match ke.key {
                    Key::Escape => {
                        self.approval_scroll = 0;
                        Some(Msg::DismissApproval)
                    }
                    Key::Up | Key::PageUp => {
                        self.approval_scroll =
                            self.approval_scroll.saturating_sub(if ke.key == Key::Up {
                                1
                            } else {
                                usize::from(self.terminal_height.saturating_sub(5).max(1))
                            });
                        None
                    }
                    Key::Down | Key::PageDown => {
                        self.approval_scroll =
                            self.approval_scroll.saturating_add(if ke.key == Key::Down {
                                1
                            } else {
                                usize::from(self.terminal_height.saturating_sub(5).max(1))
                            });
                        self.approval_scroll = self.approval_scroll.min(self.approval_max_scroll());
                        None
                    }
                    Key::Home => {
                        self.approval_scroll = 0;
                        None
                    }
                    Key::End => {
                        self.approval_scroll = self.approval_max_scroll();
                        None
                    }
                    Key::Char('r') => Some(Msg::RefreshApproval),
                    Key::Char('y') if !self.approval_in_flight => Some(Msg::ApprovalDecision(true)),
                    Key::Char('n') if !self.approval_in_flight => {
                        Some(Msg::ApprovalDecision(false))
                    }
                    _ => None,
                },
                _ => None,
            };
        }
        if self.work.page != crate::work::Page::Closed && !matches!(event, Event::Quit) {
            self.work.detail_scroll = self
                .work
                .detail_scroll
                .min(crate::work_views::max_scroll(self));
            if let Event::Key(ke) = event {
                match ke.key {
                    Key::Escape => {
                        self.work.page = if self.work.page == crate::work::Page::Detail {
                            crate::work::Page::Work
                        } else {
                            crate::work::Page::Closed
                        };
                    }
                    Key::Up | Key::PageUp if self.work.page == crate::work::Page::Detail => {
                        self.work.detail_scroll =
                            self.work
                                .detail_scroll
                                .saturating_sub(if ke.key == Key::Up {
                                    1
                                } else {
                                    usize::from(self.terminal_height.saturating_sub(3).max(1))
                                });
                    }
                    Key::Down | Key::PageDown if self.work.page == crate::work::Page::Detail => {
                        self.work.detail_scroll = self
                            .work
                            .detail_scroll
                            .saturating_add(if ke.key == Key::Down {
                                1
                            } else {
                                usize::from(self.terminal_height.saturating_sub(3).max(1))
                            })
                            .min(crate::work_views::max_scroll(self));
                    }
                    Key::Home if self.work.page == crate::work::Page::Detail => {
                        self.work.detail_scroll = 0;
                    }
                    Key::End if self.work.page == crate::work::Page::Detail => {
                        self.work.detail_scroll = crate::work_views::max_scroll(self);
                    }
                    Key::Up => self.work.selected = self.work.selected.saturating_sub(1),
                    Key::Down => {
                        let len = if self.work.page == crate::work::Page::Work {
                            self.work.runs.len()
                        } else {
                            crate::work::attention(
                                &self.work,
                                self.pending_approval
                                    .as_ref()
                                    .filter(|_| self.approval_is_stream)
                                    .map(|r| r.id.as_str()),
                            )
                            .len()
                        };
                        self.work.selected = (self.work.selected + 1).min(len.saturating_sub(1));
                    }
                    Key::Enter => return Some(Msg::InspectWork),
                    Key::Char('c') if self.work.page == crate::work::Page::Detail => {
                        return Some(Msg::CopyReviewedWork);
                    }
                    Key::Char('a') if self.work.page == crate::work::Page::Detail => {
                        return Some(Msg::AttachWork)
                    }
                    Key::Char('r') => return Some(Msg::RefreshWork),
                    _ => {}
                }
            }
            return None;
        }
        match event {
            Event::Quit if self.waiting_response => Some(Msg::AbortStream),
            Event::Quit if !self.input.current_value().is_empty() && !self.quit_armed => {
                self.quit_armed = true;
                Some(Msg::DraftExitWarning)
            }
            Event::Quit => Some(Msg::ExitRequested),
            Event::Submit { id } if id == "chat-input" => Some(Msg::SendMessage),
            Event::Change { id, value } if id == "chat-input" => {
                self.input.set_value(value.clone());
                self.refresh_composer();
                None
            }
            Event::Paste { text, focused_id } if focused_id.as_deref() == Some("chat-input") => {
                self.newline_prefix = false;
                self.input.insert_text(text);
                self.refresh_composer();
                None
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    ulttui::event::MouseEventKind::ScrollUp => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_by(-3.0);
                        self.follow_output = false;
                    }
                    ulttui::event::MouseEventKind::ScrollDown => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_by(3.0);
                        self.follow_output = self.chat_scroll.is_near_bottom(0.0);
                        if self.follow_output {
                            self.unread_output = false;
                        }
                    }
                    _ => {}
                }
                self.persist_conversation();
                None
            }
            Event::Key(ke) => {
                if ke.key == Key::Char('y') && ke.modifiers.ctrl {
                    return Some(Msg::CopyLatestAssistant);
                }
                match ke.key {
                    Key::PageUp | Key::PageDown => {
                        self.update_scroll_dimensions();
                        let delta = self.chat_scroll.viewport_height()
                            * if ke.key == Key::PageUp { -1.0 } else { 1.0 };
                        self.chat_scroll.scroll_by(delta);
                        self.follow_output =
                            ke.key == Key::PageDown && self.chat_scroll.is_near_bottom(0.0);
                        if self.follow_output {
                            self.unread_output = false;
                        }
                        self.persist_conversation();
                        return None;
                    }
                    Key::Home if ke.modifiers.ctrl => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_to(0.0);
                        self.follow_output = false;
                        self.persist_conversation();
                        return None;
                    }
                    Key::End if ke.modifiers.ctrl => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_to_bottom();
                        self.follow_output = true;
                        self.unread_output = false;
                        self.persist_conversation();
                        return None;
                    }
                    _ => {}
                }
                if ke.focused_id.as_deref() == Some("chat-input") {
                    if ke.key == Key::Escape {
                        self.newline_prefix = false;
                        self.suggestions.clear();
                        return None;
                    }
                    if ke.key == Key::Char('x') && ke.modifiers.ctrl {
                        self.newline_prefix = true;
                        return None;
                    }
                    let newline = self.newline_prefix || ke.modifiers.shift;
                    self.newline_prefix = false;
                    if ke.key == Key::Enter {
                        if newline {
                            self.input.insert_text("\n");
                            self.refresh_composer();
                            return None;
                        }
                        return Some(Msg::SendMessage);
                    }
                    if ke.key == Key::Char(' ') && ke.modifiers.ctrl {
                        if let Some((first, _)) = self.suggestions.first() {
                            self.input.set_value(format!("{first} "));
                        }
                    } else {
                        let _ = self.input.on_key(ke);
                    }
                    self.refresh_composer();
                } else {
                    self.newline_prefix = false;
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn render_view(&self) -> Stack {
        let bottom_bar = Column::new()
            .height(Dimension::Cells(self.bottom_height() as f32))
            .flex_shrink(0.0)
            .child(Divider::horizontal())
            .child(self.input_view())
            .child(self.status_bar_view());
        let chat_area = self.messages_view();
        let center_content = Column::new()
            .overflow_y_hidden()
            .child(chat_area)
            .child(bottom_bar);
        let mut dock = DockLayout::new().dock_top(self.header_view());
        if self.should_show_sidebar() {
            dock = dock.dock_right(sidebar_view(self));
        }
        let main_content = dock.center(center_content);
        if self.conversations.open {
            return Stack::new().child(self.conversation_view());
        }
        if self.show_help {
            let mut help = Column::new()
                .width(Dimension::Percent(100.0))
                .height(Dimension::Percent(100.0))
                .padding(1.0);
            for line in [
                "Keys and commands",
                "F2: conversations; search, Enter to open, Ctrl+N new.",
                "F3: work and run review. F4: attention.",
                "In conversations, Ctrl+L loads older conversations.",
                "Esc or F1: back to your draft",
                "",
                "Enter: send when ready; keep draft while busy.",
                "Ctrl+X then Enter: newline.",
                "Shift+Enter: newline when terminal supports it.",
                "Paste: insert text without sending.",
                "Ctrl+Y: copy raw reply. Review: c copies evidence.",
                "Native: drag selects; Ctrl+Shift+C: copy selection.",
                "Native Ultty: Ctrl+click a link to open it.",
                "ANSI: use your terminal's selection and copy controls.",
                "Ctrl+C: stop active work.",
                "Ctrl+C when idle: save draft and quit.",
                "With a draft, press Ctrl+C twice to quit.",
                "PgUp/PgDn: history. Ctrl+Home/End: oldest/latest.",
                "Type / for commands. Ctrl+Space: complete.",
                "/reconnect: inspect. /attach ID: read a run.",
            ] {
                help = help.child(Text::new(line));
            }
            return Stack::new().child(help);
        }
        if self.work.page != crate::work::Page::Closed && !self.show_approval_modal {
            return Stack::new().child(crate::work_views::view(self));
        }
        if self.show_approval_modal {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(self.approval_page_view(req));
            }
        }
        Stack::new().child(main_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulttui::event::{KeyEvent, Modifiers};
    use ulttui::widgets::wrapped_line_count;

    fn composer_key(key: Key, ctrl: bool) -> Event {
        Event::Key(KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                ..Modifiers::default()
            },
            focused_id: Some("chat-input".into()),
        })
    }

    #[test]
    fn long_approval_is_readable_and_enter_cannot_decide() {
        let mut app = PorygonTui::new();
        app.terminal_width = 40;
        app.terminal_height = 12;
        app.pending_approval = Some(ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("fixture-version".into()),
            decision_stale: false,
            id: "approval-scoped-1".into(),
            tool_name: "write_file".into(),
            params: serde_json::json!({"content": format!("{}TAIL-ARGUMENT", "long value ".repeat(80))}),
            risk_score: -1.0,
            reasoning: "Inspect the complete request".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        });
        app.show_approval_modal = true;
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        app.handle_event(&composer_key(Key::End, false));
        assert!(app.approval_scroll > 20);
        let native = ulttui::widgets::render_view_messages(
            &app.approval_page_view(app.pending_approval.as_ref().unwrap()),
        )
        .unwrap();
        assert!(!native.iter().any(|message| matches!(message,
            ultty_protocol::AppMessage::SetContent { content: ultty_protocol::Content::Text(text), .. }
            if text.text.trim() == "{" || text.text.trim() == "}")));
        assert!(native.iter().any(|message| matches!(message,
            ultty_protocol::AppMessage::SetContent { content: ultty_protocol::Content::RichText(text), .. }
            if text.spans.iter().any(|span| span.text.trim() == "}"))));

        let render_screen = |app: &PorygonTui| {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
            terminal
                .draw(|frame| {
                    app.approval_page_view(app.pending_approval.as_ref().unwrap())
                        .render_ansi(frame, frame.area())
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let final_page = render_screen(&app);
        assert!(final_page.contains("}"), "{final_page}");
        let end = app.approval_scroll;
        for _ in 0..20 {
            app.handle_event(&composer_key(Key::Down, false));
        }
        assert_eq!(app.approval_scroll, end);
        let mut action_reachable = false;
        while app.approval_scroll > 0 && !action_reachable {
            app.handle_event(&composer_key(Key::Up, false));
            let page = render_screen(&app);
            action_reachable = page.contains("TAIL-ARGUMENT");
        }
        assert!(
            action_reachable,
            "action arguments were not reachable by paging"
        );
        assert!(matches!(
            app.handle_event(&composer_key(Key::Char('n'), false)),
            Some(Msg::ApprovalDecision(false))
        ));
    }

    #[test]
    fn paste_and_newline_chord_edit_without_submitting() {
        let mut app = PorygonTui::new();
        assert!(app
            .handle_event(&Event::Paste {
                text: "first\r\n界 second".into(),
                focused_id: Some("chat-input".into()),
            })
            .is_none());
        assert!(app
            .handle_event(&composer_key(Key::Char('x'), true))
            .is_none());
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        assert_eq!(app.input.current_value(), "first\n界 second\n");
        assert!(!app.waiting_response);
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::SendMessage)
        ));
    }

    #[test]
    fn composer_accepts_typing_immediately_and_after_escape() {
        let mut app = PorygonTui::new();
        for ch in "hello".chars() {
            app.handle_event(&composer_key(Key::Char(ch), false));
        }
        app.handle_event(&composer_key(Key::Escape, false));
        app.handle_event(&composer_key(Key::Char('!'), false));
        assert_eq!(app.input.current_value(), "hello!");
    }

    #[test]
    fn busy_enter_keeps_draft_and_escape_does_not_stop() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;
        app.input.set_value("next\nthought");
        let message = app.handle_event(&composer_key(Key::Enter, false)).unwrap();
        app.handle_update(message);
        assert_eq!(app.input.current_value(), "next\nthought");
        assert!(app
            .handle_event(&composer_key(Key::Escape, false))
            .is_none());
        assert!(app.waiting_response);
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::AbortStream)
        ));
    }

    #[test]
    fn new_output_keeps_the_readers_position_until_explicit_follow() {
        let mut app = PorygonTui::new();
        app.terminal_width = 80;
        app.terminal_height = 24;
        app.messages[0].content = (0..100).map(|i| format!("line {i}\n\n")).collect();
        app.auto_scroll_chat();
        app.handle_event(&composer_key(Key::PageUp, false));
        let anchor = app.chat_scroll.scroll_position();
        app.messages[0].content.push_str("new output\n\n");
        app.auto_scroll_chat();
        assert_eq!(app.chat_scroll.scroll_position(), anchor);
        assert!(app.unread_output);
        app.handle_event(&composer_key(Key::End, true));
        assert!(!app.unread_output);
        assert!(app.chat_scroll.scroll_position().1 > anchor.1);
    }

    #[test]
    fn idle_quit_requires_confirmation_to_discard_a_draft() {
        let mut app = PorygonTui::new();
        app.input.set_value("keep me");
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::DraftExitWarning)
        ));
        assert_eq!(app.input.current_value(), "keep me");
        app.handle_event(&composer_key(Key::Char('!'), false));
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::DraftExitWarning)
        ));
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::ExitRequested)
        ));
    }

    #[test]
    fn help_keeps_stop_and_draft_confirmation_active() {
        let mut app = PorygonTui::new();
        app.show_help = true;
        app.input.set_value("keep me");
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::DraftExitWarning)
        ));
        app.waiting_response = true;
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::AbortStream)
        ));
    }

    #[test]
    fn help_page_keeps_title_keys_and_return_action_visible() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for (width, height) in [(61, 30), (80, 24)] {
            let mut app = PorygonTui::new();
            app.show_help = true;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            for expected in [
                "Keys and commands",
                "F2: conversations; search, Enter to open, Ctrl+N new.",
                "In conversations, Ctrl+L loads older conversations.",
                "back to your draft",
                "Ctrl+X then Enter: newline.",
                "save draft and quit.",
                "/attach ID: read a run.",
            ] {
                assert!(
                    screen.contains(expected),
                    "{width}x{height}: missing {expected}"
                );
            }
        }
    }

    #[test]
    fn conversation_and_multiline_draft_are_visible_at_both_sizes() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for (width, height) in [(80, 24), (160, 48)] {
            let mut app = PorygonTui::new();
            app.terminal_width = width;
            app.terminal_height = height;
            app.messages[0].content = "Visible assistant response".into();
            app.input.set_value("draft first\ndraft second");
            ulttui::focus::FocusManager::with_current_mut(|focus| {
                focus.register("chat-input".into());
                focus.focus("chat-input");
            });
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
                .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                screen.contains("Visible assistant response"),
                "{width}x{height}: {screen}"
            );
            assert!(screen.contains("draft first"), "{width}x{height}: {screen}");
            assert!(
                screen.contains("draft second"),
                "{width}x{height}: {screen}"
            );
        }
    }

    #[test]
    fn wrapped_line_count_accounts_for_long_words() {
        assert_eq!(wrapped_line_count("abcdefghijkl", 5), 3);
    }

    #[test]
    fn wrapped_line_count_preserves_explicit_blank_lines() {
        assert_eq!(wrapped_line_count("alpha\n\nbeta", 20), 3);
    }

    #[test]
    fn message_card_height_grows_for_narrow_transcript_lines() {
        let mut app = PorygonTui::new();
        app.terminal_width = 75;

        app.messages[0].content = "short line".into();
        let short = app.transcript_height();
        app.messages[0].content =
            "- **Project DNA:** AGENTS.md found at /home/travers/projects/tui-terminal/AGENTS.md"
                .into();
        let long = app.transcript_height();

        assert!(long > short, "long={long} short={short}");
    }

    #[test]
    fn sidebar_breakpoint_keeps_transcript_content_width_usable() {
        let mut app = PorygonTui::new();
        let widths = [99, 100, 102];
        let content_widths: Vec<_> = widths
            .into_iter()
            .map(|width| {
                app.terminal_width = width;
                app.message_content_width_cols()
            })
            .collect();

        assert_eq!(content_widths, [95, 96, 98]);
        app.terminal_width = 117;
        assert!(!app.should_show_sidebar());
        app.terminal_width = 118;
        assert!(app.should_show_sidebar());
        assert_eq!(app.message_content_width_cols(), 80);
    }

    #[test]
    fn durable_approval_shows_action_context_before_arguments() {
        let mut app = PorygonTui::new();
        app.terminal_width = 60;
        let request = ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("version-1".into()),
            decision_stale: false,
            id: "approval-1".into(),
            tool_name: "shell".into(),
            params: serde_json::json!({
                "run_id": "run-1",
                "principal_id": "owner-1",
                "approval_expires_at": "2099-01-01T00:00:00Z",
                "reason": "missing_policy_binding",
                "call_hash": "low-level-hash",
                "params": {"argv": ["git", "status", "--short"]}
            }),
            risk_score: -1.0,
            reasoning: "Authorize this exact action once".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        };

        let rows = app.approval_rows(&request).join("\n");
        assert!(rows.find("Arguments:").unwrap() < rows.find("Run: run-1").unwrap());
        assert!(rows.contains("Principal: owner-1"));
        assert!(rows.contains("Expires: 2099-01-01T00:00:00Z"));
        assert!(rows.contains("Scope: this invocation only"));
        assert!(rows.contains("git"));
        assert!(!rows.contains("low-level-hash"));
        assert!(rows.contains("Authorize this exact action once"));
    }

    #[test]
    fn pending_approval_status_precedes_stream_token_count() {
        let mut app = PorygonTui::new();
        app.waiting_response = true;
        app.stream_token_count = 42;
        app.status.step = "blocked; approval required (F4)".into();
        app.pending_approval = Some(ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("version-1".into()),
            decision_stale: false,
            id: "approval-1".into(),
            tool_name: "shell".into(),
            params: serde_json::json!({}),
            risk_score: -1.0,
            reasoning: "Approve once".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        });
        assert_eq!(app.status_step_text(), "blocked; approval required (F4)");
    }
}
