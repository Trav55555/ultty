use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::toast::Toast;
use ulttui::theme::{
    ButtonTheme, ControlTheme, DesignTokens, InputTheme, SelectionTheme, Theme,
};
use ulttui::widgets::{RichText, Stack};
use ultty_protocol::{BorderEdges, BorderStyle, BorderType, Color, Dimension};

fn porygon_theme() -> Theme {
    let primary = Color::Rgb(255, 93, 110); // Coral
    let secondary = Color::Rgb(58, 224, 221); // Cyan
    let background = Color::Rgb(30, 30, 36);
    let surface_0 = Color::Rgb(30, 30, 36);
    let surface_1 = Color::Rgb(37, 37, 40);
    let surface_2 = Color::Rgb(45, 45, 50);
    let foreground = Color::Rgb(247, 247, 247);
    let muted = Color::Rgb(136, 136, 136);
    let error = Color::Rgb(255, 93, 110); // Coral
    let success = Color::Rgb(166, 227, 161);
    let warning = Color::Rgb(249, 226, 175);
    let border_subtle = Color::Rgb(68, 68, 68);
    let border_strong = Color::Rgb(58, 224, 221); // Cyan
    let text_accent = Color::Rgb(58, 224, 221); // Cyan

    Theme {
        primary,
        background,
        foreground,
        muted,
        error,
        success,
        warning,
        button: ButtonTheme {
            fg: foreground,
            bg: surface_1,
            focused_fg: foreground,
            focused_bg: primary,
        },
        input: InputTheme {
            fg: foreground,
            bg: surface_1,
            placeholder_fg: muted,
            cursor_fg: primary,
            focused_border: primary,
        },
        control: ControlTheme {
            fg: foreground,
            active_fg: success,
            indicator: primary,
        },
        selection: SelectionTheme {
            fg: background,
            bg: secondary,
        },
        tokens: DesignTokens {
            space_xs: 0.5,
            space_sm: 1.0,
            space_md: 2.0,
            space_lg: 3.0,
            space_xl: 4.0,
            surface_0,
            surface_1,
            surface_2,
            border_subtle,
            border_strong,
            text_title: foreground,
            text_body: foreground,
            text_muted: muted,
            text_accent,
        },
    }
}

#[derive(Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
    timestamp: String,
}

#[derive(Debug, Clone)]
struct AgentStatus {
    cost_usd: f64,
    budget_usd: f64,
    pressure: String,
    model: String,
    context_quality: String,
    step: String,
    agent_name: String,
    cpu_usage: f64,
    ram_usage: f64,
}

#[derive(Debug, Clone)]
struct WeatherData {
    temp_c: i32,
    description: String,
    emoji: String,
    city: String,
    forecast: Vec<(String, i32, i32)>,
    hourly_temps: Vec<f64>,
}

#[derive(Debug, Clone)]
struct SplashScreen {
    boot_log: Vec<String>,
    boot_start: Instant,
}

impl SplashScreen {
    fn new() -> Self {
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
            .child(
                RichText::new().span(
                    "Digital Evolution Interface // PORYGON-OS v1.4.2",
                    Style::new().bold().fg(Color::Rgb(58, 224, 221)),
                ),
            )
            .child(Divider::horizontal())
            .child(boot_container)
            .child(Divider::horizontal())
            .child(
                RichText::new().span(
                    prompt_text,
                    Style::new().bold().fg(Color::Rgb(58, 224, 221)),
                ),
            )
    }

    fn on_event(&mut self, event: &Event) -> ScreenAction {
        match event {
            Event::Key(ke) if ke.key == Key::Enter && self.boot_complete() => {
                ScreenAction::Dismiss
            }
            _ => ScreenAction::Continue,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ApprovalRequest {
    id: String,
    tool_name: String,
    params: serde_json::Value,
    risk_score: f64,
    reasoning: String,
    blast_radius: String,
    reversible: bool,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self {
            cost_usd: 0.0,
            budget_usd: 5.0,
            pressure: "normal".into(),
            model: "claude-sonnet-4".into(),
            context_quality: "peak".into(),
            step: "idle".into(),
            agent_name: "agent".into(),
            cpu_usage: 0.0,
            ram_usage: 0.0,
        }
    }
}

struct PorygonTui {
    messages: Vec<ChatMessage>,
    input: TextInput,
    chat_scroll: ScrollView,
    status: AgentStatus,
    daemon_url: String,
    client: reqwest::Client,
    connected: bool,
    error: Option<String>,
    spinner_frame: usize,
    waiting_response: bool,
    started: bool,
    pending_approval: Option<ApprovalRequest>,
    show_approval_modal: bool,
    weather: Option<WeatherData>,
    clock_text: String,
    terminal_width: u16,
    terminal_height: u16,
    started_connection_msg: bool,
}

impl PorygonTui {
    fn new() -> Self {
        let daemon_url = std::env::var("PORYGON_URL")
            .unwrap_or_else(|_| "http://localhost:18789".to_string());

        Self {
            messages: vec![ChatMessage {
                role: "system".into(),
                content: {
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "unknown".into());
                    let dna = if std::path::Path::new("AGENTS.md").exists() {
                        format!("AGENTS.md found at {cwd}/AGENTS.md")
                    } else {
                        "No AGENTS.md detected".to_string()
                    };
                    format!(
                        "Welcome to Porygon OS v1.4.2\n\n\
                        - **CWD:** `{cwd}`\n\
                        - **Project DNA:** {dna}\n\
                        - **Mode:** Standalone TUI\n\n\
                        Type a message below to get started."
                    )
                },
                timestamp: format_timestamp(),
            }],
            input: TextInput::new()
                .id("chat-input")
                .height(3)
                .placeholder("Type a message...")
                .vim(),
            chat_scroll: ScrollView::new(),
            status: AgentStatus::default(),
            daemon_url,
            client: reqwest::Client::new(),
            connected: false,
            error: None,
            spinner_frame: 0,
            waiting_response: false,
            started: false,
            pending_approval: None,
            show_approval_modal: false,
            weather: None,
            clock_text: format_clock(),
            terminal_width: 200,
            terminal_height: 50,
            started_connection_msg: false,
        }
    }
}

#[derive(Debug)]
enum Msg {
    Init,
    SendMessage,
    PollConnection,
    PollStatus,
    DaemonResponse(String),
    DaemonError(String),
    ConnectionChecked(bool),
    StatusUpdated(AgentStatus),
    SpinnerTick,
    ClockTick,
    PollWeather,
    WeatherFetched(WeatherData),
    WeatherFailed,
    PollApprovals,
    ApprovalReceived(ApprovalRequest),
    ApprovalDecision(bool),
    ApprovalSubmitted,
    NoApprovalsPending,
}

impl Application for PorygonTui {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Init => {
                if self.started {
                    Cmd::none()
                } else {
                    self.started = true;
                    Cmd::batch(vec![
                        Cmd::PushScreen(Box::new(SplashScreen::new())),
                        Cmd::message(Msg::PollWeather),
                    ])
                }
            }
            Msg::SendMessage => {
                if self.input.current_value().is_empty() || self.waiting_response {
                    return Cmd::none();
                }

                let user_text = self.input.submit_value();
                self.messages.push(ChatMessage {
                    role: "user".into(),
                    content: user_text.clone(),
                    timestamp: format_timestamp(),
                });
                // Always jump to bottom when user sends a message
                self.update_scroll_dimensions();
                self.chat_scroll.scroll_to_bottom();

                if !self.connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "No daemon connected. Messages will be sent once a daemon is available.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    return Cmd::none();
                }

