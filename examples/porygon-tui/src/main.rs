//! Porygon TUI: a chat front end for the Porygon agent daemon.
//!
//! Configuration (environment variables):
//! - `PORYGON_URL`: daemon base URL (default `http://localhost:18789`).
//! - `PORYGON_WEATHER=1`: opt in to the sidebar weather panel. When set, the
//!   app calls `https://ipapi.co/json/` (which sees your public IP) to
//!   geolocate you and then `https://api.open-meteo.com` for the forecast, at
//!   startup and every 30 minutes. It is off by default and no network
//!   request to either service is made unless you opt in.

use std::time::{Duration, Instant};

use ulttui::prelude::*;
use ulttui::theme::{ButtonTheme, ControlTheme, DesignTokens, InputTheme, SelectionTheme, Theme};

mod commands;
mod daemon;
mod sidebar;
mod types;
mod views;

use commands::CommandRegistry;
use types::*;
use views::{format_clock, format_timestamp};

fn porygon_theme() -> Theme {
    let primary = Color::Rgb(255, 93, 110);
    let secondary = Color::Rgb(58, 224, 221);
    let background = Color::Rgb(30, 30, 36);
    let surface_0 = Color::Rgb(30, 30, 36);
    let surface_1 = Color::Rgb(37, 37, 40);
    let surface_2 = Color::Rgb(45, 45, 50);
    let foreground = Color::Rgb(247, 247, 247);
    let muted = Color::Rgb(136, 136, 136);
    let error = Color::Rgb(255, 93, 110);
    let success = Color::Rgb(166, 227, 161);
    let warning = Color::Rgb(249, 226, 175);
    let border_subtle = Color::Rgb(68, 68, 68);
    let border_strong = Color::Rgb(58, 224, 221);
    let text_accent = Color::Rgb(58, 224, 221);

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

pub(crate) struct PorygonTui {
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) input: TextInput,
    pub(crate) chat_scroll: ScrollView,
    pub(crate) status: AgentStatus,
    pub(crate) daemon_url: String,
    pub(crate) client: reqwest::Client,
    pub(crate) connected: bool,
    pub(crate) error: Option<String>,
    pub(crate) spinner_frame: usize,
    pub(crate) waiting_response: bool,
    pub(crate) streaming_content: String,
    pub(crate) stream_token_count: usize,
    pub(crate) started: bool,
    pub(crate) pending_approval: Option<ApprovalRequest>,
    pub(crate) show_approval_modal: bool,
    pub(crate) weather: Option<WeatherData>,
    /// Opt-in flag for the weather panel (see the crate docs).
    pub(crate) weather_enabled: bool,
    pub(crate) clock_text: String,
    pub(crate) terminal_width: u16,
    pub(crate) terminal_height: u16,
    pub(crate) started_connection_msg: bool,
    pub(crate) command_registry: CommandRegistry,
    pub(crate) session_id: Option<String>,
    pub(crate) suggestions: Vec<(String, String)>,
    pub(crate) tool_calls: Vec<ToolCall>,
    pub(crate) stream_started_at: Option<Instant>,
    pub(crate) first_token_at: Option<Instant>,
    pub(crate) last_token_at: Option<Instant>,
    pub(crate) append_latencies_ms: Vec<u64>,
    pub(crate) last_stream_perf: Option<StreamPerfSummary>,
    pub(crate) stream_ttft_history_ms: Vec<u64>,
    pub(crate) append_latency_history_ms: Vec<u64>,
    pub(crate) cancel_visual_stop_history_ms: Vec<u64>,
    pub(crate) mcp_server_count: usize,
    pub(crate) mcp_healthy_count: usize,
    pub(crate) mcp_last_error: Option<String>,
    pub(crate) model_last_error: Option<String>,
}

