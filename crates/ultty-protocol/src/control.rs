//! Control protocol for CLI automation (ulttyctl).
//!
//! This module defines messages for external control of ultty,
//! separate from the TUI app protocol.

use crate::{Key, Modifiers, OverrideFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Output format used by `ulttyctl` responses.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-readable plain text output.
    #[default]
    Text,
    /// Machine-readable JSON output.
    Json,
}

/// Where `SplitPane` puts the new pane relative to the focused one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    /// Side by side, the new pane on the right.
    Right,
    /// Stacked, the new pane below.
    Down,
}

/// Complete control socket request envelope.
///
/// `ulttyctl` sends this structure so automation can preserve argument
/// boundaries and request machine-readable responses without relying on
/// shell-like command parsing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlEnvelope {
    /// Preferred response format for this client.
    #[serde(default)]
    pub output_format: OutputFormat,
    /// Typed request to execute.
    pub request: ControlRequest,
}

/// Request message sent over the control socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    // === Input Injection ===
    SendText {
        text: String,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    SendKey {
        key: Key,
        modifiers: Modifiers,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    SendRaw {
        data: Vec<u8>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Command Execution ===
    NewTab {
        title: Option<String>,
        command: Option<String>,
        args: Vec<String>,
        working_dir: Option<String>,
    },

    RunCommand {
        command: String,
        args: Vec<String>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    /// Open a pane next to `target` (the focused pane when omitted), running
    /// `command` (the shell when omitted). Answered with
    /// `ControlData::PaneOpened`.
    SplitPane {
        direction: SplitDirection,
        command: Option<String>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        /// Pane to split (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    CloseTab {
        index: Option<usize>,
    },

    // === State Queries ===
    GetText {
        include_scrollback: bool,
        scrollback_lines: Option<usize>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    GetSelection {
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    ListTabs,

    /// Every pane in every tab, tabs in order and panes in layout order.
    ListPanes,

    /// The native apps drawing in a pane's tab: each one's region tree
    /// with computed layout, content and focus. Read-only.
    Regions {
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    GetInfo,

    // === Overrides (ADR 0023 §9) ===
    /// Push a user override into the native app in a pane's tab, where it
    /// applies on the next frame, in memory, over the app's override files.
    /// Answered `Ok` once the app applied it, or with the app's parse
    /// error.
    PushOverride {
        format: OverrideFormat,
        /// The override file's text, at most `MAX_OVERRIDE_SOURCE_LEN` bytes.
        source: String,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    /// Take back the override pushed into the native app in a pane's tab;
    /// its files apply as before.
    ClearOverride {
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    /// The last command the pane's shell ran (or is running) and what it
    /// printed, from OSC 133 shell integration marks.
    LastCommand {
        /// Keep only the last this many lines of output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail_lines: Option<u64>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Search ===
    Search {
        pattern: String,
        regex: bool,
        case_sensitive: bool,
        include_scrollback: bool,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Wait Operations ===
    /// Wait until something happens in a pane: `pattern` appears (the
    /// default), or the event `until` names. The pane is fixed when the
    /// request arrives: `target`, or the pane focused at that moment.
    WaitFor {
        /// Text to wait for; ignored unless `until` is `Text`.
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        regex: bool,
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "WaitCondition::is_text")]
        until: WaitCondition,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Events ===
    /// Turn this connection into an event stream: after the `Subscribed`
    /// reply, one [`ControlEvent`] per line until the client disconnects.
    Subscribe {
        /// Only events about this pane; every pane's events when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Window Control ===
    FocusWindow,

    SwitchTab {
        index: usize,
    },

    ToggleFullscreen,

    Resize {
        width: u32,
        height: u32,
    },

    SetOpacity {
        opacity: f32,
    },

    /// Show what a pane is doing in the tab bar; `None` clears it.
    SetPaneStatus {
        #[serde(default)]
        status: Option<PaneStatus>,
        /// Pane to act on (an id from `ListPanes`); the focused pane when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u64>,
    },

    // === Screenshot ===
    Screenshot {
        path: Option<String>,
    },

    // === Batch Mode ===
    Batch {
        commands: Vec<ControlRequest>,
    },

    // === Auth ===
    Authenticate {
        token: String,
    },

    /// Authenticate and execute `request` in one round trip.
    ///
    /// The control server handles one request per connection, so clients
    /// (ulttyctl) wrap their request with the token the
    /// instance wrote at start. The response is the inner request's response.
    WithToken {
        token: String,
        request: Box<ControlRequest>,
    },

    // === Misc ===
    Ping,

    Version,
}

/// What a `WaitFor` request waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitCondition {
    /// The pattern appears on screen or in scrollback.
    #[default]
    Text,
    /// The next command finishes (OSC 133;D; needs shell integration).
    CommandFinished,
    /// The bell rings.
    Bell,
    /// No output for `ms` milliseconds.
    Quiet { ms: u64 },
}

impl WaitCondition {
    pub fn is_text(&self) -> bool {
        *self == WaitCondition::Text
    }
}

/// What a program says it is doing in its pane, shown in the tab bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneStatus {
    pub state: PaneState,
    /// A few words, such as "running tests". Shown in the title band.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// How far along, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    /// Ask for the user: marks the window urgent while it is unfocused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attention: bool,
}

/// The kind of thing a pane is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneState {
    Idle,
    Working,
    /// Waiting on the user.
    Waiting,
    Done,
    Error,
}

impl PaneStatus {
    /// Longest label kept, in characters.
    pub const MAX_LABEL_CHARS: usize = 64;

    /// The status with its label cut to [`Self::MAX_LABEL_CHARS`] and free
    /// of control characters, and its progress finite and within 0..=1.
    pub fn sanitized(mut self) -> Self {
        self.label = self
            .label
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(Self::MAX_LABEL_CHARS)
            .collect::<String>()
            .trim()
            .to_string();
        self.progress = self
            .progress
            .filter(|progress| progress.is_finite())
            .map(|progress| progress.clamp(0.0, 1.0));
        self
    }

    /// How much the status asks for the user's eye: an error first, then a
    /// pane waiting on the user, then work in progress.
    pub fn urgency(&self) -> u8 {
        match (self.state, self.attention) {
            (PaneState::Error, _) => 4,
            (PaneState::Waiting, _) | (_, true) => 3,
            (PaneState::Working, _) => 2,
            (PaneState::Done, _) => 1,
            (PaneState::Idle, _) => 0,
        }
    }
}

/// Response envelope returned by the control server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlResponse {
    Ok,
    Text { text: String },
    Data(ControlData),
    Error { code: u16, message: String },
}

/// Structured payload for successful control responses.
///
/// Variants carry named fields: serde's internal `type` tag cannot wrap a
/// bare string, list or option, so such a variant would fail to serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlData {
    Info(TerminalInfo),
    Tabs {
        tabs: Vec<TabInfo>,
    },
    Panes {
        panes: Vec<PaneInfo>,
    },
    Selection {
        text: Option<String>,
    },
    Version(VersionInfo),
    Pong,
    ScreenshotSaved {
        path: String,
    },
    ScreenshotData {
        data: Vec<u8>,
    },
    SearchResults {
        matches: Vec<SearchMatch>,
    },
    BatchResults {
        results: Vec<ControlResponse>,
    },
    Authenticated,
    WaitMatched {
        line: usize,
        col: usize,
    },
    /// A `WaitFor` for an event saw it.
    WaitEvent {
        event: ControlEvent,
    },
    /// A `WaitFor` for quiet saw no output for the whole period.
    WaitQuiet,
    WaitTimeout,
    /// The connection is now an event stream.
    Subscribed,
    /// `NewTab` opened a tab; `pane` is its pane's id.
    TabOpened {
        tab_index: usize,
        pane: u64,
    },
    /// `SplitPane` opened a pane in the tab at `tab_index`.
    PaneOpened {
        tab_index: usize,
        pane: u64,
    },
    /// The native apps in a pane's tab; see `ControlRequest::Regions`.
    Regions {
        scenes: Vec<NativeScene>,
    },
    /// A pane's last command; see `ControlRequest::LastCommand`.
    LastCommand {
        command: String,
        /// What it printed; empty when `output_available` is false.
        output: String,
        /// Its exit status once finished, if the shell reported one.
        exit_code: Option<i32>,
        /// Still running: `output` is what it printed so far.
        running: bool,
        /// False when the output's rows are gone (cleared, reset, or out
        /// of the scrollback).
        output_available: bool,
    },
}

/// Something that happened in a pane, sent to subscribers one per line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ControlEvent {
    /// A pane was opened (a new tab or a split).
    PaneOpened { pane: u64, tab_index: usize },
    /// A pane was closed.
    PaneClosed { pane: u64 },
    /// The shell started running a command (OSC 133;C).
    CommandStarted { pane: u64 },
    /// The command finished (OSC 133;D), with its exit code when the shell
    /// reported one.
    CommandFinished { pane: u64, exit_code: Option<i32> },
    /// The bell rang.
    Bell { pane: u64 },
    /// A program asked for a desktop notification (OSC 9/777).
    Notification {
        pane: u64,
        title: String,
        body: String,
    },
    /// A program set the pane title (OSC 0/2).
    TitleChanged { pane: u64, title: String },
    /// The pane's shell process exited.
    ProcessExited { pane: u64, status: i32 },
}

impl ControlEvent {
    /// The pane the event is about.
    pub fn pane(&self) -> u64 {
        match self {
            ControlEvent::PaneOpened { pane, .. }
            | ControlEvent::PaneClosed { pane }
            | ControlEvent::CommandStarted { pane }
            | ControlEvent::CommandFinished { pane, .. }
            | ControlEvent::Bell { pane }
            | ControlEvent::Notification { pane, .. }
            | ControlEvent::TitleChanged { pane, .. }
            | ControlEvent::ProcessExited { pane, .. } => *pane,
        }
    }
}

/// A single text search match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// Zero-based line number of the match.
    pub line: usize,
    /// Zero-based column of the match.
    pub col: usize,
    /// Matched text.
    pub text: String,
    /// Context preceding the match.
    pub context_before: String,
    /// Context following the match.
    pub context_after: String,
}

/// Snapshot of terminal state returned by `get_info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalInfo {
    /// Terminal width in cells.
    pub cols: u16,
    /// Terminal height in cells.
    pub rows: u16,
    /// Cursor column in cells.
    pub cursor_col: u16,
    /// Cursor row in cells.
    pub cursor_row: u16,
    /// Currently selected tab index.
    pub current_tab: usize,
    /// Total number of tabs.
    pub tab_count: usize,
    /// Whether this window has focus.
    pub focused: bool,
    /// Working directory for the active tab, if known.
    pub cwd: Option<String>,
}

/// Metadata describing a single tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Tab index.
    pub index: usize,
    /// User-visible tab title.
    pub title: String,
    /// Whether this tab is currently active.
    pub active: bool,
    /// Foreground process ID when available.
    pub pid: Option<u32>,
    /// Working directory for the tab, if known.
    pub cwd: Option<String>,
}

/// One native app's scene, as `ControlRequest::Regions` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeScene {
    /// The app's session, unique for the terminal's lifetime.
    pub session: u64,
    /// Connected in-band over the PTY rather than over the tab's socket.
    pub in_band: bool,
    /// The region with keyboard focus, if any.
    pub focused: Option<u32>,
    /// Every region of the full-scene tree, root first, in depth-first
    /// (drawing) order. Surface regions are listed under `surfaces`.
    pub regions: Vec<RegionInfo>,
    /// The app's inline surfaces (ADR 0020), in creation order. Left out
    /// of JSON when there are none, and empty from a terminal that
    /// predates them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<SurfaceInfo>,
}

