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

mod approval_client;
mod commands;
mod connection;
mod contract_probe;
mod conversations;
mod daemon;
mod native_stream;
mod session_ui;
mod sidebar;
mod transcript;
mod types;
mod views;
mod work;
mod work_views;

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
    pub(crate) conversations: conversations::Conversations,
    pub(crate) work: work::WorkState,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) transcript: std::cell::RefCell<transcript::Transcript>,
    pub(crate) input: TextInput,
    pub(crate) newline_prefix: bool,
    pub(crate) show_help: bool,
    pub(crate) quit_armed: bool,
    pub(crate) follow_output: bool,
    pub(crate) unread_output: bool,
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
    pub(crate) connection: Option<connection::Connection>,
    pub(crate) current_run: Option<String>,
    pub(crate) conversation_id: Option<String>,
    pub(crate) stream_control: Option<tokio::sync::mpsc::Sender<native_stream::Control>>,
    pub(crate) run_unresolved: bool,
    pub(crate) stopping: bool,
    pub(crate) approval_in_flight: bool,
    pub(crate) approval_is_stream: bool,
    pub(crate) tool_ids: Vec<String>,
    pub(crate) current_response: Option<usize>,
    pub(crate) run_can_continue: bool,
    pub(crate) submitting_parked_run: bool,
    pub(crate) run_epoch: u64,
    pub(crate) queued_stream_approvals: std::collections::VecDeque<ApprovalRequest>,
}

impl PorygonTui {
    fn new() -> Self {
        let daemon_url =
            std::env::var("PORYGON_URL").unwrap_or_else(|_| "http://localhost:18789".to_string());
        let connection = connection::Connection::from_env();
        let connection_error = connection.as_ref().err().cloned();
        let connection = connection.ok();
        let client = connection
            .as_ref()
            .map(|c| c.client.clone())
            .unwrap_or_default();

        Self {
            transcript: std::cell::RefCell::default(),
            conversations: conversations::Conversations::default(),
            work: work::WorkState::default(),
            messages: vec![ChatMessage {
                role: "system".into(),
                content: "Type a message to start a conversation.\nUse F1 for keys and commands. Your draft stays here while disconnected.".into(),
                timestamp: format_timestamp(),
            }],
            input: TextInput::new()
                .without_vim_mode()
                .id("chat-input")
                .multiline()
                .height(3)
                .placeholder("Message Porygon"),
            newline_prefix: false,
            show_help: false,
            quit_armed: false,
            follow_output: true,
            unread_output: false,
            chat_scroll: ScrollView::new(),
            status: AgentStatus::default(),
            daemon_url,
            client,
            connected: false,
            error: connection_error,
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
            connection,
            current_run: None,
            conversation_id: None,
            stream_control: None,
            run_unresolved: false,
            stopping: false,
            approval_in_flight: false,
            approval_is_stream: false,
            tool_ids: Vec::new(),
            current_response: None,
            run_can_continue: false,
            submitting_parked_run: false,
            run_epoch: 0,
            queued_stream_approvals: std::collections::VecDeque::new(),
        }
    }
}

#[derive(Debug)]
enum Msg {
    FlushDrafts,
    OpenConversations,
    ConversationsListed(Result<Vec<conversations::Conversation>, String>),
    LoadConversation(String, u64),
    ConversationLoaded(String, u64, Result<Vec<ChatMessage>, String>),
    Init,
    SendMessage,
    PollConnection,
    PollStatus,
    ConnectionChecked(Result<(), String>),
    StatusUpdated(AgentStatus),
    SpinnerTick,
    ClockTick,
    PollWeather,
    WeatherFetched(WeatherData),
    WeatherFailed,
    OpenWork(work::Page),
    RefreshWork,
    WorkRuns(Result<Vec<work::WorkRun>, String>),
    WorkApprovals(Result<Vec<approval_client::PendingApproval>, String>),
    WorkReports(String, Result<Vec<work::Report>, String>),
    InspectWork,
    ApprovalReceived(ApprovalRequest, bool),
    ApprovalDecision(bool),
    ApprovalSubmitted(String, Result<(), String>),
    DismissApproval,
    RefreshApproval,
    ApprovalReconciled(String, Result<bool, String>),
    StreamToken(String),
    AbortStream,
    NativeEvent(String, u64, native_stream::Event),
    InspectRun,
    RunObserved(String, u64, Result<connection::RunSnapshot, String>),
    StopObserved(String, u64, Result<connection::RunSnapshot, String>),
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
    DraftExitWarning,
}

impl Application for PorygonTui {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        self.handle_update(msg)
    }

    fn subscriptions(&self) -> Sub<Msg> {
        let mut subs = vec![
            Sub::every(Duration::from_millis(250), || Msg::FlushDrafts),
            Sub::every(Duration::from_secs(3), || Msg::PollConnection),
            Sub::every(Duration::from_secs(2), || Msg::PollStatus),
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
        if self.run_unresolved && !self.waiting_response {
            subs.push(Sub::every(Duration::from_secs(3), || Msg::InspectRun));
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
    if std::env::args().any(|arg| arg == "--verify-native-contract") {
        return contract_probe::run().map_err(anyhow::Error::msg);
    }
    PorygonTui::new().run_with_theme(porygon_theme())?;
    Ok(())
}