                self.waiting_response = true;
                self.error = None;
                self.status.step = "awaiting daemon response".into();

                let url = format!("{}/api/ask", self.daemon_url);
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match send_to_daemon(&client, &url, &user_text).await {
                        Ok(response) => Msg::DaemonResponse(response),
                        Err(err) => Msg::DaemonError(err),
                    }
                })
            }
            Msg::PollConnection => {
                let url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move { Msg::ConnectionChecked(check_connection(&client, &url).await) })
            }
            Msg::PollStatus => {
                let url = self.daemon_url.clone();
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_status(&client, &url).await {
                        Ok(status) => Msg::StatusUpdated(status),
                        Err(_) => {
                            let (cpu, ram) = get_system_stats();
                            Msg::StatusUpdated(AgentStatus { cpu_usage: cpu, ram_usage: ram, ..AgentStatus::default() })
                        }
                    }
                })
            }
            Msg::PollApprovals => {
                let url = format!("{}/api/approvals/pending", self.daemon_url);
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_pending_approval(&client, &url).await {
                        Ok(Some(request)) => Msg::ApprovalReceived(request),
                        Ok(None) => Msg::NoApprovalsPending,
                        Err(_) => {
                            Msg::NoApprovalsPending
                        }
                    }
                })
            }
            Msg::DaemonResponse(response) => {
                self.waiting_response = false;
                self.status.step = "idle".into();
                self.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: response,
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::none()
            }
            Msg::DaemonError(err) => {
                self.waiting_response = false;
                self.status.step = "idle".into();
                self.error = Some(err.clone());
                self.messages.push(ChatMessage {
                    role: "system".into(),
                    content: format!("Error: {err}"),
                    timestamp: format_timestamp(),
                });
                self.auto_scroll_chat();
                Cmd::ShowToast(Toast::error(format!("Request failed: {err}")))
            }
            Msg::ConnectionChecked(ok) => {
                let was_connected = self.connected;
                self.connected = ok;
                if !ok {
                    self.error = Some("Cannot reach daemon".into());
                }
                if ok && !was_connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: format!("Daemon connected at `{}`.", self.daemon_url),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    Cmd::ShowToast(Toast::success("Daemon connected"))
                } else if !ok && was_connected {
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: "Daemon disconnected.".into(),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    Cmd::ShowToast(Toast::warning("Daemon disconnected"))
                } else if !ok && !was_connected && !self.started_connection_msg {
                    self.started_connection_msg = true;
                    self.messages.push(ChatMessage {
                        role: "system".into(),
                        content: format!(
                            "Running in **standalone mode** (no daemon at `{}`).\n\n\
                            To connect a daemon:\n\n\
                            ```\n\
                            porygon serve\n\
                            PORYGON_URL=http://host:port porygon tui\n\
                            ```\n\n\
                            The sidebar will update when a daemon connects.",
                            self.daemon_url
                        ),
                        timestamp: format_timestamp(),
                    });
                    self.auto_scroll_chat();
                    Cmd::none()
                } else {
                    Cmd::none()
                }
            }
            Msg::StatusUpdated(status) => {
                self.status.cost_usd = status.cost_usd;
                self.status.budget_usd = status.budget_usd;
                self.status.pressure = status.pressure;
                self.status.model = status.model;
                self.status.context_quality = status.context_quality;
                self.status.agent_name = status.agent_name;
                self.status.cpu_usage = status.cpu_usage;
                self.status.ram_usage = status.ram_usage;
                if !self.waiting_response {
                    self.status.step = status.step;
                }
                Cmd::none()
            }
            Msg::SpinnerTick => {
                self.spinner_frame = (self.spinner_frame + 1) % 10;
                Cmd::none()
            }
            Msg::ClockTick => {
                self.clock_text = format_clock();
                let (cpu, ram) = get_system_stats();
                self.status.cpu_usage = cpu;
                self.status.ram_usage = ram;
                Cmd::none()
            }
            Msg::PollWeather => {
                let client = self.client.clone();
                Cmd::spawn(async move {
                    match fetch_weather(&client).await {
                        Ok(data) => Msg::WeatherFetched(data),
                        Err(_) => Msg::WeatherFailed,
                    }
                })
            }
            Msg::WeatherFetched(data) => {
                self.weather = Some(data);
                Cmd::none()
            }
            Msg::WeatherFailed => Cmd::none(),
            Msg::ApprovalReceived(request) => {
                self.pending_approval = Some(request);
                self.show_approval_modal = true;
                Cmd::none()
            }
            Msg::ApprovalDecision(approved) => {
                self.show_approval_modal = false;
                if let Some(request) = self.pending_approval.take() {
                    let url = format!("{}/api/approvals/{}/decide", self.daemon_url, request.id);
                    let client = self.client.clone();
                    Cmd::spawn(async move {
                        let _ = submit_approval(&client, &url, approved).await;
                        Msg::ApprovalSubmitted
                    })
                } else {
                    Cmd::none()
                }
            }
            Msg::ApprovalSubmitted => Cmd::none(),
            Msg::NoApprovalsPending => Cmd::none(),
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        let mut subs = vec![
            Sub::every(Duration::from_secs(3), || Msg::PollConnection),
            Sub::every(Duration::from_secs(2), || Msg::PollStatus),
            Sub::every(Duration::from_millis(500), || Msg::PollApprovals),
            Sub::every(Duration::from_secs(1), || Msg::ClockTick),
            Sub::every(Duration::from_secs(1800), || Msg::PollWeather),
        ];
        if !self.started {
            subs.push(Sub::every(Duration::from_millis(1), || Msg::Init));
        }
        if self.waiting_response {
            subs.push(Sub::every(Duration::from_millis(80), || Msg::SpinnerTick));
        }
        Sub::batch(subs)
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
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
            Event::Resize { width, height } => {
                self.terminal_width = *width;
                self.terminal_height = *height;
                None
            }
            Event::Submit { id } if id == "chat-input" => Some(Msg::SendMessage),
            Event::Change { id, value } if id == "chat-input" => {
                self.input.set_value(value.clone());
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

    fn view(&self) -> impl View {
        let bottom_bar = Column::new()
            .flex_shrink(0.0)
            .child(Divider::horizontal())
            .child(self.input_view())
            .child(Divider::horizontal())
            .child(self.status_bar_view());
        let mut chat_area = ScrollView::new()
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

        let mut dock = DockLayout::new()
            .dock_top(self.header_view().flex_shrink(0.0));

        if self.terminal_width >= 80 {
            dock = dock.dock_right(self.sidebar_view());
        }

        let main_content = dock.center(center_content);

        if self.show_approval_modal {
            if let Some(req) = &self.pending_approval {
                return Stack::new().child(main_content).child(
                    Modal::new()
                        .title("Tool Approval Required")
                        .body(self.approval_modal_body(req))
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

impl PorygonTui {
    fn update_scroll_dimensions(&mut self) {
        let estimated_content = self.messages.len() as f32 * 4.0 + 4.0;
        let viewport = (self.terminal_height as f32 - 7.0).max(1.0);
        self.chat_scroll.set_content_height(estimated_content);
        self.chat_scroll.set_viewport_height(viewport);
    }

    fn auto_scroll_chat(&mut self) {
        self.update_scroll_dimensions();
        if self.chat_scroll.is_near_bottom(2.0) {
            self.chat_scroll.scroll_to_bottom();
        }
    }

    fn approval_modal_body(&self, req: &ApprovalRequest) -> String {
        let params_display = serde_json::to_string_pretty(&req.params)
            .unwrap_or_else(|_| format!("{:?}", req.params));
        let risk = format!("{:.0}%", req.risk_score * 100.0);
        let reversible = if req.reversible { "yes" } else { "no" };

        format!(
            "Tool: {}\nRisk: {}\nBlast radius: {}\nReversible: {}\n\nReasoning:\n{}\n\nParameters:\n{}",
            req.tool_name, risk, req.blast_radius, reversible, req.reasoning, params_display
        )
    }

    fn hex_gauge(&self) -> String {
        let count = 5usize;
        let idx = self.spinner_frame % (count * 2);
        let filled = if idx < count { idx + 1 } else { count * 2 - idx - 1 };
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
            header = header.span(
                format!(" {} ", self.hex_gauge()),
                white.clone(),
            );
        }

        Row::new()
            .bg(Color::Rgb(255, 93, 110))
            .height(Dimension::Cells(1.0))
            .flex_shrink(0.0)
            .child(header)
            .child(Spacer)
            .child(RichText::new().span(
                format!(" {} ", self.clock_text),
                white,
            ))
    }

    fn weather_view(&self) -> Column {
        let coral = Color::Rgb(255, 93, 110);
        let cyan = Color::Rgb(58, 224, 221);

        let mut panel = Column::new()
            .bg(Color::Rgb(26, 26, 29))
            .border(BorderStyle::double())
            .border_color(coral)
            .padding(1.0)
            .gap(0.0);

        if let Some(w) = &self.weather {
            let city_str = if w.city.is_empty() {
                String::new()
            } else {
                format!(" ({})", w.city)
            };
            // Centered title
            panel = panel.child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(
                        RichText::new()
                            .span("WEATHER", Style::new().fg(cyan).bold()),
                    ),
            );
            panel = panel.child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(
                        RichText::new()
                            .span(
                                format!("{} {}°C {}", w.emoji, w.temp_c, w.description),
                                Style::default(),
                            )
                            .span(city_str, Style { dim: true, ..Style::new() }),
                    ),
            );
            if let Some(today) = w.forecast.first() {
                panel = panel.child(
                    Column::new()
                        .align_center()
                        .width(Dimension::Percent(100.0))
                        .child(
                            RichText::new()
                                .span(
                                    format!("H:{}° ", today.1),
                                    Style::new().fg(coral),
                                )
                                .span(
                                    format!("L:{}°", today.2),
                                    Style::new().fg(cyan),
                                ),
                        ),
                );
            }

            // Braille dot plot chart
            if !w.hourly_temps.is_empty() {
                let n = w.hourly_temps.len();
                let chart_width = 21usize; // 7 days × 3 cols each
                let sample_count = chart_width * 2; // 42 points for 21 braille chars

                // Downsample with cosine interpolation
                let mut samples = Vec::with_capacity(sample_count);
                for i in 0..sample_count {
                    let t = i as f64 / (sample_count - 1).max(1) as f64
                        * (n - 1).max(0) as f64;
                    let idx = t.floor() as usize;
                    let frac = t - idx as f64;
                    let a = w.hourly_temps[idx.min(n - 1)];
                    let b = w.hourly_temps[(idx + 1).min(n - 1)];
                    let mu = (1.0 - (frac * std::f64::consts::PI).cos()) / 2.0;
                    samples.push(a * (1.0 - mu) + b * mu);
                }

                let raw_min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
                let raw_max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let pad = ((raw_max - raw_min) * 0.1).max(2.0); // 10% padding, min 2°
                let data_min = (raw_min - pad).floor();
                let data_max = (raw_max + pad).ceil();
                let data_range = (data_max - data_min).max(1.0);

                let chart_rows = 3usize;
                let y_levels = chart_rows * 4; // 12 vertical positions

                let mut grid = vec![vec![0u8; chart_width]; chart_rows];
                let mut char_temps: Vec<Vec<Vec<f64>>> =
                    vec![vec![Vec::new(); chart_width]; chart_rows];

                // Plot data points
                for (i, &temp) in samples.iter().enumerate() {
                    let norm =
                        ((temp - data_min) / data_range).clamp(0.0, 1.0);
                    let y = ((1.0 - norm) * (y_levels - 1) as f64).round()
                        as usize;
                    let y = y.min(y_levels - 1);

                    let text_row = y / 4;
                    let y_in_char = y % 4;
                    let char_col = i / 2;
                    let x_in_char = i % 2;

                    if char_col >= chart_width {
                        continue;
                    }

                    let bit = match (x_in_char, y_in_char) {
                        (0, 0) => 0,
                        (0, 1) => 1,
                        (0, 2) => 2,
                        (0, 3) => 6,
                        (1, 0) => 3,
                        (1, 1) => 4,
                        (1, 2) => 5,
                        (1, 3) => 7,
                        _ => unreachable!(),
                    };

                    grid[text_row][char_col] |= 1 << bit;
                    char_temps[text_row][char_col].push(temp);
                }

                // Add vertical separator lines at even day boundaries
                let num_days = (n + 23) / 24; // typically 7
                let cols_per_day = chart_width / num_days.max(1);
                let mut boundary_cols = Vec::new();
                for day in 1..num_days {
                    let char_col = day * cols_per_day;
                    if char_col > 0 && char_col < chart_width {
                        boundary_cols.push(char_col);
                        // Add left-column dotted line (bits 0,1,2,6)
                        for row in 0..chart_rows {
                            grid[row][char_col] |= 0b0100_0111;
                        }
                    }
                }

                // Track which cols are boundaries for coloring
                let boundary_set: std::collections::HashSet<usize> =
                    boundary_cols.iter().cloned().collect();

                let dim_line = Color::Rgb(68, 68, 68);

                // Add 0°C horizontal line if it falls within data range
                if data_min < 0.0 && data_max > 0.0 {
                    let zero_norm =
                        ((0.0 - data_min) / data_range).clamp(0.0, 1.0);
                    let zero_y = ((1.0 - zero_norm)
                        * (y_levels - 1) as f64)
                        .round() as usize;
                    let zero_y = zero_y.min(y_levels - 1);
                    let zero_row = zero_y / 4;
                    let zero_y_in_char = zero_y % 4;
                    // Set dots across all columns for 0°C line
                    for col in 0..chart_width {
                        if boundary_set.contains(&col) {
                            continue; // don't overwrite boundary lines
                        }
                        // Set both left and right column dots at zero_y
                        let left_bit = match zero_y_in_char {
                            0 => 0,
                            1 => 1,
                            2 => 2,
                            3 => 6,
                            _ => unreachable!(),
                        };
                        let right_bit = match zero_y_in_char {
                            0 => 3,
                            1 => 4,
                            2 => 5,
                            3 => 7,
                            _ => unreachable!(),
                        };
                        grid[zero_row][col] |=
                            (1 << left_bit) | (1 << right_bit);
                    }
                }

                // Scale labels for each row
                // Find which row contains the 0°C line
                let zero_row_idx = if data_min < 0.0 && data_max > 0.0 {
                    let zero_norm =
                        ((0.0 - data_min) / data_range).clamp(0.0, 1.0);
                    let zero_y = ((1.0 - zero_norm)
                        * (y_levels - 1) as f64)
                        .round() as usize;
                    Some(zero_y.min(y_levels - 1) / 4)
                } else {
                    None
                };

                let row_temps: Vec<String> = (0..chart_rows)
                    .map(|row| {
                        if Some(row) == zero_row_idx {
                            format!("{:>3}°", 0)
                        } else {
                            let mid_y = row as f64 * 4.0 + 2.0;
                            let norm =
                                1.0 - mid_y / (y_levels - 1) as f64;
                            let temp = data_min + norm * data_range;
                            format!("{:>3.0}°", temp)
                        }
                    })
                    .collect();

                for row in 0..chart_rows {
                    let mut rt = RichText::new();

                    // Y-axis label
                    rt = rt.span(
                        row_temps[row].clone(),
                        Style { dim: true, ..Style::new() },
                    );

                    for col in 0..chart_width {
                        let ch = char::from_u32(
                            0x2800 + grid[row][col] as u32,
                        )
                        .unwrap_or(' ');

                        let color = if boundary_set.contains(&col) {
                            dim_line
                        } else {
                            let avg =
                                if char_temps[row][col].is_empty() {
                                    data_min
                                } else {
                                    char_temps[row][col].iter().sum::<f64>()
                                        / char_temps[row][col].len()
                                            as f64
                                };
                            let t = ((avg - data_min) / data_range)
                                .clamp(0.0, 1.0);
                            let r =
                                (58.0 + t * (255.0 - 58.0)) as u8;
                            let g =
                                (224.0 + t * (93.0 - 224.0)) as u8;
                            let b =
                                (221.0 + t * (110.0 - 221.0)) as u8;
                            Color::Rgb(r, g, b)
                        };

                        rt = rt.span(
                            ch.to_string(),
                            Style::new().fg(color),
                        );
                    }
                    panel = panel.child(rt.fill_width());
                }

                let dim_s = Style { dim: true, ..Style::new() };

                // Day label row below chart (day-of-month numbers)
                {
                    // Get today's day-of-month
                    use std::time::{SystemTime, UNIX_EPOCH};
                    let secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let time_t = secs as libc::time_t;
                    let mut tm: libc::tm =
                        unsafe { std::mem::zeroed() };
                    unsafe { libc::localtime_r(&time_t, &mut tm) };
                    let today_mday = tm.tm_mday as usize;

                    let mut edges = vec![0usize];
                    for &bc in &boundary_cols {
                        edges.push(bc);
                    }
                    edges.push(chart_width);

                    let mut label_rt = RichText::new();
                    // Indent to align with chart (past Y-axis labels)
                    label_rt = label_rt.span("    ", dim_s.clone());
                    for seg in 0..edges.len() - 1 {
                        let start = edges[seg];
                        let end = edges[seg + 1];
                        let span_w = end - start;
                        let day_num =
                            format!("{}", (today_mday + seg - 1) % 31 + 1);

                        if span_w >= day_num.len() {
                            let pad_left =
                                (span_w - day_num.len()) / 2;
                            let pad_right =
                                span_w - day_num.len() - pad_left;
                            if pad_left > 0 {
                                label_rt = label_rt.span(
                                    " ".repeat(pad_left),
                                    dim_s.clone(),
                                );
                            }
                            label_rt = label_rt.span(
                                day_num,
                                dim_s.clone(),
                            );
                            if pad_right > 0 {
                                label_rt = label_rt.span(
                                    " ".repeat(pad_right),
                                    dim_s.clone(),
                                );
                            }
                        } else {
                            label_rt = label_rt.span(
                                day_num[..span_w].to_string(),
                                dim_s.clone(),
                            );
                        }
                    }
                    panel = panel.child(label_rt.fill_width());
                }

                // Forecast: one line per day with day number
                for (d, h, l) in w.forecast.iter() {
                    let day_num = if d.len() >= 2 {
                        &d[d.len() - 2..]
                    } else {
                        d.as_str()
                    };
                    panel = panel.child(
                        RichText::new()
                            .span(
                                format!("{day_num}  "),
                                dim_s.clone(),
                            )
                            .span(
                                format!("{h:>3}°"),
                                Style::new().fg(coral),
                            )
                            .span(" / ", dim_s.clone())
                            .span(
                                format!("{l:>3}°"),
                                Style::new().fg(cyan),
                            )
                            .fill_width(),
                    );
                }
            }
        } else {
            panel = panel.child(
                RichText::new()
                    .span(
                        "WEATHER: ",
                        Style::new().fg(cyan).bold(),
                    )
                    .span("Loading...", Style { dim: true, ..Style::new() }),
            );
        }

        panel
    }

    fn moon_view(&self) -> Column {
        // Unicode moon phase emoji (each renders as 2 columns wide)
        let emojis = ["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];
        let moon = moon_phase();
        let coral = Color::Rgb(255, 93, 110);
        let cyan = Color::Rgb(58, 224, 221);
        let dim_s = Style { dim: true, ..Style::new() };

        // Rotating wheel: 7 visible slots, current phase at center (slot 3)
        // Each slot: emoji (2 cols) + gap (2 spaces) = 4 cols per slot
        // Smooth transition via color blending based on frac
        let total_slots = 7usize;
        let center = total_slots / 2; // slot 3

        // Build both rows with identical structure so they align when centered
        let mut arrow_rt = RichText::new();
        let mut phases_rt = RichText::new();
        for slot in 0..total_slots {
            if slot > 0 {
                arrow_rt = arrow_rt.span("  ", Style::default());
                phases_rt = phases_rt.span("  ", Style::default());
            }
            // Arrow: ▼ only at center, space-width placeholder elsewhere
            // Emoji is 2 cols wide, so use 2 spaces as placeholder for non-center
            if slot == center {
                arrow_rt = arrow_rt.span("▼ ", Style::new().fg(coral).bold());
            } else {
                arrow_rt = arrow_rt.span("  ", Style::default());
            }
            // Emoji: rotate so current phase is at center
            let emoji_idx = (moon.idx + 8 + slot - center) % 8;
            let dist = (slot as isize - center as isize).unsigned_abs();
            let style = if dist == 0 {
                Style::new().fg(coral).bold()
            } else if dist == 1 {
                Style::new().fg(Color::Rgb(249, 226, 175))
            } else {
                dim_s.clone()
            };
            phases_rt = phases_rt.span(emojis[emoji_idx], style);
        }

        // Next full moon info
        let full_rt = RichText::new()
            .span("🌕 ", Style::new().fg(Color::Rgb(249, 226, 175)))
            .span(
                format!("{}", moon.full_moon_date),
                Style::new().fg(coral),
            )
            .span(
                format!(" ({}d)", moon.days_to_full),
                dim_s,
            );

        Column::new()
            .bg(Color::Rgb(26, 26, 29))
            .border(BorderStyle::double())
            .border_color(coral)
            .padding(1.0)
            .gap(0.0)
            .child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(
                        RichText::new()
                            .span("MOON", Style::new().fg(cyan).bold()),
                    ),
            )
            .child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(
                        RichText::new()
                            .span(moon.name, Style::default()),
                    ),
            )
            .child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(arrow_rt)
                    .child(phases_rt),
            )
            .child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(full_rt),
            )
    }

    fn messages_view(&self) -> Column {
        let mut col = Column::new().gap(0.5).padding(1.0).width(Dimension::Percent(100.0));
        for msg in &self.messages {
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

            card = card.child(MarkdownView::new(msg.content.clone()));
            col = col.child(card);
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
        Column::new()
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
            .child(self.input.clone())
            .child(
                RichText::new().span(
                    format!("  {}", self.daemon_url),
                    Style::new().fg(Color::Rgb(136, 136, 136)),
                ),
            )
    }

    fn sidebar_view(&self) -> Column {
        let conn_status = if self.connected {
            RichText::new()
                .span("DAEMON: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                .span("● ", Style::new().fg(Color::Rgb(166, 227, 161)).bold())
                .span("connected", Style::new().fg(Color::Rgb(166, 227, 161)))
        } else {
            RichText::new()
                .span("DAEMON: ", Style::new().fg(Color::Rgb(136, 136, 136)))
                .span("○ ", Style::new().fg(Color::Rgb(255, 93, 110)).bold())
                .span("disconnected", Style::new().fg(Color::Rgb(255, 93, 110)))
        };

        let (ctx_icon, ctx_color, ctx_label) = match self.status.context_quality.as_str() {
            "peak" => ("●", Color::Rgb(166, 227, 161), "PEAK"),
            "good" => ("●", Color::Rgb(58, 224, 221), "GOOD"),
            "degrading" => ("◐", Color::Rgb(249, 226, 175), "DEGRADING"),
            "poor" => ("○", Color::Rgb(255, 93, 110), "POOR"),
            _ => ("◆", Color::Rgb(247, 247, 247), "UNKNOWN"),
        };

        let ratio = if self.status.budget_usd > 0.0 {
            self.status.cost_usd / self.status.budget_usd
        } else {
            0.0
        };
        let bar_w = 10usize;
        let filled = (ratio.clamp(0.0, 1.0) * bar_w as f64).round() as usize;
        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(bar_w - filled));
        let bar_color = if ratio > 0.8 {
            Color::Rgb(255, 93, 110)
        } else if ratio > 0.5 {
            Color::Rgb(249, 226, 175)
        } else {
            Color::Rgb(58, 224, 221)
        };
        let pct = (ratio * 100.0).round() as u32;

        let dim = Style { dim: true, ..Style::new() };
        let label_style = Style::new().fg(Color::Rgb(136, 136, 136));

        let mut cassette = Column::new()
            .bg(Color::Rgb(26, 26, 29))
            .border(BorderStyle::double())
            .border_color(Color::Rgb(255, 93, 110))
            .padding(1.0)
            .gap(0.0)
            .child(
                Column::new()
                    .align_center()
                    .width(Dimension::Percent(100.0))
                    .child(RichText::new().span(
                        "▚▚ CASSETTE STATUS ▚▚",
                        Style::new().fg(Color::Rgb(255, 93, 110)).bold(),
                    ))
            )
            .child(conn_status.fill_width())
            .child(RichText::new()
                .span("MODEL:  ", label_style.clone())
                .span(self.status.model.as_str(), Style::new().fg(Color::Rgb(247, 247, 247)))
                .fill_width())
            .child(RichText::new()
                .span("AGENT:  ", label_style.clone())
                .span(self.status.agent_name.as_str(), Style::new().fg(Color::Rgb(58, 224, 221)))
                .fill_width())
            .child(RichText::new()
                .span("MODE:   ", label_style.clone())
                .span("ACTIVE", Style::new().fg(Color::Rgb(255, 93, 110)).bold())
                .fill_width())
            .child(Divider::horizontal())
            .child(RichText::new()
                .span("BUDGET: ", label_style.clone())
                .span(
                    format!("${:.2}", self.status.budget_usd),
                    Style::default(),
                ).fill_width())
            .child(RichText::new()
                .span("USAGE:  ", label_style.clone())
                .span(&bar, Style::new().fg(bar_color))
                .span(format!(" {pct}%"), dim.clone())
                .fill_width())
            .child(RichText::new()
                .span("CPU     ", label_style.clone())
                .span(
                    format!("[{}{}]", "█".repeat((self.status.cpu_usage * 10.0).round() as usize), "░".repeat(10 - (self.status.cpu_usage * 10.0).round() as usize)),
                    Style::new().fg(if self.status.cpu_usage > 0.8 { Color::Rgb(255, 93, 110) } else { Color::Rgb(166, 227, 161) })
                )
                .span(format!(" {:.0}%", self.status.cpu_usage * 100.0), dim.clone())
                .fill_width())
            .child(RichText::new()
                .span("RAM     ", label_style.clone())
                .span(
                    format!("[{}{}]", "█".repeat((self.status.ram_usage * 10.0).round() as usize), "░".repeat(10 - (self.status.ram_usage * 10.0).round() as usize)),
                    Style::new().fg(if self.status.ram_usage > 0.8 { Color::Rgb(255, 93, 110) } else { Color::Rgb(58, 224, 221) })
                )
                .span(format!(" {:.0}%", self.status.ram_usage * 100.0), dim.clone())
                .fill_width())
            .child(Divider::horizontal())
            .child(RichText::new()
                .span("CONTEXT: ", label_style.clone())
                .span(format!("{ctx_icon} {ctx_label}"), Style::new().fg(ctx_color).bold())
                .fill_width())
            .child(RichText::new()
                .span("STEP:    ", label_style)
                .span(self.status.step.as_str(), dim)
                .fill_width());

        if self.waiting_response {
            cassette = cassette.child(RichText::new().span(
                self.hex_gauge(),
                Style::new().fg(Color::Rgb(255, 93, 110)),
            ).fill_width());
        }

        if let Some(err) = &self.error {
            let err_msg = if err.len() > 25 {
                format!("{}...", &err[..22])
            } else {
                err.clone()
            };
            cassette = cassette.child(RichText::new().span(
                format!("ERR: {err_msg}"),
                Style::new().fg(Color::Rgb(249, 226, 175)),
            ).fill_width());
        }

        Column::new()
            .width(Dimension::Cells(34.0))
            .flex_shrink(0.0)
            .bg(Color::Rgb(37, 37, 40))
            .padding(1.0)
            .border(BorderStyle {
                style: BorderType::Thin,
                edges: BorderEdges {
                    top: false,
                    right: false,
                    bottom: false,
                    left: true,
                },
                radius: 0,
            })
            .border_color(Color::Rgb(58, 224, 221))
            .overflow_y_hidden()
            .gap(1.0)
            .child(cassette)
            .child(self.weather_view())
            .child(self.moon_view())
    }

    fn status_bar_view(&self) -> Row {
        let (pressure_icon, pressure_color) = match self.status.pressure.as_str() {
            "normal" => ("●", Color::Rgb(166, 227, 161)),
            "elevated" => ("▲", Color::Rgb(249, 226, 175)),
            "critical" => ("◆", Color::Rgb(255, 93, 110)),
            _ => ("?", Color::Rgb(247, 247, 247)),
        };

        Row::new().child(
            RichText::new()
                .span(
                    format!(" {} ", pressure_icon),
                    Style::new().fg(pressure_color).bold(),
                )
                .span(
                    format!(" {} ", self.status.step),
                    Style { dim: true, ..Style::new() },
                )
                .span(
                    " Enter:send PgUp/Dn:scroll Ctrl+C:quit ",
                    Style { dim: true, ..Style::new() },
                ),
        )
    }
}