/// One inline surface of a native scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceInfo {
    /// The surface root's region id.
    pub id: u32,
    /// Height in rows, fixed for the surface's life.
    pub rows: u16,
    /// `unplaced` until its anchor and `CreateSurface` have both arrived,
    /// then `placed`.
    pub state: String,
    /// Line of the surface's first row, counted from the oldest line the
    /// terminal still holds, once placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_line: Option<u64>,
    /// The surface's regions, its root first, in depth-first order.
    /// Positions are from the surface's top left; depth from its root.
    pub regions: Vec<RegionInfo>,
}

/// One region of a native scene. Positions and sizes are in cells, from
/// the top left of the app's area, before any scrolled ancestor moves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionInfo {
    pub id: u32,
    /// None for the root.
    pub parent: Option<u32>,
    /// The root is 0.
    pub depth: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// How far the region's content is scrolled, in cells.
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// `none`, `text`, `rich_text`, or a widget: `input`, `markdown`,
    /// `code`, `progress`, `spinner`, `image`, `button`.
    pub content: String,
    /// The text the region shows, for content that has text (an input's
    /// value, a button's label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Key-value pairs the app set with `SetRegionMetadata` (a later
    /// duplicate key wins).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Effective accessibility role (spec §10.5), snake_case: `group`,
    /// `text`, `log`, `input`, `button`, `list_item` and so on. Derived
    /// from the content unless the app set one with `SetSemantics`.
    /// Empty from a terminal that predates semantics.
    #[serde(default)]
    pub role: String,
    /// Effective accessible name, if the region has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `off`, `polite` or `assertive`; left out of JSON when `off`.
    #[serde(default = "live_off", skip_serializing_if = "is_live_off")]
    pub live: String,
}