impl PorygonTui {
    fn new() -> Self {
        let daemon_url =
            std::env::var("PORYGON_URL").unwrap_or_else(|_| "http://localhost:18789".to_string());

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
            streaming_content: String::new(),
            stream_token_count: 0,
            started: false,
            pending_approval: None,
            show_approval_modal: false,
            weather: None,
            weather_enabled: weather_enabled_from_env(),
            clock_text: format_clock(),
            terminal_width: 200,
            terminal_height: 50,
            started_connection_msg: false,
            command_registry: CommandRegistry::new(),
            session_id: None,
            suggestions: Vec::new(),
            tool_calls: Vec::new(),
            stream_started_at: None,
            first_token_at: None,
            last_token_at: None,
            append_latencies_ms: Vec::new(),
            last_stream_perf: None,
            stream_ttft_history_ms: Vec::new(),
            append_latency_history_ms: Vec::new(),
            cancel_visual_stop_history_ms: Vec::new(),
            mcp_server_count: 0,
            mcp_healthy_count: 0,
            mcp_last_error: None,
            model_last_error: None,
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
    StreamToken(String),
    StreamComplete,
    StreamError(String),
    AbortStream,
    StreamAbortAck(Result<(), String>),
    SessionsFetched(Vec<SessionInfo>),
    SessionResumed(Vec<ChatMessage>),
    SessionSaved,
    SessionForked(String),
    SessionExported(String),
    SessionError(String),
    MemoryStored,
    MemoryRecalled(Vec<MemoryResult>),
    MemoryBrowsed(Vec<MemoryResult>),
    MemoryError(String),
    ToolsFetched(Vec<ToolInfo>),
    ToolDetailsFetched(ToolInfo),
    McpStatusFetched(Vec<McpServerInfo>),
    McpToolsFetched(Vec<ToolInfo>),
    McpError(String),
    ToolsError(String),
    FileTreeFetched {
        path: Option<String>,
        tree: FileTreeEntry,
    },
    FileTreeError(String),
    ProjectInfoFetched(ProjectInfo),
    ProjectSwitched,
    ProjectError(String),
    PlanCreated(Plan),
    PlanFetched(Plan),
    PlanAdvanced,
    PlanError(String),
    KanbanFetched(KanbanBoard),
    KanbanTaskAdded,
    KanbanTaskMoved,
    KanbanError(String),
    AsyncTasksFetched(Vec<AsyncTaskInfo>),
    AsyncTaskDetailFetched(AsyncTaskInfo),
    AsyncTaskCancelled,
    AsyncTaskError(String),
    CronFetched(Vec<CronJob>),
    CronTriggered,
    CronToggled,
    CronError(String),
    ModelsFetched(Vec<ModelInfo>),
    ModelChanged,
    ModelsError(String),
    SearchResultsFetched(Vec<SearchResult>),
    SearchError(String),
    ProfileFetched(TrainerProfile),
    PartyFetched(Vec<AgentInfo>),
    ProfileError(String),
    ValidationResultFetched(ValidationResult),
    GymResultFetched(ValidationResult),
    ValidationError(String),
    VirtualScrollBenchReady(String),
    DiffBenchReady(String),
    ExitRequested,
}

impl Application for PorygonTui {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        self.handle_update(msg)
    }

    fn subscriptions(&self) -> Sub<Msg> {
        let mut subs = vec![
            Sub::every(Duration::from_secs(3), || Msg::PollConnection),
            Sub::every(Duration::from_secs(2), || Msg::PollStatus),
            Sub::every(Duration::from_millis(500), || Msg::PollApprovals),
            Sub::every(Duration::from_secs(1), || Msg::ClockTick),
        ];
        if self.weather_enabled {
            subs.push(Sub::every(Duration::from_secs(1800), || Msg::PollWeather));
        }
        if !self.started {
            subs.push(Sub::every(Duration::from_millis(1), || Msg::Init));
        }
        if self.waiting_response {
            subs.push(Sub::every(Duration::from_millis(80), || Msg::SpinnerTick));
        }
        Sub::batch(subs)
    }

    fn on_event(&mut self, event: &ulttui::event::Event) -> Option<Msg> {
        self.handle_event(event)
    }

    fn view(&self) -> impl View {
        self.render_view()
    }
}

/// Weather is opt-in because it reveals the user's IP to a geolocation service.
fn weather_enabled_from_env() -> bool {
    std::env::var("PORYGON_WEATHER").is_ok_and(|value| value == "1")
}

fn main() -> anyhow::Result<()> {
    PorygonTui::new().run_with_theme(porygon_theme())
}
