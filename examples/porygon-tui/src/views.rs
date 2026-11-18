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
        if self.terminal_height <= 10 {
            0
        } else {
            self.suggestions
                .len()
                .min(5)
                .min(self.terminal_height.saturating_sub(10) as usize) as u16
        }
    }

    fn bottom_height(&self) -> u16 {
        self.composer_rows() + self.suggestion_rows() + 4
    }

    fn approval_panel_height(&self) -> u16 {
        // Room for the host-execution line and two justification lines.
        let host_rows = if self
            .pending_approval
            .as_ref()
            .is_some_and(crate::approval_ui::runs_on_host)
        {
            3
        } else if self
            .pending_approval
            .as_ref()
            .is_some_and(crate::approval_ui::runs_contained)
        {
            1
        } else {
            0
        };
        self.terminal_height
            .saturating_sub(2)
            .clamp(1, 10 + host_rows)
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
        self.suggestions = if value.starts_with('/') && !value.chars().any(char::is_whitespace) {
            self.command_registry
                .completions(value)
                .iter()
                .map(|cmd| (format!("/{}", cmd.name), cmd.description.to_string()))
                .collect()
        } else {
            Vec::new()
        };
        self.suggestion_selected = self
            .suggestion_selected
            .min(self.suggestions.len().saturating_sub(1));
        self.quit_armed = false;
        self.update_scroll_dimensions();
        if self.follow_output {
            self.chat_scroll.scroll_to_bottom();
        }
    }

    fn approval_rows(&self, request: &ApprovalRequest) -> Vec<String> {
        let width = usize::from(self.terminal_width.saturating_sub(2).max(1));
        if self.approval_stage == 1 {
            let width = usize::from(
                self.terminal_width
                    .saturating_sub(4)
                    .min(80)
                    .saturating_sub(2)
                    .max(1),
            );
            let scope = &request.params["session_scope"];
            let location = if crate::approval_ui::runs_on_host(request) {
                format!("{}.\n", crate::approval_ui::HOST_EXECUTION_LINE)
            } else if crate::approval_ui::runs_contained(request) {
                format!("{}.\n", crate::approval_ui::CONTAINED_EXECUTION_LINE)
            } else {
                String::new()
            };
            let content = format!("{location}This exact command may run again in this session until the daemon restarts.\nConversation: {}\nProject: {}\nExact argv: {}", scope["conversation_id"].as_str().unwrap_or(""), scope["project_root"].as_str().unwrap_or(""), scope["argv"]);
            return crate::work_views::wrap(&content, width);
        }
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
        } else if matches!(
            durable_context
                .and_then(|context| context.get("reason"))
                .and_then(serde_json::Value::as_str),
            Some("missing_policy_binding" | "host_execution_request" | "contained_project_work")
        ) {
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
        let location = if crate::approval_ui::runs_on_host(request) {
            format!(
                "{}\nJustification: {}\n",
                crate::approval_ui::HOST_EXECUTION_LINE,
                crate::approval_ui::host_justification(request).unwrap_or("(none given)")
            )
        } else if crate::approval_ui::runs_contained(request) {
            format!("{}\n", crate::approval_ui::CONTAINED_EXECUTION_LINE)
        } else {
            String::new()
        };
        let content = format!("{location}Tool: {}\nArguments:\n{}\nScope: {scope}\n{}{}{}Request: {}\nReason: {}\nRisk: {risk}\nAffected scope: {}\nReversible: {}",
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
            .saturating_sub(if self.approval_stage == 1 {
                usize::from(self.approval_panel_height().saturating_sub(4).max(1))
            } else {
                usize::from(self.terminal_height.saturating_sub(5).max(1))
            })
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
            .child(RichText::new().text(if self.approval_stage == 1 {
                "Confirm session permission | Esc back"
            } else {
                "Approval details | Esc back"
            }))
            .child(
                RichText::new().text(if let Some(reason) = self.approval_decision_blocker() {
                    reason
                } else if self.approval_in_flight {
                    "Decision unconfirmed | r refresh"
                } else if self.approval_stage == 1 {
                    "Read scope; scroll to end, Enter confirms | Esc back"
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

    fn approval_session_view(&self, request: &ApprovalRequest) -> Column {
        let rows = self.approval_rows(request);
        let available = usize::from(self.approval_panel_height().saturating_sub(4).max(1));
        let start = self
            .approval_scroll
            .min(rows.len().saturating_sub(available));
        let mut panel = Column::new()
            .height(Dimension::Cells(f32::from(self.approval_panel_height())))
            .border(BorderStyle::rounded())
            .border_color(Theme::current().primary)
            .bg(Theme::current().background)
            .child(Text::new("Confirm session permission"));
        for row in rows.iter().skip(start).take(available) {
            panel = panel.child(Text::new(row.clone()));
        }
        panel.child(Text::new(if start + available >= rows.len() {
            "Enter confirms | Esc back"
        } else {
            "Scroll to read scope | Esc back"
        }))
    }

    fn approval_menu_view(&self, request: &ApprovalRequest) -> Column {
        let arguments = request.params.get("params").unwrap_or(&request.params);
        let preview = arguments
            .get("argv")
            .and_then(serde_json::Value::as_array)
            .map(|args| {
                args.iter()
                    .map(serde_json::Value::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| arguments.to_string());
        let width = usize::from(
            self.terminal_width
                .saturating_sub(4)
                .min(80)
                .saturating_sub(2)
                .max(1),
        );
        let preview = format!("{}: {preview}", request.tool_name);
        // At most two wrapped lines; a cut-off second line ends with an ellipsis.
        let clipped = |text: &str| {
            let lines = crate::work_views::wrap(text, width);
            let more = lines.len() > 2;
            lines
                .into_iter()
                .take(2)
                .enumerate()
                .map(|(index, line)| {
                    if index == 1 && more {
                        format!(
                            "{} …",
                            line.chars()
                                .take(width.saturating_sub(2))
                                .collect::<String>()
                        )
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut panel = Column::new()
            .height(Dimension::Cells(f32::from(self.approval_panel_height())))
            .flex_shrink(0.0)
            .border(BorderStyle::rounded())
            .border_color(Theme::current().primary)
            .bg(Theme::current().background)
            .child(Text::new("Permission required"));
        for line in clipped(&preview) {
            panel = panel.child(Text::new(line));
        }
        if crate::approval_ui::runs_on_host(request) {
            panel = panel.child(RichText::new().span(
                crate::approval_ui::HOST_EXECUTION_LINE,
                Style::new().fg(Theme::current().warning).bold(),
            ));
            if let Some(reason) = crate::approval_ui::host_justification(request) {
                for line in clipped(&format!("Why: {reason}")) {
                    panel = panel.child(Text::new(line));
                }
            }
        } else if crate::approval_ui::runs_contained(request) {
            panel = panel.child(RichText::new().span(
                crate::approval_ui::CONTAINED_EXECUTION_LINE,
                Style::new().fg(Theme::current().success),
            ));
        }
        if self.approval_stage == 2 {
            return panel
                .child(Text::new("Reject this command. Optional instructions:"))
                .child(self.approval_feedback.clone())
                .child(Text::new("Enter: reject and send | Esc: back"));
        }
        use crate::approval_ui::{
            APPROVE_ONCE_LABEL, APPROVE_SESSION_LABEL, REJECT_FEEDBACK_LABEL, REJECT_LABEL,
        };
        let reject = if self.approval_supports("reject_feedback") && !self.approval_is_stream {
            REJECT_FEEDBACK_LABEL
        } else {
            REJECT_LABEL
        };
        let labels = if self.approval_session_available() {
            vec![APPROVE_ONCE_LABEL, APPROVE_SESSION_LABEL, reject]
        } else {
            vec![APPROVE_ONCE_LABEL, reject]
        };
        for (index, label) in labels.iter().enumerate() {
            let selected = self.approval_choice == index;
            let style = if selected {
                Style::new().fg(Theme::current().primary).bold()
            } else {
                Style::default()
            };
            panel = panel.child(RichText::new().span(
                format!("{} {label}", if selected { ">" } else { " " }),
                style,
            ));
        }
        let hint = if let Some(reason) = self.approval_decision_blocker() {
            reason
        } else if self.approval_in_flight {
            "Decision unconfirmed | r refresh"
        } else {
            "Up/Down Enter | d details | Esc back"
        };
        panel.child(Text::new(hint))
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
            format!(" ^P commands F1 F2 F3 F4({attention}) ")
        } else {
            format!(" Ctrl+P commands F1 help F2 sessions F3 work F4 attention {attention} ")
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
        let count = self.suggestion_rows() as usize;
        let start = self
            .suggestion_selected
            .saturating_sub(count.saturating_sub(1));
        for (index, (name, description)) in
            self.suggestions.iter().enumerate().skip(start).take(count)
        {
            let line = format!(
                "{} {name}  {description}",
                if index == self.suggestion_selected {
                    ">"
                } else {
                    " "
                }
            );
            let width = usize::from(self.terminal_width.max(1));
            let line = crate::work_views::wrap(&line, width)
                .into_iter()
                .next()
                .unwrap_or_default();
            col = col.child(Text::new(line));
        }
        let hint = if self.suggestions.is_empty() {
            "Shift+Enter: newline | / commands"
        } else {
            "↑↓ select · Tab insert · Enter run · Esc"
        };
        let hint = crate::work_views::wrap(hint, usize::from(self.terminal_width.max(1)))
            .into_iter()
            .next()
            .unwrap_or_default();
        col.child(Text::new(hint))
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
                if key.modifiers.ctrl
                    && key.key == Key::Char('p')
                    && !self.conversations.open
                    && self.work.page == crate::work::Page::Closed
                    && !self.show_help
                {
                    self.palette_open = !self.palette_open;
                    self.palette_query.clear();
                    self.suggestion_selected = 0;
                    return None;
                }
                match key.key {
                    Key::F(1) => {
                        self.palette_open = false;
                        self.conversations.open = false;
                        self.work.page = crate::work::Page::Closed;
                        self.show_help = !self.show_help;
                        return None;
                    }
                    Key::F(2) => {
                        self.palette_open = false;
                        self.show_help = false;
                        self.work.page = crate::work::Page::Closed;
                        return Some(Msg::OpenConversations);
                    }
                    Key::F(3) | Key::F(4) => {
                        self.palette_open = false;
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
        if self.show_approval_modal {
            self.palette_open = false;
        }
        if self.palette_open && !matches!(event, Event::Quit) {
            if let Event::Paste { text, .. } = event {
                let remaining = 256usize.saturating_sub(self.palette_query.chars().count());
                self.palette_query
                    .extend(text.chars().filter(|ch| !ch.is_control()).take(remaining));
                self.suggestion_selected = 0;
                return None;
            }
            if let Event::Key(key) = event {
                let count = self.command_registry.completions(&self.palette_query).len();
                let page = self.terminal_height.saturating_sub(6).max(1) as usize;
                match key.key {
                    Key::Escape => self.palette_open = false,
                    Key::Up => {
                        self.suggestion_selected = self.suggestion_selected.saturating_sub(1)
                    }
                    Key::Down => {
                        self.suggestion_selected =
                            (self.suggestion_selected + 1).min(count.saturating_sub(1))
                    }
                    Key::PageUp => {
                        self.suggestion_selected = self.suggestion_selected.saturating_sub(page)
                    }
                    Key::PageDown => {
                        self.suggestion_selected =
                            (self.suggestion_selected + page).min(count.saturating_sub(1))
                    }
                    Key::Backspace => {
                        self.palette_query.pop();
                        self.suggestion_selected = 0;
                    }
                    Key::Char(ch) if !key.modifiers.ctrl && !key.modifiers.alt => {
                        if self.palette_query.chars().count() < 256 && !ch.is_control() {
                            self.palette_query.push(ch);
                            self.suggestion_selected = 0;
                        }
                    }
                    Key::Tab | Key::Enter => {
                        if let Some(cmd) = self
                            .command_registry
                            .completions(&self.palette_query)
                            .get(self.suggestion_selected)
                        {
                            let command = format!("/{} ", cmd.name);
                            self.palette_open = false;
                            if key.key == Key::Enter {
                                return Some(Msg::RunPalette(command));
                            }
                            self.input.insert_text(&command);
                            self.refresh_composer();
                        }
                    }
                    _ => {}
                }
            }
            return None;
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
                        if self.approval_stage != 0 {
                            self.approval_stage = 0;
                            self.approval_details = false;
                            self.approval_scroll = 0;
                            return None;
                        }
                        if self.approval_details {
                            self.approval_details = false;
                            return None;
                        }
                        self.approval_scroll = 0;
                        Some(Msg::DismissApproval)
                    }
                    Key::Enter if self.approval_stage == 2 && !self.approval_in_flight => {
                        return Some(Msg::ApprovalRejectFeedback(
                            self.approval_feedback.current_value().to_string(),
                        ));
                    }
                    _ if self.approval_stage == 2 => {
                        let _ = self.approval_feedback.on_key(ke);
                        return None;
                    }
                    Key::Enter
                        if self.approval_stage == 1
                            && !self.approval_in_flight
                            && self.approval_scroll >= self.approval_max_scroll() =>
                    {
                        return Some(Msg::ApprovalSession);
                    }
                    Key::Char('d') if !self.approval_details => {
                        self.approval_details = true;
                        None
                    }
                    Key::Up if !self.approval_details => {
                        self.approval_choice = self.approval_choice.saturating_sub(1);
                        None
                    }
                    Key::Down | Key::Tab if !self.approval_details => {
                        self.approval_choice =
                            (self.approval_choice + 1).min(if self.approval_session_available() {
                                2
                            } else {
                                1
                            });
                        None
                    }
                    Key::Enter if !self.approval_details && !self.approval_in_flight => {
                        match self.approval_choice {
                            0 => Some(Msg::ApprovalDecision(true)),
                            1 if self.approval_session_available() => {
                                self.approval_stage = 1;
                                self.approval_details = true;
                                self.approval_scroll = 0;
                                None
                            }
                            _ if self.approval_supports("reject_feedback")
                                && !self.approval_is_stream =>
                            {
                                self.approval_stage = 2;
                                None
                            }
                            _ => Some(Msg::ApprovalDecision(false)),
                        }
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
                    Key::Char('y') if self.approval_stage == 0 && !self.approval_in_flight => {
                        Some(Msg::ApprovalDecision(true))
                    }
                    Key::Char('n') if self.approval_stage == 0 && !self.approval_in_flight => {
                        Some(Msg::ApprovalDecision(false))
                    }
                    // "Don't ask again" still shows its scope before it is sent.
                    Key::Char('a')
                        if self.approval_stage == 0
                            && !self.approval_in_flight
                            && self.approval_session_available() =>
                    {
                        self.approval_choice = 1;
                        self.approval_stage = 1;
                        self.approval_details = true;
                        self.approval_scroll = 0;
                        None
                    }
                    _ => None,
                },
                Event::Paste { text, .. } if self.approval_stage == 2 => {
                    self.approval_feedback.insert_text(text);
                    None
                }
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
                if ke.focused_id.as_deref() == Some("chat-input") && self.suggestion_rows() > 0 {
                    let last = self.suggestions.len().saturating_sub(1);
                    let step = self.suggestion_rows().max(1) as usize;
                    match ke.key {
                        Key::Up => {
                            self.suggestion_selected = self.suggestion_selected.saturating_sub(1);
                            return None;
                        }
                        Key::Down => {
                            self.suggestion_selected = (self.suggestion_selected + 1).min(last);
                            return None;
                        }
                        Key::PageUp => {
                            self.suggestion_selected =
                                self.suggestion_selected.saturating_sub(step);
                            return None;
                        }
                        Key::PageDown => {
                            self.suggestion_selected = (self.suggestion_selected + step).min(last);
                            return None;
                        }
                        Key::Tab | Key::Enter if !ke.modifiers.shift && !self.newline_prefix => {
                            let name = self.suggestions[self.suggestion_selected].0.clone();
                            self.input.set_value(format!("{name} "));
                            self.refresh_composer();
                            return if ke.key == Key::Enter {
                                Some(Msg::SendMessage)
                            } else {
                                None
                            };
                        }
                        _ => {}
                    }
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
                        if let Some((first, _)) = self.suggestions.get(self.suggestion_selected) {
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
        if self.palette_open && !self.show_approval_modal {
            let commands = self.command_registry.completions(&self.palette_query);
            let height = self.terminal_height.saturating_sub(5).max(1) as usize;
            let start = self
                .suggestion_selected
                .saturating_sub(height.saturating_sub(1));
            let width = usize::from(self.terminal_width.saturating_sub(2).max(1));
            let header = crate::work_views::wrap("Commands · ↑↓ Tab Enter Esc", width)
                .into_iter()
                .next()
                .unwrap_or_default();
            let query = crate::work_views::wrap(&format!("Search: {}", self.palette_query), width)
                .into_iter()
                .last()
                .unwrap_or_default();
            let mut view = Column::new()
                .padding(1.0)
                .child(Text::new(header))
                .child(Text::new(query));
            if commands.is_empty() {
                view = view.child(Text::new("No matching commands."));
            }
            for (index, cmd) in commands.iter().enumerate().skip(start).take(height) {
                let line = format!(
                    "{} /{}  {}",
                    if index == self.suggestion_selected {
                        ">"
                    } else {
                        " "
                    },
                    cmd.name,
                    cmd.description
                );
                let width = usize::from(self.terminal_width.saturating_sub(2).max(1));
                let line = crate::work_views::wrap(&line, width)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                view = view.child(Text::new(line));
            }
            return Stack::new().child(view);
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
                "Ctrl+P: commands. /help: all registered commands.",
                "/reconnect: inspect. /attach ID: read a run.",
            ] {
                help = help.child(Text::new(line));
            }
            return Stack::new().child(help);
        }
        if self.work.page != crate::work::Page::Closed && !self.show_approval_modal {
            return Stack::new().child(crate::work_views::view(self));
        }
        if self.show_approval_modal && self.approval_details && self.approval_stage != 1 {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(self.approval_page_view(req));
            }
        }
        if self.show_approval_modal {
            if let Some(request) = &self.pending_approval {
                let width = self.terminal_width.saturating_sub(4).clamp(1, 80);
                let height = self.approval_panel_height();
                let panel = if self.approval_stage == 1 {
                    self.approval_session_view(request)
                } else {
                    self.approval_menu_view(request)
                };
                return Stack::new()
                    .child(main_content)
                    .overlay(panel, width, height);
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
    fn palette_escape_preserves_draft_and_tab_inserts_without_running() {
        let mut app = PorygonTui::new();
        app.input.set_value("unfinished draft");
        app.handle_event(&composer_key(Key::Char('p'), true));
        assert!(app.palette_open);
        for ch in "mdl".chars() {
            app.handle_event(&composer_key(Key::Char(ch), false));
        }
        assert!(app
            .command_registry
            .completions(&app.palette_query)
            .iter()
            .any(|c| c.name == "model"));
        app.handle_event(&composer_key(Key::Escape, false));
        assert_eq!(app.input.current_value(), "unfinished draft");
        app.handle_event(&composer_key(Key::Char('p'), true));
        for ch in "model".chars() {
            app.handle_event(&composer_key(Key::Char(ch), false));
        }
        assert!(app.handle_event(&composer_key(Key::Tab, false)).is_none());
        assert!(app.input.current_value().contains("unfinished draft"));
        assert!(app.input.current_value().contains("/model "));
        assert!(!app.palette_open);
    }

    #[test]
    fn palette_paste_does_not_change_draft_and_approval_takes_focus() {
        let mut app = PorygonTui::new();
        app.input.set_value("keep draft");
        app.handle_event(&composer_key(Key::Char('p'), true));
        app.handle_event(&Event::Paste {
            text: "model\n".repeat(80),
            focused_id: None,
        });
        assert!(app.palette_query.chars().count() <= 256);
        assert!(!app.palette_query.contains('\n'));
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        assert_eq!(app.input.current_value(), "keep draft");
        app.show_approval_modal = true;
        app.handle_event(&composer_key(Key::Escape, false));
        assert!(!app.palette_open);
    }

    #[test]
    fn palette_enter_dispatches_without_changing_draft() {
        let mut app = PorygonTui::new();
        app.input.set_value("keep draft");
        app.handle_event(&composer_key(Key::Char('p'), true));
        for ch in "model".chars() {
            app.handle_event(&composer_key(Key::Char(ch), false));
        }
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::RunPalette(_))
        ));
        assert_eq!(app.input.current_value(), "keep draft");
    }

    #[test]
    fn slash_selection_moves_and_enter_explicitly_dispatches() {
        let mut app = PorygonTui::new();
        app.terminal_height = 24;
        for ch in "/s".chars() {
            app.handle_event(&composer_key(Key::Char(ch), false));
        }
        assert!(app.suggestions.len() > 1);
        app.handle_event(&composer_key(Key::Down, false));
        let selected = app.suggestions[1].0.clone();
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::SendMessage)
        ));
        assert_eq!(app.input.current_value(), format!("{selected} "));
    }

    #[test]
    fn approval_menu_keeps_transcript_and_draft_and_selects_no() {
        use crate::types::ChatMessage;

        let mut app = PorygonTui::new();
        app.terminal_width = 80;
        app.terminal_height = 24;
        app.input.set_value("unsent draft");
        app.messages.push(ChatMessage {
            role: "user".into(),
            content: "Visible conversation".into(),
            timestamp: "12:00".into(),
        });
        app.pending_approval = Some(ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("v1".into()),
            decision_stale: false,
            id: "one".into(),
            tool_name: "run_argv".into(),
            params: serde_json::json!({"params": {"argv": ["/a/very/long/absolute/path/to/the/installed/demo/tool/with/a/long/name", "--help"]}}),
            risk_score: -1.0,
            reasoning: "review".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        });
        app.show_approval_modal = true;
        app.update_scroll_dimensions();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(screen.contains("Visible conversation"), "{screen}");
        assert!(screen.contains("Permission required"), "{screen}");
        assert!(screen.contains("--help"), "{screen}");
        assert!(screen.contains("> Yes"), "{screen}");
        assert!(screen.contains("  No"), "{screen}");
        let native = ulttui::widgets::render_view_messages(&app.render_view()).unwrap();
        assert!(native.iter().any(|message| matches!(message, ultty_protocol::AppMessage::SetSemantics { semantics, .. } if semantics.modal && semantics.role == ultty_protocol::Role::Dialog)));
        assert!(native.iter().any(|message| matches!(
            message,
            ultty_protocol::AppMessage::SetContent {
                content: ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(_)),
                ..
            }
        )));
        app.handle_event(&composer_key(Key::Down, false));
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::ApprovalDecision(false))
        ));
        assert_eq!(app.input.current_value(), "unsent draft");
        app.handle_event(&composer_key(Key::Char('d'), false));
        assert!(app.approval_details);
        app.handle_event(&composer_key(Key::Escape, false));
        assert!(!app.approval_details);
        assert_eq!(app.input.current_value(), "unsent draft");
    }

    #[test]
    fn approval_menu_shows_host_execution_only_when_daemon_marks_it() {
        let render = |params: serde_json::Value| {
            let mut app = PorygonTui::new();
            app.terminal_width = 80;
            app.terminal_height = 24;
            app.pending_approval = Some(ApprovalRequest {
                scope: Some("tool_call".into()),
                decision_version: Some("v1".into()),
                decision_stale: false,
                id: "one".into(),
                tool_name: "run_argv".into(),
                params,
                risk_score: -1.0,
                reasoning: "review".into(),
                blast_radius: "unknown".into(),
                reversible: None,
            });
            app.show_approval_modal = true;
            app.update_scroll_dimensions();
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
                .unwrap();
            let screen = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            let details = app
                .approval_rows(app.pending_approval.as_ref().unwrap())
                .join("\n");
            (screen, details)
        };
        let argv = serde_json::json!({"argv": ["ulttyctl", "--help"]});
        let (screen, details) = render(serde_json::json!({
            "params": argv,
            "reason": "host_execution_request",
            "execution": "host",
            "justification": "ulttyctl drives your terminal",
            "approval_options": ["once", "reject_feedback"],
        }));
        assert!(
            screen.contains("Runs on your machine (outside the project sandbox)"),
            "{screen}"
        );
        assert!(
            screen.contains("Why: ulttyctl drives your terminal"),
            "{screen}"
        );
        assert!(
            screen.contains("> Yes")
                && screen.contains("No, and tell Porygon what to do differently"),
            "{screen}"
        );
        assert!(!screen.contains("Runs sandboxed"), "{screen}");
        assert!(details.contains("Runs on your machine"), "{details}");
        assert!(
            details.contains("Justification: ulttyctl drives your terminal"),
            "{details}"
        );
        assert!(details.contains("Scope: this invocation only"), "{details}");

        // An older daemon sends no execution field: the menu renders as before.
        let (screen, details) = render(serde_json::json!({
            "params": argv,
            "reason": "missing_policy_binding",
            "approval_options": ["once", "reject_feedback"],
        }));
        assert!(screen.contains("Permission required"), "{screen}");
        assert!(screen.contains("> Yes"), "{screen}");
        assert!(!screen.contains("Runs on your machine"), "{screen}");
        assert!(!screen.contains("Runs sandboxed"), "{screen}");
        assert!(!screen.contains("Why:"), "{screen}");
        assert!(!details.contains("Runs on your machine"), "{details}");

        // A refreshed pending record keeps the stored reason and arguments.
        let (screen, _) = render(serde_json::json!({
            "params": {"argv": ["ulttyctl", "--help"], "host": true, "justification": "drive the terminal"},
            "reason": "host_execution_request",
        }));
        assert!(
            screen.contains("Runs on your machine (outside the project sandbox)"),
            "{screen}"
        );
        assert!(screen.contains("Why: drive the terminal"), "{screen}");
    }

    fn contained_request(options: serde_json::Value) -> ApprovalRequest {
        ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("v1".into()),
            decision_stale: false,
            id: "contained".into(),
            tool_name: "run_argv".into(),
            params: serde_json::json!({
                "params": {"argv": ["pip", "download", "requests"]},
                "reason": "contained_project_work",
                "execution": "contained",
                "approval_options": options,
                "session_scope": {
                    "conversation_id": "conversation-1",
                    "project_root": "/project",
                    "argv": ["pip", "download", "requests"],
                    "execution": "contained",
                },
            }),
            risk_score: -1.0,
            reasoning: "review".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        }
    }

    fn screen_of(app: &mut PorygonTui) -> String {
        app.update_scroll_dimensions();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
    }

    #[test]
    fn contained_approval_offers_the_three_choice_menu_and_says_it_is_sandboxed() {
        let mut app = PorygonTui::new();
        app.terminal_width = 80;
        app.terminal_height = 24;
        app.input.set_value("unsent draft");
        app.pending_approval = Some(contained_request(serde_json::json!([
            "once",
            "reject_feedback",
            "session"
        ])));
        app.show_approval_modal = true;
        let screen = screen_of(&mut app);
        assert!(screen.contains("pip"), "{screen}");
        assert!(
            screen.contains("Runs sandboxed (a disposable copy of the project, no network)"),
            "{screen}"
        );
        assert!(!screen.contains("Runs on your machine"), "{screen}");
        let yes = screen.find("> Yes").expect("Yes is first and selected");
        let always = screen
            .find("Yes, and don't ask again for this command")
            .expect("session choice");
        let no = screen
            .find("No, and tell Porygon what to do differently")
            .expect("reject with instructions");
        assert!(yes < always && always < no, "{screen}");
        let details = app
            .approval_rows(app.pending_approval.as_ref().unwrap())
            .join("\n");
        assert!(details.contains("Runs sandboxed"), "{details}");
        assert!(details.contains("Scope: this invocation only"), "{details}");

        // 1. Yes: a one-time approval.
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::ApprovalDecision(true))
        ));
        // 2. Don't ask again: shows the sandboxed scope, then confirms.
        assert!(app
            .handle_event(&composer_key(Key::Char('a'), false))
            .is_none());
        assert_eq!(app.approval_stage, 1);
        let scope = app
            .approval_rows(app.pending_approval.as_ref().unwrap())
            .join("\n");
        assert!(scope.contains("Runs sandboxed"), "{scope}");
        assert!(scope.contains("conversation-1") && scope.contains("download"));
        app.handle_event(&composer_key(Key::End, false));
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::ApprovalSession)
        ));
        // 3. No: type what to do instead; the draft is untouched.
        app.handle_event(&composer_key(Key::Escape, false));
        app.approval_choice = 0;
        app.handle_event(&composer_key(Key::Down, false));
        app.handle_event(&composer_key(Key::Down, false));
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        assert_eq!(app.approval_stage, 2);
        for key in "use the vendored wheel".chars() {
            app.handle_event(&composer_key(Key::Char(key), false));
        }
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::ApprovalRejectFeedback(text)) if text == "use the vendored wheel"
        ));
        assert_eq!(app.input.current_value(), "unsent draft");
    }

    #[test]
    fn contained_approval_without_session_offers_yes_and_no_only() {
        let mut app = PorygonTui::new();
        app.terminal_width = 80;
        app.terminal_height = 24;
        app.pending_approval = Some(contained_request(serde_json::json!([
            "once",
            "reject_feedback"
        ])));
        app.show_approval_modal = true;
        let screen = screen_of(&mut app);
        assert!(screen.contains("Runs sandboxed"), "{screen}");
        assert!(!screen.contains("don't ask again"), "{screen}");
        assert!(app
            .handle_event(&composer_key(Key::Char('a'), false))
            .is_none());
        assert_eq!(app.approval_stage, 0);
    }

    #[test]
    fn session_confirmation_and_rejection_require_advertised_options() {
        let mut app = PorygonTui::new();
        app.terminal_width = 40;
        app.terminal_height = 12;
        app.pending_approval = Some(ApprovalRequest {
            scope: Some("tool_call".into()),
            decision_version: Some("v1".into()),
            decision_stale: false,
            id: "one".into(),
            tool_name: "run_argv".into(),
            params: serde_json::json!({"params":{"argv":["printf","hello"]}, "approval_options":["once","session","reject_feedback"], "session_scope":{"conversation_id":"conversation-1","project_root":"/project","argv":["printf","hello"]}}),
            risk_score: -1.0,
            reasoning: "review".into(),
            blast_radius: "unknown".into(),
            reversible: None,
        });
        app.show_approval_modal = true;
        app.handle_event(&composer_key(Key::Down, false));
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        assert_eq!(app.approval_stage, 1);
        let rows = app
            .approval_rows(app.pending_approval.as_ref().unwrap())
            .join("\n");
        assert!(
            rows.contains("conversation-1") && rows.contains("/project") && rows.contains("printf")
        );
        app.handle_event(&composer_key(Key::End, false));
        assert!(matches!(
            app.handle_event(&composer_key(Key::Enter, false)),
            Some(Msg::ApprovalSession)
        ));
        app.handle_event(&composer_key(Key::Escape, false));
        app.handle_event(&composer_key(Key::Down, false));
        assert!(app.handle_event(&composer_key(Key::Enter, false)).is_none());
        assert_eq!(app.approval_stage, 2);
        app.handle_event(&composer_key(Key::Char('x'), false));
        assert!(
            matches!(app.handle_event(&composer_key(Key::Enter, false)), Some(Msg::ApprovalRejectFeedback(text)) if text == "x")
        );
        app.pending_approval.as_mut().unwrap().params["session_scope"]["argv"] =
            serde_json::json!(["different"]);
        assert!(!app.approval_session_available());
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
        app.approval_details = true;
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
    fn ansi_palette_navigation_is_visible_at_standard_sizes() {
        for (width, height) in [(80, 24), (160, 48)] {
            let mut app = PorygonTui::new();
            app.terminal_width = width;
            app.terminal_height = height;
            app.input.set_value("draft remains");
            app.handle_event(&composer_key(Key::Char('p'), true));
            for ch in "mdl".chars() {
                app.handle_event(&composer_key(Key::Char(ch), false));
            }
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| app.render_view().render_ansi(frame, frame.area()))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("/model"), "{width}x{height}: {text}");
            app.handle_event(&composer_key(Key::Escape, false));
            assert_eq!(app.input.current_value(), "draft remains");
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