fn live_off() -> String {
    "off".to_string()
}

fn is_live_off(live: &str) -> bool {
    live == "off"
}

/// Metadata describing a single pane (one terminal in a tab's layout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Pane id. Unique within one Ultty process and never reused, so it
    /// stays valid across splits, focus changes and tab switches until the
    /// pane closes. Treat it as opaque.
    pub id: u64,
    /// Index of the tab holding the pane.
    pub tab_index: usize,
    /// Whether this is the focused pane of its tab.
    pub active: bool,
    /// Whether this pane has keyboard focus: the active pane of the active tab.
    pub focused: bool,
    /// Title the program set (OSC 0/2); empty when none was set.
    pub title: String,
    /// Name of the foreground process, when it can be read.
    pub foreground: Option<String>,
    /// Shell process ID.
    pub pid: Option<u32>,
    /// Working directory, if known.
    pub cwd: Option<String>,
    /// Pane width in cells.
    pub cols: u16,
    /// Pane height in cells.
    pub rows: u16,
    /// What the pane's program said it is doing, if it set a status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PaneStatus>,
}

/// Build and protocol version information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Semantic version string.
    pub version: String,
    /// Control protocol version.
    pub protocol_version: u16,
    /// Optional VCS revision hash.
    pub git_hash: Option<String>,
}

