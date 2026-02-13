use std::time::Instant;

use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::{parse_unified_diff, DiffKind, RichText, Stack};
use ultty_protocol::{BorderStyle, Color, Dimension};

use crate::sidebar::sidebar_view;
use crate::types::{ApprovalRequest, FileTreeEntry, ToolStatus};
use crate::{Msg, PorygonTui};

#[derive(Debug, Clone)]
pub(crate) struct SplashScreen {
    boot_log: Vec<String>,
    boot_start: Instant,
}

impl SplashScreen {
    pub(crate) fn new() -> Self {
        Self {
            boot_log: vec![
                "[  OK  ] Started PorygonOS Kernel v1.4.2.".into(),
                "[ INFO ] Initializing LanceDB vector store...".into(),
                "[  OK  ] Pokedex database online".into(),
                "[ INFO ] Building Porygon habitat environment...".into(),
                "[ .... ] Installing shader programs: Vertex/Fragment".into(),
                "[ INFO ] Aligning buffer matrix...".into(),
                "[ INFO ] Optimizing sprite texture atlas...".into(),
                "[ INFO ] Loading project files...".into(),
                "[ INFO ] Initializing tool registry...".into(),
                "[ INFO ] Initializing TUI Subsystem...".into(),
                "[  OK  ] Graphics engine online.".into(),
                "[  OK  ] Input controller online.".into(),
                "[ INFO ] Establishing Uplink to User Terminal...".into(),
                "[  OK  ] Link established.".into(),
                "[ WARN ] HELLO WORLD".into(),
                "...".into(),
                "System Ready.".into(),
            ],
            boot_start: Instant::now(),
        }
    }

    fn visible_lines(&self) -> usize {
        let elapsed_ms = self.boot_start.elapsed().as_millis() as usize;
        (elapsed_ms / 120).min(self.boot_log.len())
    }

    fn boot_complete(&self) -> bool {
        self.visible_lines() >= self.boot_log.len()
    }
}

impl Screen for SplashScreen {
    fn view(&self) -> Column {
        let logo = Column::new()
            .gap(0.0)
            .align_center()
            .child(RichText::new().span(
                "P O R Y G O N",
                Style::new().fg(Color::Rgb(255, 93, 110)).bold(),
            ))
            .child(RichText::new().span(
                "=========  O S  =========",
                Style::new().fg(Color::Rgb(58, 224, 221)).bold(),
            ));

        let visible = self.visible_lines();
        let mut boot_lines = Column::new()
            .gap(0.0)
            .padding(0.5)
            .width(Dimension::Percent(100.0));
        for line in self.boot_log.iter().take(visible) {
            let (prefix, rest, color) = if let Some(rest) = line.strip_prefix("[  OK  ]") {
                ("[  OK  ]", rest, Color::Rgb(166, 227, 161))
            } else if let Some(rest) = line.strip_prefix("[ INFO ]") {
                ("[ INFO ]", rest, Color::Rgb(58, 224, 221))
            } else if let Some(rest) = line.strip_prefix("[ WARN ]") {
                ("[ WARN ]", rest, Color::Rgb(249, 226, 175))
            } else if let Some(rest) = line.strip_prefix("[ .... ]") {
                ("[ .... ]", rest, Color::Rgb(247, 247, 247))
            } else {
                ("", line.as_str(), Color::Rgb(247, 247, 247))
            };

            boot_lines = boot_lines.child(
                RichText::new()
                    .span(prefix, Style::new().fg(color).bold())
                    .span(rest, Style::default())
                    .fill_width(),
            );
        }

        let prompt_text = if self.boot_complete() {
            "Press ENTER to continue"
        } else {
            "BOOTING..."
        };

        let boot_container = Column::new()
            .height(Dimension::Cells(20.0))
            .width(Dimension::Percent(80.0))
            .bg(Color::Rgb(17, 17, 21))
            .child(ScrollView::new().child(boot_lines));

        Column::new()
            .flex_grow(1.0)
            .bg(Color::Rgb(37, 37, 40))
            .border(BorderStyle::double())
            .border_color(Color::Rgb(255, 93, 110))
            .padding(2.0)
            .justify_center()
            .child(logo)
            .child(RichText::new().span(
                "Digital Evolution Interface // PORYGON-OS v1.4.2",
                Style::new().bold().fg(Color::Rgb(58, 224, 221)),
            ))
            .child(Divider::horizontal())
            .child(boot_container)
            .child(Divider::horizontal())
            .child(RichText::new().span(
                prompt_text,
                Style::new().bold().fg(Color::Rgb(58, 224, 221)),
            ))
    }

