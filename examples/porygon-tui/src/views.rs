use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::{parse_unified_diff, DiffKind, RichText, Stack};

use crate::sidebar::sidebar_view;
use crate::types::{ApprovalRequest, FileTreeEntry};
use crate::{Msg, PorygonTui};

const SIDEBAR_MIN_TERMINAL_WIDTH: u16 = 100;
const SIDEBAR_WIDTH_CELLS: u16 = 34;
const MESSAGE_CHROME_CELLS: u16 = 4;

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

    fn approval_modal_view(&self, req: &ApprovalRequest) -> Column {
        let risk_pct = if req.risk_score < 0.0 {
            "unknown".into()
        } else {
            format!("{:.0}%", req.risk_score * 100.0)
        };
        let risk_color = if req.risk_score < 0.0 {
            Color::Rgb(136, 136, 136)
        } else if req.risk_score >= 0.7 {
            Color::Rgb(255, 93, 110)
        } else if req.risk_score >= 0.3 {
            Color::Rgb(249, 226, 175)
        } else {
            Color::Rgb(166, 227, 161)
        };
        let reversible_text = match req.reversible {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };

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
        let connection = if self.connected {
            "connected"
        } else {
            "disconnected"
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
            .child(RichText::new().span("  F1 help ", Style::default()))
    }

    fn messages_view(&self) -> Column {
        let mut transcript = self.transcript.borrow_mut();
        transcript.sync(
            self.message_content_width_cols(),
            &self.messages,
            &self.tool_calls,
        );
        transcript.view(
            self.chat_scroll.scroll_position().1.max(0.0) as usize,
            self.terminal_height
                .saturating_sub(1 + self.bottom_height() + 2)
                .max(1) as usize,
            self.waiting_response,
        )
    }

    pub(crate) fn composer_hint(&self) -> &'static str {
        if self.newline_prefix {
            "Enter: insert newline | Esc: cancel chord"
        } else if self.waiting_response || self.stopping {
            "Working | draft kept here | Ctrl+C: stop"
        } else if self.pending_approval.is_some() {
            "Decision pending | draft kept here | /approvals"
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
        if self.show_help && !matches!(event, Event::Quit) {
            if matches!(event, Event::Key(ke) if matches!(ke.key, Key::Escape | Key::F(1))) {
                self.show_help = false;
            }
            return None;
        }
        if self.show_approval_modal {
            return match event {
                Event::Quit => Some(Msg::AbortStream),
                Event::Key(ke) => match ke.key {
                    Key::Escape => Some(Msg::DismissApproval),
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
                None
            }
            Event::Key(ke) => {
                if ke.key == Key::F(1) {
                    self.show_help = true;
                    return None;
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
                        return None;
                    }
                    Key::End if ke.modifiers.ctrl => {
                        self.update_scroll_dimensions();
                        self.chat_scroll.scroll_to_bottom();
                        self.follow_output = true;
                        self.unread_output = false;
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
        if self.show_help {
            let mut help = Column::new()
                .width(Dimension::Percent(100.0))
                .height(Dimension::Percent(100.0))
                .padding(1.0);
            for line in [
                "Keys and commands",
                "Esc or F1: back to your draft",
                "",
                "Enter: send when ready; keep draft while busy.",
                "Ctrl+X then Enter: newline.",
                "Shift+Enter: newline when terminal supports it.",
                "Paste: insert text without sending.",
                "Ctrl+C: stop active work.",
                "Ctrl+C when idle: quit; confirm to discard draft.",
                "PgUp/PgDn: history. Ctrl+End: latest output.",
                "Type / for commands. Ctrl+Space: complete.",
                "/reconnect: inspect. /attach ID: read a run.",
            ] {
                help = help.child(Text::new(line));
            }
            return Stack::new().child(help);
        }
        if self.show_approval_modal {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(main_content).child(
                    Modal::new()
                        .title("Tool Approval Required")
                        .body_view(self.approval_modal_view(req))
                        .buttons(if self.approval_in_flight {
                            vec!["Decision unconfirmed", "R:refresh Esc:close"]
                        } else {
                            vec!["Approve once (y)", "Deny (n)", "Esc:close"]
                        })
                        .visible(true),
                );
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
                "back to your draft",
                "Ctrl+X then Enter: newline.",
                "confirm to discard draft.",
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