/// Control protocol frame magic (`"UC"`).
pub const CONTROL_MAGIC: [u8; 2] = [0x55, 0x43]; // "UC"

/// Current control protocol version.
pub const CONTROL_VERSION: u16 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    fn every_response() -> Vec<ControlResponse> {
        let data = vec![
            ControlData::Info(TerminalInfo {
                cols: 80,
                rows: 24,
                cursor_col: 1,
                cursor_row: 2,
                current_tab: 0,
                tab_count: 1,
                focused: true,
                cwd: Some("/home".to_string()),
            }),
            ControlData::Tabs {
                tabs: vec![TabInfo {
                    index: 0,
                    title: "zsh".to_string(),
                    active: true,
                    pid: Some(42),
                    cwd: None,
                }],
            },
            ControlData::Panes {
                panes: vec![PaneInfo {
                    id: 7,
                    tab_index: 0,
                    active: true,
                    focused: false,
                    title: "vim".to_string(),
                    foreground: Some("vim".to_string()),
                    pid: Some(42),
                    cwd: None,
                    cols: 40,
                    rows: 24,
                    status: Some(PaneStatus {
                        state: PaneState::Working,
                        label: "tests".to_string(),
                        progress: Some(0.5),
                        attention: false,
                    }),
                }],
            },
            ControlData::Selection {
                text: Some("picked".to_string()),
            },
            ControlData::Selection { text: None },
            ControlData::Version(VersionInfo {
                version: "0.1.0".to_string(),
                protocol_version: 1,
                git_hash: None,
            }),
            ControlData::Pong,
            ControlData::ScreenshotSaved {
                path: "/tmp/shot.png".to_string(),
            },
            ControlData::ScreenshotData {
                data: vec![1, 2, 3],
            },
            ControlData::SearchResults {
                matches: vec![SearchMatch {
                    line: 3,
                    col: 4,
                    text: "needle".to_string(),
                    context_before: "a ".to_string(),
                    context_after: " b".to_string(),
                }],
            },
            ControlData::BatchResults {
                results: vec![ControlResponse::Ok],
            },
            ControlData::Authenticated,
            ControlData::WaitMatched { line: 1, col: 2 },
            ControlData::WaitTimeout,
            ControlData::Subscribed,
            ControlData::WaitEvent {
                event: ControlEvent::CommandFinished {
                    pane: 1,
                    exit_code: Some(2),
                },
            },
            ControlData::WaitQuiet,
            ControlData::TabOpened {
                tab_index: 2,
                pane: 9,
            },
            ControlData::PaneOpened {
                tab_index: 2,
                pane: 10,
            },
            ControlData::LastCommand {
                command: "make".to_string(),
                output: "ok".to_string(),
                exit_code: Some(0),
                running: false,
                output_available: true,
            },
            ControlData::Regions {
                scenes: vec![NativeScene {
                    session: 3,
                    in_band: false,
                    focused: Some(2),
                    regions: vec![RegionInfo {
                        id: 2,
                        parent: Some(0),
                        depth: 1,
                        x: 0.0,
                        y: 1.0,
                        width: 80.0,
                        height: 22.0,
                        scroll_x: 0.0,
                        scroll_y: 4.0,
                        content: "text".to_string(),
                        text: Some("log line".to_string()),
                        metadata: BTreeMap::from([("id".to_string(), "log".to_string())]),
                        role: "log".to_string(),
                        label: Some("Build log".to_string()),
                        live: "polite".to_string(),
                    }],
                    surfaces: vec![SurfaceInfo {
                        id: 7,
                        rows: 3,
                        state: "unplaced".to_string(),
                        anchor_line: None,
                        regions: Vec::new(),
                    }],
                }],
            },
        ];
        let mut responses = vec![
            ControlResponse::Ok,
            ControlResponse::Text {
                text: "screen".to_string(),
            },
            ControlResponse::Error {
                code: 4,
                message: "nope".to_string(),
            },
        ];
        responses.extend(data.into_iter().map(ControlResponse::Data));
        responses
    }

    #[test]
    fn pane_status_is_cleaned_before_it_is_shown() {
        let status = PaneStatus {
            state: PaneState::Working,
            label: format!("  run\x1b[31m{}", "x".repeat(100)),
            progress: Some(7.0),
            attention: false,
        }
        .sanitized();
        assert!(!status.label.contains('\x1b'));
        assert!(status.label.starts_with("run ["));
        assert_eq!(
            status.label.chars().count(),
            PaneStatus::MAX_LABEL_CHARS - 2
        );
        assert_eq!(status.progress, Some(1.0));
        let nan = PaneStatus {
            progress: Some(f32::NAN),
            ..status
        };
        assert_eq!(nan.sanitized().progress, None);
    }

    #[test]
    fn errors_outrank_waiting_outranks_work() {
        let status = |state, attention| PaneStatus {
            state,
            label: String::new(),
            progress: None,
            attention,
        };
        let error = status(PaneState::Error, false).urgency();
        let waiting = status(PaneState::Waiting, false).urgency();
        let attention = status(PaneState::Working, true).urgency();
        let working = status(PaneState::Working, false).urgency();
        let done = status(PaneState::Done, false).urgency();
        assert!(error > waiting && waiting == attention && attention > working && working > done);
    }

    #[test]
    fn pane_status_json_is_small() {
        let json = serde_json::to_string(&PaneStatus {
            state: PaneState::Done,
            label: String::new(),
            progress: None,
            attention: false,
        })
        .unwrap();
        assert_eq!(json, r#"{"state":"done"}"#);
    }

    #[test]
    fn events_are_flat_json_objects_named_by_event() {
        let event = ControlEvent::CommandFinished {
            pane: 3,
            exit_code: Some(1),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"event":"command_finished","pane":3,"exit_code":1}"#
        );
        assert_eq!(serde_json::from_str::<ControlEvent>(&json).unwrap(), event);
        assert_eq!(event.pane(), 3);
    }

    #[test]
    fn wait_condition_defaults_to_text_and_round_trips() {
        let old: ControlRequest = serde_json::from_str(
            r#"{"type":"wait_for","pattern":"ok","regex":false,"timeout_ms":5}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            ControlRequest::WaitFor {
                until: WaitCondition::Text,
                ..
            }
        ));
        for until in [
            WaitCondition::CommandFinished,
            WaitCondition::Bell,
            WaitCondition::Quiet { ms: 500 },
        ] {
            let request = ControlRequest::WaitFor {
                pattern: String::new(),
                regex: false,
                timeout_ms: 1000,
                until,
                target: None,
            };
            let json = serde_json::to_string(&request).unwrap();
            assert_eq!(
                serde_json::from_str::<ControlRequest>(&json).unwrap(),
                request
            );
        }
    }

    /// Clients written before pane targets keep working: an omitted `target`
    /// means the focused pane, and an unset one is not sent.
    #[test]
    fn target_is_optional_on_the_wire() {
        let old: ControlRequest =
            serde_json::from_str(r#"{"type":"send_text","text":"hi"}"#).unwrap();
        assert_eq!(
            old,
            ControlRequest::SendText {
                text: "hi".to_string(),
                target: None
            }
        );
        let old: ControlRequest = serde_json::from_str(r#"{"type":"get_selection"}"#).unwrap();
        assert_eq!(old, ControlRequest::GetSelection { target: None });

        let untargeted = serde_json::to_string(&ControlRequest::GetSelection { target: None });
        assert_eq!(untargeted.unwrap(), r#"{"type":"get_selection"}"#);
        let targeted: ControlRequest =
            serde_json::from_str(r#"{"type":"get_selection","target":4}"#).unwrap();
        assert_eq!(targeted, ControlRequest::GetSelection { target: Some(4) });
    }

    #[test]
    fn split_pane_names_its_direction_and_defaults_its_arguments() {
        let request = ControlRequest::SplitPane {
            direction: SplitDirection::Right,
            command: Some("htop".to_string()),
            args: vec!["-d".to_string()],
            working_dir: Some("/tmp".to_string()),
            target: Some(4),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"type":"split_pane","direction":"right","command":"htop","args":["-d"],"working_dir":"/tmp","target":4}"#
        );
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            request
        );

        let bare: ControlRequest =
            serde_json::from_str(r#"{"type":"split_pane","direction":"down","command":null}"#)
                .unwrap();
        assert_eq!(
            bare,
            ControlRequest::SplitPane {
                direction: SplitDirection::Down,
                command: None,
                args: Vec::new(),
                working_dir: None,
                target: None,
            }
        );
    }

    #[test]
    fn override_requests_spell_their_format_lowercase_and_default_the_target() {
        let push = ControlRequest::PushOverride {
            format: OverrideFormat::Luau,
            source: "return {}".to_string(),
            target: Some(2),
        };
        let json = serde_json::to_string(&push).unwrap();
        assert_eq!(
            json,
            r#"{"type":"push_override","format":"luau","source":"return {}","target":2}"#
        );
        assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), push);

        let clear: ControlRequest = serde_json::from_str(r#"{"type":"clear_override"}"#).unwrap();
        assert_eq!(clear, ControlRequest::ClearOverride { target: None });
        assert_eq!(
            serde_json::to_string(&clear).unwrap(),
            r#"{"type":"clear_override"}"#
        );
    }

    /// ulttyctl reads every reply as JSON, so each one must survive the trip.
    #[test]
    fn every_response_round_trips_through_json() {
        for response in every_response() {
            let json = serde_json::to_string(&response)
                .unwrap_or_else(|e| panic!("{response:?} does not serialize: {e}"));
            let back: ControlResponse = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{json} does not parse back: {e}"));
            assert_eq!(back, response);
        }
    }

    /// A scene from a terminal that predates surfaces still parses, and a
    /// scene without surfaces is written as it was before them.
    #[test]
    fn scenes_without_surfaces_keep_their_old_json() {
        let old = r#"{"session":1,"in_band":false,"focused":null,"regions":[]}"#;
        let scene: NativeScene = serde_json::from_str(old).unwrap();
        assert!(scene.surfaces.is_empty());
        assert_eq!(serde_json::to_string(&scene).unwrap(), old);

        let surface: SurfaceInfo =
            serde_json::from_str(r#"{"id":7,"rows":3,"state":"unplaced","regions":[]}"#).unwrap();
        assert_eq!(surface.anchor_line, None);
    }
}