#[derive(Serialize)]
struct AskRequest {
    prompt: String,
}

#[derive(Deserialize)]
struct AskResponse {
    response: Option<String>,
    content: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct StatusResponse {
    cost_usd: Option<f64>,
    budget_usd: Option<f64>,
    pressure: Option<String>,
    model: Option<String>,
    context_quality: Option<String>,
    step: Option<String>,
    agent_name: Option<String>,
    cpu_usage: Option<f64>,
    ram_usage: Option<f64>,
}

async fn send_to_daemon(client: &reqwest::Client, url: &str, message: &str) -> Result<String, String> {
    let response = client
        .post(url)
        .json(&AskRequest {
            prompt: message.to_string(),
        })
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        return Err(format!("daemon returned {}", response.status()));
    }

    let body: AskResponse = response.json().await.map_err(|err| err.to_string())?;
    Ok(body
        .response
        .or(body.content)
        .or(body.message)
        .unwrap_or_else(|| "(empty response)".to_string()))
}

async fn check_connection(client: &reqwest::Client, daemon_url: &str) -> bool {
    client
        .get(daemon_url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok()
}

async fn fetch_status(client: &reqwest::Client, daemon_url: &str) -> anyhow::Result<AgentStatus> {
    let url = format!("{daemon_url}/api/status");
    let body: StatusResponse = client
        .get(url)
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .json()
        .await?;

    let mut status = AgentStatus::default();
    status.cost_usd = body.cost_usd.unwrap_or(status.cost_usd);
    status.budget_usd = body.budget_usd.unwrap_or(status.budget_usd);
    status.pressure = body.pressure.unwrap_or(status.pressure);
    status.model = body.model.unwrap_or(status.model);
    status.context_quality = body.context_quality.unwrap_or(status.context_quality);
    status.step = body.step.unwrap_or(status.step);
    status.agent_name = body.agent_name.unwrap_or(status.agent_name);

    // Get system stats
    let (cpu, ram) = get_system_stats();
    status.cpu_usage = body.cpu_usage.unwrap_or(cpu);
    status.ram_usage = body.ram_usage.unwrap_or(ram);

    Ok(status)
}

fn get_system_stats() -> (f64, f64) {
    let mut cpu = 0.0;
    let mut ram = 0.0;

    #[cfg(target_os = "linux")]
    {
        // CPU usage from loadavg (1 min)
        let mut load = [0.0; 1];
        if unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } != -1 {
            // Normalize by number of cores
            let cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } as f64;
            cpu = (load[0] / cores).clamp(0.0, 1.0);
        }

        // RAM usage from /proc/meminfo
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0.0;
            let mut free = 0.0;
            let mut available = 0.0;
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    total = line.split_whitespace().nth(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                } else if line.starts_with("MemFree:") {
                    free = line.split_whitespace().nth(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                } else if line.starts_with("MemAvailable:") {
                    available = line.split_whitespace().nth(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                }
            }
            if total > 0.0 {
                let used = if available > 0.0 { total - available } else { total - free };
                ram = (used / total).clamp(0.0, 1.0);
            }
        }
    }

    (cpu, ram)
}

