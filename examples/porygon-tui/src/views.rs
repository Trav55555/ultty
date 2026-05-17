use std::time::Instant;

use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::{parse_unified_diff, DiffKind, RichText, Stack};
use ultty_protocol::{BorderStyle, Color, Dimension};
use unicode_width::UnicodeWidthChar;

use crate::sidebar::sidebar_view;
use crate::types::{ApprovalRequest, FileTreeEntry, ToolStatus};
use crate::{Msg, PorygonTui};

const SIDEBAR_MIN_TERMINAL_WIDTH: u16 = 100;
const SIDEBAR_WIDTH_CELLS: u16 = 34;
const MESSAGE_CHROME_CELLS: u16 = 6;

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

    fn estimated_message_card_height(&self, content: &str) -> f32 {
        let content_lines = wrapped_line_count(content, self.message_content_width_cols());
        1.0 + content_lines as f32 + 3.0
    }

    pub(crate) fn update_scroll_dimensions(&mut self) {
        let estimated_content = (self.messages.len() + self.tool_calls.len()) as f32 * 4.0
            + if self.waiting_response { 6.0 } else { 4.0 };
        let viewport = (self.terminal_height as f32 - 7.0).max(1.0);
        self.chat_scroll.set_content_height(estimated_content);
        self.chat_scroll.set_viewport_height(viewport);
    }

    pub(crate) fn auto_scroll_chat(&mut self) {
        let should_follow = self.chat_scroll.is_near_bottom(2.0);
        self.update_scroll_dimensions();
        if should_follow {
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
        const ESTIMATED_MESSAGE_HEIGHT: f32 = 4.0;
        let mut col = Column::new()
            .gap(0.5)
            .padding(1.0)
            .justify_start()
            .align_content_start()
            .height(Dimension::Percent(100.0))
            .width(Dimension::Percent(100.0));
        let total_messages = self.messages.len();
        let (start_idx, end_idx) = self
            .chat_scroll
            .visible_child_range(total_messages)
            .unwrap_or((0, total_messages));

        if start_idx > 0 {
            col = col.child(Column::new().height(Dimension::Cells(
                start_idx as f32 * ESTIMATED_MESSAGE_HEIGHT,
            )));
        }

        for (i, msg) in self
            .messages
            .iter()
            .enumerate()
            .skip(start_idx)
            .take(end_idx.saturating_sub(start_idx))
        {
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
            let card_height = self.estimated_message_card_height(&msg.content);
            let mut card = Column::new()
                .bg(bg_color)
                .border(BorderStyle::thin())
                .border_color(bdr_color)
                .padding(1.0)
                .gap(0.0)
                .justify_start()
                .align_content_start()
                .height(Dimension::Cells(card_height))
                .flex_shrink(0.0)
                .width(Dimension::Percent(100.0))
                .overflow_y_hidden()
                .child(header);

            let is_streaming_msg =
                self.waiting_response && i + 1 == self.messages.len() && msg.role == "assistant";
            if is_streaming_msg {
                card = card.child(
                    MarkdownView::new(msg.content.clone())
                        .wrap_width_cols(self.message_content_width_cols())
                        .streaming(true),
                );
            } else {
                card = card.child(
                    MarkdownView::new(msg.content.clone())
                        .wrap_width_cols(self.message_content_width_cols()),
                );
            }
            col = col.child(card);
        }

        if end_idx < total_messages {
            col = col.child(Column::new().height(Dimension::Cells(
                (total_messages - end_idx) as f32 * ESTIMATED_MESSAGE_HEIGHT,
            )));
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

        if end_idx >= total_messages {
            col = col.child(Spacer);
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

        let step_text = self.status_step_text();

        let perf_text = self.last_stream_perf.as_ref().map(|perf| {
            let p95 = perf
                .append_p95_ms
                .map(|v| format!("{v}ms"))
                .unwrap_or_else(|| "n/a".to_string());
            let avg = perf
                .append_avg_ms
                .map(|v| format!("{v}ms"))
                .unwrap_or_else(|| "n/a".to_string());
            format!(
                "ttft:{}ms p95:{} avg:{} n:{} tok:{} total:{}ms",
                perf.ttft_ms, p95, avg, perf.append_sample_count, perf.token_count, perf.total_ms
            )
        });

        let mut status_line = RichText::new()
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
            );

        if let Some(perf_text) = perf_text {
            status_line = status_line.span(
                format!(" {} ", perf_text),
                Style {
                    dim: true,
                    ..Style::new()
                },
            );
        }

        if !self.cancel_visual_stop_history_ms.is_empty() {
            let mut sorted = self.cancel_visual_stop_history_ms.clone();
            sorted.sort_unstable();
            let idx = ((sorted.len() - 1) as f64 * 0.95).round() as usize;
            let cancel_p95 = sorted[idx];
            status_line = status_line.span(
                format!(" cancel_p95:{}ms ", cancel_p95),
                Style {
                    dim: true,
                    ..Style::new()
                },
            );
        }

        Row::new().child(status_line.span(
            self.status_key_hint(),
            Style {
                dim: true,
                ..Style::new()
            },
        ))
    }

    pub(crate) fn status_step_text(&self) -> String {
        if self.waiting_response && self.stream_token_count > 0 {
            format!("streaming: {} tokens", self.stream_token_count)
        } else {
            self.status.step.clone()
        }
    }

    pub(crate) fn status_key_hint(&self) -> &'static str {
        if self.waiting_response {
            " Enter:send Esc/Ctrl+C:abort PgUp/Dn:scroll "
        } else {
            " Enter:send PgUp/Dn:scroll Ctrl+C:quit "
        }
    }

    pub(crate) fn should_show_sidebar(&self) -> bool {
        self.terminal_width >= SIDEBAR_MIN_TERMINAL_WIDTH
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
            Event::Quit if self.waiting_response => Some(Msg::AbortStream),
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
        let mut chat_area = ScrollView::new()
            .virtual_scroll(true)
            .item_height(4.0)
            .child(self.messages_view());
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

        if self.should_show_sidebar() {
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

fn wrapped_line_count(text: &str, max_cols: usize) -> usize {
    if max_cols == 0 {
        return text.lines().count().max(1);
    }

    let mut count = 0usize;
    for line in text.split('\n') {
        count += line_wrapped_count(line, max_cols);
    }
    count.max(1)
}

fn line_wrapped_count(line: &str, max_cols: usize) -> usize {
    if line.is_empty() {
        return 1;
    }

    let mut lines = 1usize;
    let mut cols = 0usize;
    for ch in line.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if cols > 0 && cols + width > max_cols {
            lines += 1;
            cols = 0;
        }
        cols += width;
        while cols > max_cols {
            lines += 1;
            cols -= max_cols;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let short = app.estimated_message_card_height("short line");
        let long = app.estimated_message_card_height(
            "- **Project DNA:** AGENTS.md found at /home/travers/projects/tui-terminal/AGENTS.md",
        );

        assert!(long > short, "long={long} short={short}");
    }

    #[test]
    fn message_content_width_reserves_sidebar_when_visible() {
        let mut app = PorygonTui::new();
        app.terminal_width = 99;
        let narrow = app.message_content_width_cols();
        app.terminal_width = 100;
        let with_sidebar = app.message_content_width_cols();

        assert!(
            with_sidebar < narrow,
            "with_sidebar={with_sidebar} narrow={narrow}"
        );
    }
}