    fn on_event(&mut self, event: &Event) -> ScreenAction {
        match event {
            Event::Key(ke) if ke.key == Key::Enter && self.boot_complete() => ScreenAction::Dismiss,
            _ => ScreenAction::Continue,
        }
    }
}

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

pub(crate) fn format_clock() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let time_t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_t, &mut tm) };

    let h = tm.tm_hour as u64;
    let m = tm.tm_min as u64;
    let s = tm.tm_sec as u64;
    format!("{h:02}:{m:02}:{s:02}")
}

pub(crate) fn format_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let time_t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_t, &mut tm) };

    let h = tm.tm_hour as u64;
    let m = tm.tm_min as u64;
    format!("{h:02}:{m:02}")
}

impl PorygonTui {
    pub(crate) fn update_scroll_dimensions(&mut self) {
        let estimated_content = self.messages.len() as f32 * 4.0 + 4.0;
        let viewport = (self.terminal_height as f32 - 7.0).max(1.0);
        self.chat_scroll.set_content_height(estimated_content);
        self.chat_scroll.set_viewport_height(viewport);
    }

    pub(crate) fn auto_scroll_chat(&mut self) {
        self.update_scroll_dimensions();
        if self.chat_scroll.is_near_bottom(2.0) {
            self.chat_scroll.scroll_to_bottom();
        }
    }