async fn fetch_weather(client: &reqwest::Client) -> anyhow::Result<WeatherData> {
    let loc: serde_json::Value = client
        .get("https://ipapi.co/json/")
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json()
        .await
        .unwrap_or(serde_json::json!({
            "latitude": 43.65,
            "longitude": -79.38,
            "city": "Toronto"
        }));
    let lat = loc["latitude"].as_f64().unwrap_or(43.65);
    let lon = loc["longitude"].as_f64().unwrap_or(-79.38);
    let city = loc["city"].as_str().unwrap_or("").to_string();

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,weathercode&daily=temperature_2m_max,temperature_2m_min&hourly=temperature_2m&timezone=auto"
    );
    let data: serde_json::Value = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json()
        .await?;

    let current = &data["current"];
    let temp_c = current["temperature_2m"].as_f64().unwrap_or(0.0) as i32;
    let code = current["weathercode"].as_i64().unwrap_or(0) as i32;
    let (emoji, description) = wmo_description(code);

    let daily = &data["daily"];
    let dates = daily["time"].as_array();
    let highs = daily["temperature_2m_max"].as_array();
    let lows = daily["temperature_2m_min"].as_array();

    let mut forecast = Vec::new();
    if let (Some(d), Some(h), Some(l)) = (dates, highs, lows) {
        for (i, date_val) in d.iter().enumerate().take(5.min(d.len())) {
            let date = date_val.as_str().unwrap_or("").to_string();
            let date_short = if date.len() >= 5 {
                date[date.len() - 5..].to_string()
            } else {
                date
            };
            let high = h.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            let low = l.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            forecast.push((date_short, high, low));
        }
    }

    // Parse hourly temperature data
    let hourly_temps: Vec<f64> = data["hourly"]["temperature_2m"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();

    Ok(WeatherData {
        temp_c,
        description,
        emoji,
        city,
        forecast,
        hourly_temps,
    })
}

fn wmo_description(code: i32) -> (String, String) {
    match code {
        0 => ("☀".into(), "Clear".into()),
        1 => ("🌤".into(), "Mostly Clear".into()),
        2 => ("⛅".into(), "Partly Cloudy".into()),
        3 => ("☁".into(), "Overcast".into()),
        45 | 48 => ("🌫".into(), "Fog".into()),
        51..=55 => ("🌦".into(), "Drizzle".into()),
        56 | 57 => ("🌧".into(), "Freezing Drizzle".into()),
        61..=65 => ("🌧".into(), "Rain".into()),
        66 | 67 => ("🌧".into(), "Freezing Rain".into()),
        71..=77 => ("❄".into(), "Snow".into()),
        80..=82 => ("🌦".into(), "Showers".into()),
        85 | 86 => ("🌨".into(), "Snow Showers".into()),
        95..=99 => ("⛈".into(), "Thunderstorm".into()),
        _ => ("🌡".into(), "Unknown".into()),
    }
}

struct MoonInfo {
    idx: usize,
    name: &'static str,
    days_to_full: u64,
    full_moon_date: String,
}

fn moon_phase() -> MoonInfo {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Jan 6, 2000 ~13:50 UTC (calibrated to match observed phases)
    let known_new_moon = 947_165_400u64;
    let lunation_secs = 2_551_443u64;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pos = (now - known_new_moon) % lunation_secs;
    let phase = pos as f64 / lunation_secs as f64;
    let names = [
        "New Moon",
        "Waxing Crescent",
        "First Quarter",
        "Waxing Gibbous",
        "Full Moon",
        "Waning Gibbous",
        "Last Quarter",
        "Waning Crescent",
    ];
    let idx = ((phase * 8.0) as usize) % 8;

    // Seconds until next full moon (phase = 0.5)
    let full_pos = lunation_secs / 2;
    let secs_to_full = if pos < full_pos {
        full_pos - pos
    } else {
        lunation_secs - pos + full_pos
    };
    let days_to_full = secs_to_full / 86400;

    // Format the full moon date
    let full_epoch = now + secs_to_full;
    let time_t = full_epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_t, &mut tm) };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let full_moon_date = format!(
        "{} {}",
        months[tm.tm_mon as usize],
        tm.tm_mday,
    );

    MoonInfo { idx, name: names[idx], days_to_full, full_moon_date }
}

fn format_clock() -> String {
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

fn format_timestamp() -> String {
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

async fn fetch_pending_approval(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<Option<ApprovalRequest>> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(1))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NO_CONTENT
        || response.status() == reqwest::StatusCode::NOT_FOUND
    {
        return Ok(None);
    }

    let request: ApprovalRequest = response.json().await?;
    Ok(Some(request))
}

async fn submit_approval(client: &reqwest::Client, url: &str, approved: bool) -> anyhow::Result<()> {
    client
        .post(url)
        .json(&serde_json::json!({ "approved": approved }))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    PorygonTui::new().run_with_theme(porygon_theme())
}