    fn approval_modal_view(&self, req: &ApprovalRequest) -> Column {
        let risk_pct = format!("{:.0}%", req.risk_score * 100.0);
        let risk_color = if req.risk_score >= 0.7 {
            Color::Rgb(255, 93, 110)
        } else if req.risk_score >= 0.3 {
            Color::Rgb(249, 226, 175)
        } else {
            Color::Rgb(166, 227, 161)
        };
        let reversible_text = if req.reversible { "yes" } else { "no" };

        let params_str = serde_json::to_string_pretty(&req.params)
            .unwrap_or_else(|_| format!("{:?}", req.params));

        Column::new()
            .gap(0.5)
            .width(Dimension::Percent(100.0))
            .child(
                RichText::new()
                    .span("Tool: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                    .span(
                        &req.tool_name,
                        Style::new().fg(Color::Rgb(58, 224, 221)).bold(),
                    )
                    .fill_width(),
            )
            .child(
                RichText::new()
                    .span("Risk: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                    .span(&risk_pct, Style::new().fg(risk_color).bold())
                    .span("  Blast: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                    .span(&req.blast_radius, Style::default())
                    .span("  Reversible: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                    .span(reversible_text, Style::default())
                    .fill_width(),
            )
            .child(
                RichText::new()
                    .span(
                        &req.reasoning,
                        Style {
                            dim: true,
                            ..Style::new()
                        },
                    )
                    .fill_width(),
            )
            .child(MarkdownView::new(format!("```json\n{}\n```", params_str)))
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
        let (conn_icon, conn_color) = if self.connected {
            ("●", Color::Rgb(166, 227, 161))
        } else {
            ("○", Color::Rgb(255, 93, 110))
        };

        let white = Style::new().fg(Color::Rgb(247, 247, 247)).bold();

        let mut header = RichText::new()
            .span(" ⬡ PorygonOS ", white.clone())
            .span(format!(" {} ", conn_icon), Style::new().fg(conn_color));

        if self.waiting_response {
            header = header.span(format!(" {} ", self.hex_gauge()), white.clone());
        }

        Row::new()
            .bg(Color::Rgb(255, 93, 110))
            .height(Dimension::Cells(1.0))
            .flex_shrink(0.0)
            .child(header)
            .child(Spacer)
            .child(RichText::new().span(format!(" {} ", self.clock_text), white))
    }

    fn messages_view(&self) -> Column {
        let mut col = Column::new()
            .gap(0.5)
            .padding(1.0)
            .width(Dimension::Percent(100.0));
        for (i, msg) in self.messages.iter().enumerate() {
            let (prefix, fg_color, bg_color, bdr_color) = match msg.role.as_str() {
                "user" => (
                    "You",
                    Color::Rgb(58, 224, 221),
                    Color::Rgb(42, 42, 48),
                    Color::Rgb(58, 224, 221),
                ),
                "assistant" => (
                    "Porygon",
                    Color::Rgb(255, 93, 110),
                    Color::Rgb(31, 36, 40),
                    Color::Rgb(255, 93, 110),
                ),
                "system" => (
                    "System",
                    Color::Rgb(136, 136, 136),
                    Color::Rgb(35, 35, 40),
                    Color::Rgb(51, 51, 51),
                ),
                _ => (
                    "?",
                    Color::Rgb(247, 247, 247),
                    Color::Rgb(30, 30, 36),
                    Color::Rgb(68, 68, 68),
                ),
            };
            let header = RichText::new()
                .span(
                    format!("[{}] {prefix}:", msg.timestamp),
                    Style::new().fg(fg_color).bold(),
                )
                .fill_width();
            let mut card = Column::new()
                .bg(bg_color)
                .border(BorderStyle::thin())
                .border_color(bdr_color)
                .padding(1.0)
                .gap(0.0)
                .width(Dimension::Percent(100.0))
                .overflow_y_hidden()
                .child(header);

            let is_streaming_msg =
                self.waiting_response && i + 1 == self.messages.len() && msg.role == "assistant";
            card = card.child(MarkdownView::new(msg.content.clone()).streaming(is_streaming_msg));
            col = col.child(card);
        }

        // Tool call cards
        for tool in &self.tool_calls {
            let (status_icon, status_color, status_text) = match tool.status {
                ToolStatus::Pending => ("◷", Color::Rgb(249, 226, 175), "pending"),
                ToolStatus::Approved => ("✓", Color::Rgb(166, 227, 161), "approved"),
                ToolStatus::Rejected => ("✗", Color::Rgb(255, 93, 110), "rejected"),
                ToolStatus::Running => ("⟳", Color::Rgb(58, 224, 221), "running"),
                ToolStatus::Completed => ("✓", Color::Rgb(166, 227, 161), "completed"),
                ToolStatus::Failed => ("✗", Color::Rgb(255, 93, 110), "failed"),
            };

            let header = RichText::new()
                .span("🔧 ", Style::default())
                .span(&tool.name, Style::new().fg(Color::Rgb(58, 224, 221)).bold())
                .span(
                    format!(" [{status_icon} {status_text}]"),
                    Style::new().fg(status_color),
                )
                .fill_width();

            let mut body_col = Column::new().gap(0.0);
            if !tool.args.is_empty() {
                body_col =
                    body_col.child(MarkdownView::new(format!("```json\n{}\n```", tool.args)));
            }
            if let Some(ref result) = tool.result {
                let formatted_result = format_tool_result_with_diff(result);
                if result.contains("@@") && (result.contains("---") || result.contains("+++")) {
                    let mut diff_col = Column::new().gap(0.0).child(
                        RichText::new()
                            .span("Result:", Style::new().bold())
                            .fill_width(),
                    );
                    for line in parse_unified_diff(result) {
                        let line_style = match line.kind {
                            DiffKind::Added => Style::new().fg(Color::Rgb(166, 227, 161)),
                            DiffKind::Removed => Style::new().fg(Color::Rgb(255, 93, 110)),
                            DiffKind::Header => Style::new().fg(Color::Rgb(58, 224, 221)).bold(),
                            DiffKind::Context => Style::default(),
                        };
                        diff_col = diff_col
                            .child(RichText::new().span(line.content, line_style).fill_width());
                    }
                    body_col = body_col.child(diff_col);
                } else {
                    body_col = body_col.child(
                        RichText::new()
                            .span("Result: ", Style::new().bold())
                            .span(formatted_result, Style::default())
                            .fill_width(),
                    );
                }
            }

            let tool_card = Column::new()
                .bg(Color::Rgb(35, 35, 42))
                .border(BorderStyle::thin())
                .border_color(status_color)
                .padding(0.5)
                .width(Dimension::Percent(100.0))
                .child(header);

            // Show args/result expanded for non-completed items, collapsed otherwise
            if !tool.args.is_empty() || tool.result.is_some() {
                col = col.child(
                    Column::new()
                        .width(Dimension::Percent(100.0))
                        .child(tool_card)
                        .child(body_col),
                );
            } else {
                col = col.child(tool_card);
            }
        }

        if self.waiting_response {
            col = col.child(
                Row::new()
                    .gap(1.0)
                    .child(Spinner::new().frame(self.spinner_frame))
                    .child(Text::new("Thinking...")),
            );
        }

        col
    }

    fn input_view(&self) -> Column {
        let mut col = Column::new()
            .gap(0.0)
            .child(
                RichText::new().span(
                    " P-Chat ",
                    Style::new()
                        .fg(Color::Rgb(30, 30, 36))
                        .bg(Color::Rgb(58, 224, 221))
                        .bold(),
                ),
            )
            .child(self.input.clone());

        if !self.suggestions.is_empty() {
            let mut suggestions_col = Column::new()
                .bg(Color::Rgb(45, 45, 50))
                .padding(0.5)
                .gap(0.0);
            for (name, desc) in &self.suggestions {
                suggestions_col = suggestions_col.child(
                    RichText::new()
                        .span(name, Style::new().fg(Color::Rgb(58, 224, 221)))
                        .span(
                            format!("  — {}", desc),
                            Style {
                                dim: true,
                                ..Style::new()
                            },
                        )
                        .fill_width(),
                );
            }
            col = col.child(suggestions_col);
        }

        col.child(RichText::new().span(
            format!("  {}", self.daemon_url),
            Style::new().fg(Color::Rgb(136, 136, 136)),
        ))
    }

    fn status_bar_view(&self) -> Row {
        let (pressure_icon, pressure_color) = match self.status.pressure.as_str() {
            "normal" => ("●", Color::Rgb(166, 227, 161)),
            "elevated" => ("▲", Color::Rgb(249, 226, 175)),
            "critical" => ("◆", Color::Rgb(255, 93, 110)),
            _ => ("?", Color::Rgb(247, 247, 247)),
        };

        let step_text = if self.waiting_response && self.stream_token_count > 0 {
            format!("streaming: {} tokens", self.stream_token_count)
        } else {
            self.status.step.clone()
        };

        Row::new().child(
            RichText::new()
                .span(
                    format!(" {} ", pressure_icon),
                    Style::new().fg(pressure_color).bold(),
                )
                .span(
                    format!(" {} ", step_text),
                    Style {
                        dim: true,
                        ..Style::new()
                    },
                )
                .span(
                    " Enter:send Esc:abort PgUp/Dn:scroll Ctrl+C:quit ",
                    Style {
                        dim: true,
                        ..Style::new()
                    },
                ),
        )
    }

    pub(crate) fn handle_event(&mut self, event: &Event) -> Option<Msg> {
        if self.show_approval_modal {
            return match event {
                Event::Key(ke) => match ke.key {
                    Key::Char('y') | Key::Enter => Some(Msg::ApprovalDecision(true)),
                    Key::Char('n') | Key::Escape => Some(Msg::ApprovalDecision(false)),
                    _ => None,
                },
                _ => None,
            };
        }

        match event {
            Event::Quit => Some(Msg::ExitRequested),
            Event::Resize { width, height } => {
                self.terminal_width = *width;
                self.terminal_height = *height;
                None
            }
            Event::Submit { id } if id == "chat-input" => Some(Msg::SendMessage),
            Event::Change { id, value } if id == "chat-input" => {
                self.input.set_value(value.clone());
                if value.starts_with('/') {
                    self.suggestions = self
                        .command_registry
                        .completions(value)
                        .iter()
                        .take(5)
                        .map(|cmd| (format!("/{}", cmd.name), cmd.description.to_string()))
                        .collect();
                } else {
                    self.suggestions.clear();
                }
                None
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    ulttui::event::MouseEventKind::ScrollUp => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_by(-3.0);
                    }
                    ulttui::event::MouseEventKind::ScrollDown => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_by(3.0);
                    }
                    _ => {}
                }
                None
            }
            Event::Key(ke) => {
                match ke.key {
                    Key::Tab if !self.suggestions.is_empty() => {
                        if let Some((first, _)) = self.suggestions.first() {
                            let completed = format!("{} ", first);
                            self.input.set_value(completed);
                            self.suggestions.clear();
                        }
                        return None;
                    }
                    Key::PageUp => {
                        self.update_scroll_dimensions();
                        let vh = self.chat_scroll.viewport_height();
                        self.chat_scroll.scroll_by(-vh);
                        return None;
                    }
                    Key::PageDown => {
                        self.update_scroll_dimensions();
                        let vh = self.chat_scroll.viewport_height();
                        self.chat_scroll.scroll_by(vh);
                        return None;
                    }
                    Key::Escape if self.waiting_response => {
                        return Some(Msg::AbortStream);
                    }
                    _ => {}
                }
                if ke.focused_id.as_deref() == Some("chat-input") {
                    if ke.key == Key::Enter {
                        return Some(Msg::SendMessage);
                    }
                    let _ = self.input.on_key(ke);
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn render_view(&self) -> Stack {
        let bottom_bar = Column::new()
            .flex_shrink(0.0)
            .child(Divider::horizontal())
            .child(self.input_view())
            .child(Divider::horizontal())
            .child(self.status_bar_view());
        let mut chat_area = ScrollView::new().child(self.messages_view());
        // Transfer scroll state from persistent field
        let (_, scroll_y) = self.chat_scroll.scroll_position();
        if scroll_y > 0.0 {
            let estimated_content = self.messages.len() as f32 * 4.0 + 4.0;
            let viewport = (self.terminal_height as f32 - 7.0).max(1.0);
            chat_area.set_content_height(estimated_content);
            chat_area.set_viewport_height(viewport);
            chat_area.scroll_to(scroll_y);
        }

        let center_content = Column::new()
            .overflow_y_hidden()
            .child(chat_area)
            .child(bottom_bar);

        let mut dock = DockLayout::new().dock_top(self.header_view().flex_shrink(0.0));

        if self.terminal_width >= 80 {
            dock = dock.dock_right(sidebar_view(self));
        }

        let main_content = dock.center(center_content);

        if self.show_approval_modal {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(main_content).child(
                    Modal::new()
                        .title("Tool Approval Required")
                        .body_view(self.approval_modal_view(req))
                        .buttons(vec!["Approve (y/Enter)", "Reject (n/Esc)"])
                        .visible(true),
                );
            }
        }

        Stack::new()
            .child(main_content)
            .child(Modal::new().visible(false))
    }
}
