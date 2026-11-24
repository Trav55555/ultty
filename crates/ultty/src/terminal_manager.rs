//! Terminal manager for multi-tab and multi-pane support.
//!
//! This module manages multiple terminal instances, supporting:
//! - Multiple tabs, each with its own terminal
//! - Pane splitting within tabs
//! - Keyboard navigation between tabs and panes

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::Path;

use std::sync::Arc;

use crate::event_loop::{EventLoop as TerminalEventLoop, TerminalEvent, Waker};
use crate::grid::ThemeColors;
use crate::pane_layout::{Divider, FocusDirection, PaneLayout, PaneNode, Rect};
use crate::pty::{Child, PtySize};

/// Extra environment variables for the shells of one tab.
pub type TabEnv = Vec<(OsString, OsString)>;

/// Supplies a new tab's shell environment.
///
/// It is called with the tab's id before the tab's first shell is spawned,
/// so the caller can prepare per-tab resources the shell must know about
/// (the tab's native protocol socket, for example) and hand back the
/// variables that describe them. Panes split off the tab later inherit the
/// same environment.
pub type TabEnvFn<'a> = &'a mut dyn FnMut(TabId) -> io::Result<TabEnv>;

/// A [`TabEnvFn`] for tabs whose shells need no extra environment.
pub fn no_tab_env(_tab_id: TabId) -> io::Result<TabEnv> {
    Ok(Vec::new())
}

/// `env` plus `ULTTY_PANE`, the pane's id as `ulttyctl panes` lists it, so
/// a program in the pane can name its own pane to `ulttyctl` (`--pane`, or
/// the pane `split` puts a new one next to).
fn pane_env(id: TerminalId, env: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    let mut env = env.to_vec();
    env.push(("ULTTY_PANE".into(), id.0.to_string().into()));
    env
}

/// Direction of pane splits within a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitDirection {
    /// Panes arranged side by side (left to right)
    #[default]
    Horizontal,
    /// Panes stacked vertically (top to bottom)
    Vertical,
}

/// Unique identifier for a terminal instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(usize);

/// Last observed outcome of a directly owned child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessJobOutcome {
    /// No exit has been observed by the runtime yet.
    Running,
    /// The child exited and its final output was drained.
    Exited { status: i32 },
    /// The pane was explicitly closed; no completion status is claimed.
    Closed,
}

/// A process receipt retained after its pane disappears. IDs are owner-scoped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessJob {
    /// The pane lifetime ID is also the process job ID.
    pub id: TerminalId,
    /// OS process ID, for observation rather than future addressing.
    pub pid: u32,
    /// Outcome observed by the owner.
    pub outcome: ProcessJobOutcome,
}

impl TerminalId {
    /// Create a new terminal ID.
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> usize {
        self.0
    }
}

/// The pane status a progress report (OSC 9;4) stands for.
pub fn progress_status(
    progress: crate::protocol::ansi::Progress,
) -> Option<ultty_protocol::control::PaneStatus> {
    use crate::protocol::ansi::Progress;
    use ultty_protocol::control::{PaneState, PaneStatus};
    let (state, percent) = match progress {
        Progress::Clear => return None,
        Progress::Normal(percent) => (PaneState::Working, Some(percent)),
        Progress::Indeterminate => (PaneState::Working, None),
        Progress::Error(percent) => (PaneState::Error, percent),
        Progress::Paused(percent) => (PaneState::Waiting, percent),
    };
    Some(PaneStatus {
        state,
        label: String::new(),
        progress: percent.map(|p| f32::from(p) / 100.0),
        attention: false,
    })
}

/// Project the most urgent reported record into existing pane chrome.
/// Ties prefer the latest report; complete records remain available for inspection.
pub fn program_status_projection(
    store: &ultty_vt::program_status::ProgramStatusStore,
) -> Option<ultty_protocol::control::PaneStatus> {
    use ultty_protocol::control::{PaneState, PaneStatus};
    use ultty_vt::program_status::ProgramState;

    let state = |value| match value {
        ProgramState::Idle => PaneState::Idle,
        ProgramState::Working => PaneState::Working,
        ProgramState::Done => PaneState::Done,
        ProgramState::Blocked => PaneState::Waiting,
        ProgramState::Error => PaneState::Error,
    };
    let record = store.records().iter().max_by_key(|record| {
        let urgency = match record.state {
            ProgramState::Error => 4,
            ProgramState::Blocked => 3,
            ProgramState::Working => 2,
            ProgramState::Done => 1,
            ProgramState::Idle => 0,
        };
        (urgency, record.updated)
    })?;
    Some(
        PaneStatus {
            state: state(record.state),
            label: record
                .title
                .as_deref()
                .or(record.msg.as_deref())
                .or_else(|| store.effective_app(&record.id))
                .unwrap_or(&record.id)
                .to_owned(),
            progress: record.progress.map(|value| f32::from(value) / 100.0),
            attention: record.state == ProgramState::Blocked,
        }
        .sanitized(),
    )
}

/// A single terminal instance.
pub struct ManagedTerminal {
    /// Unique ID
    pub id: TerminalId,
    /// The terminal event loop
    pub event_loop: TerminalEventLoop,
    /// Tab ID this terminal belongs to
    pub tab_id: TabId,
    /// Display title
    pub title: String,
    /// What the pane's program said it is doing (`ulttyctl status`).
    pub status: Option<ultty_protocol::control::PaneStatus>,
    /// The status its last progress report (OSC 9;4) implies.
    pub progress: Option<ultty_protocol::control::PaneStatus>,
    /// Cached display projection; full program records belong to the VT state.
    program_status: Option<ultty_protocol::control::PaneStatus>,
    program_status_snapshot: ultty_vt::program_status::ProgramStatusSnapshot,
}

impl Drop for ManagedTerminal {
    fn drop(&mut self) {
        // Its scrollback is freed with it (once the PTY thread lets go).
        crate::heap_trim::request();
    }
}

impl ManagedTerminal {
    /// The status to show for the pane: what was set through the control
    /// socket, else what the program's progress reports imply.
    pub fn shown_status(&self) -> Option<&ultty_protocol::control::PaneStatus> {
        self.status
            .as_ref()
            .or(self.program_status.as_ref())
            .or(self.progress.as_ref())
    }

    /// Status last observed by the runtime event pump, for a consistent control snapshot.
    pub fn observed_program_status(&self) -> &ultty_vt::program_status::ProgramStatusSnapshot {
        &self.program_status_snapshot
    }

    /// Create a new managed terminal whose shell gets `env` on top of the
    /// inherited environment.
    pub fn new(
        id: TerminalId,
        tab_id: TabId,
        cols: u16,
        rows: u16,
        cell_px: Option<(u16, u16)>,
        env: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let child = Child::spawn_shell(PtySize::for_cells(cols, rows, cell_px), &pane_env(id, env))
            .map_err(io::Error::other)?;
        let event_loop = TerminalEventLoop::from_child(child, cols, rows)?;
        Ok(Self {
            id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", id.0),
            status: None,
            progress: None,
            program_status: None,
            program_status_snapshot: ultty_vt::program_status::ProgramStatusStore::default()
                .snapshot(),
        })
    }

    /// Resize this terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.event_loop.resize(cols, rows)
    }

    /// Process events for this terminal.
    pub fn process(&mut self) -> io::Result<Vec<TerminalEvent>> {
        let events = self.event_loop.process()?;
        let terminal = self.event_loop.terminal();
        let store = terminal.program_status();
        if self.program_status_snapshot.generation != store.generation() {
            self.program_status = program_status_projection(store);
            self.program_status_snapshot = store.snapshot();
        }
        Ok(events)
    }

    /// Whether this terminal's exit and final output have been reported.
    pub fn has_exited(&self) -> bool {
        self.event_loop.exit_drained()
    }
}

/// Unique identifier for a window inside one Ultty process (ADR 0027).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowKey(usize);

impl WindowKey {
    /// The initial window created with a fresh [`TerminalManager`].
    pub const PRIMARY: Self = Self(0);

    /// Create a window key from a raw counter value.
    pub const fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw counter value.
    pub const fn raw(&self) -> usize {
        self.0
    }
}

/// Outcome of moving a tab between windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveTabOutcome {
    /// Window the tab was moved out of.
    pub source_window: WindowKey,
    /// Window the tab now belongs to.
    pub destination_window: WindowKey,
    /// Whether `source_window` now has zero remaining tabs.
    pub source_empty: bool,
}

/// Unique identifier for a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(usize);

impl TabId {
    /// Create a new tab ID.
    pub const fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub const fn raw(&self) -> usize {
        self.0
    }
}

/// A tab containing one or more terminal panes.
pub struct Tab {
    /// Unique ID
    pub id: TabId,
    /// The window that currently displays this tab.
    pub window_id: WindowKey,
    /// This tab's panes in layout order, read-only: kept in step with
    /// `layout` by `split` and `remove_terminal`, the only writers.
    pub terminals: Vec<TerminalId>,
    /// Currently focused terminal in this tab
    pub focused_terminal: TerminalId,
    /// Tab title (often the title of the focused terminal)
    pub title: String,
    /// How panes are arranged: a tree of side-by-side and stacked splits.
    pub layout: PaneLayout,
    /// The pane shown at full size without changing the saved split tree.
    pub zoomed_pane: Option<TerminalId>,
    /// Extra environment given to every shell spawned in this tab.
    pub shell_env: TabEnv,
}

impl Tab {
    /// Create a new tab with a single terminal in the primary window.
    pub fn new(id: TabId, terminal_id: TerminalId) -> Self {
        Self::new_in_window(id, terminal_id, WindowKey::PRIMARY)
    }

    /// Create a new tab with a single terminal owned by `window_id`.
    pub fn new_in_window(id: TabId, terminal_id: TerminalId, window_id: WindowKey) -> Self {
        Self {
            id,
            window_id,
            terminals: vec![terminal_id],
            focused_terminal: terminal_id,
            title: format!("Tab {}", id.0 + 1),
            layout: PaneLayout::new(terminal_id),
            zoomed_pane: None,
            shell_env: Vec::new(),
        }
    }

    /// Put `new` next to `target`, side by side or stacked.
    pub fn split(&mut self, target: TerminalId, direction: SplitDirection, new: TerminalId) {
        if self.layout.split(target, direction, new) {
            self.terminals = self.layout.panes();
            self.zoomed_pane = None;
        }
    }

    /// Remove a terminal from this tab.
    pub fn remove_terminal(&mut self, terminal_id: TerminalId) -> bool {
        if let Some(pos) = self.terminals.iter().position(|&id| id == terminal_id) {
            if self.zoomed_pane == Some(terminal_id) {
                self.zoomed_pane = None;
            }
            self.terminals.remove(pos);
            // The layout keeps its last pane; a tab whose last pane goes is
            // empty and gets removed by its owner.
            if self.layout.remove(terminal_id) {
                self.terminals = self.layout.panes();
            }

            // Update focused terminal if needed
            if self.focused_terminal == terminal_id && !self.terminals.is_empty() {
                self.focused_terminal = self.terminals[0];
            }
            true
        } else {
            false
        }
    }

    /// Check if this tab is empty.
    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    /// Visible pane rectangles, shared by rendering, input and sizing.
    pub fn visible_rects(&self, area: Rect) -> Vec<(TerminalId, Rect)> {
        match self.zoomed_pane {
            Some(id) => vec![(id, area)],
            None => self.layout.rects(area),
        }
    }

    fn focus(&mut self, pane: TerminalId) -> bool {
        self.focused_terminal = pane;
        if self.zoomed_pane.is_some_and(|old| old != pane) {
            self.zoomed_pane = Some(pane);
            return true;
        }
        false
    }

    fn default_title(&self) -> String {
        format!("Tab {}", self.id.0 + 1)
    }

    fn has_default_title(&self) -> bool {
        self.title.is_empty() || self.title == self.default_title()
    }
}

/// Manager for multiple terminals organized in tabs.
/// The pixel area a tab's panes share, and the cell size that turns pixels
/// into grid rows and columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneArea {
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl PaneArea {
    /// One cell's size in whole pixels, `(width, height)`, as terminals
    /// report it to applications (XTWINOPS 14 and 16).
    pub fn cell_pixel_size(&self) -> (u16, u16) {
        let px = |v: f32| v.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        (px(self.cell_width), px(self.cell_height))
    }
}

struct WindowRecord {
    id: WindowKey,
    tabs: Vec<TabId>,
    active_tab: Option<TabId>,
    pane_area: Option<PaneArea>,
    default_cols: u16,
    default_rows: u16,
}

impl WindowRecord {
    fn new(id: WindowKey, cols: u16, rows: u16) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active_tab: None,
            pane_area: None,
            default_cols: cols,
            default_rows: rows,
        }
    }
}

pub struct TerminalManager {
    /// All managed terminals
    terminals: Vec<ManagedTerminal>,
    /// Bounded process outcomes, independent of pane/grid retention.
    completed_jobs: VecDeque<ProcessJob>,
    /// All tabs across every window
    tabs: Vec<Tab>,
    /// Per-window tab lists, active tabs, and pane layout areas (ADR 0027)
    windows: Vec<WindowRecord>,
    /// Window currently targeted by window-unqualified operations
    focused_window: WindowKey,
    /// Next window key counter
    next_window_id: usize,
    /// Next terminal ID
    next_terminal_id: usize,
    /// Next tab ID
    next_tab_id: usize,
    /// Default terminal dimensions
    default_cols: u16,
    default_rows: u16,
    /// Handed to every terminal so its I/O thread can wake the UI.
    waker: Option<Waker>,
    /// The colours terminals are drawn with, which they report to
    /// applications. Handed to every terminal, like the waker.
    theme_colors: ThemeColors,
    /// Lines of scrollback each terminal keeps (`behavior.scrollback_lines`).
    scrollback_lines: usize,
    /// Image bytes each screen of a terminal may hold
    /// (`images.storage_limit`).
    image_storage_limit: usize,
    /// Whether a pane closes when its program exits
    /// (`behavior.close_on_exit`). When false, the pane stays, showing its
    /// last output, until it is closed by hand.
    close_on_exit: bool,
    /// Whether a new tab or split starts in the directory of the pane it was
    /// made from (`behavior.inherit_working_directory`).
    inherit_working_directory: bool,
    /// This machine's hostname, to tell a local OSC 7 URI from a remote one.
    hostname: String,
    /// Counts the pane layouts done (`layout_panes`), so a caller can tell
    /// with one compare that pane grids may have changed since it looked.
    layout_generation: u64,
}

impl TerminalManager {
    /// Register the callback every terminal's I/O thread uses to wake the
    /// UI, for existing terminals and any created later.
    pub fn set_waker(&mut self, waker: Waker) {
        for terminal in &self.terminals {
            terminal.event_loop.set_waker(Arc::clone(&waker));
        }
        self.waker = Some(waker);
    }

    /// Set the colours terminals are drawn with, for existing terminals and
    /// any created later, so colour queries report the live theme. Programs
    /// that asked (mode 2031) hear of a switch between dark and light.
    pub fn set_theme_colors(&mut self, theme: ThemeColors) {
        for terminal in &mut self.terminals {
            let report = {
                let mut state = terminal.event_loop.terminal();
                state.set_theme_colors(theme.clone());
                // The I/O thread sends replies as it parses, so anything
                // queued now is the report.
                state.take_responses().concat()
            };
            if !report.is_empty() {
                if let Err(err) = terminal.event_loop.send_input(&report) {
                    tracing::debug!("theme change report not sent: {err}");
                }
            }
        }
        self.theme_colors = theme;
    }

    /// Set how many lines of scrollback terminals keep, for existing
    /// terminals and any created later. Lowering it drops the oldest lines.
    pub fn set_scrollback_lines(&mut self, lines: usize) {
        for terminal in &self.terminals {
            terminal.event_loop.terminal().set_scrollback_limit(lines);
        }
        self.scrollback_lines = lines;
    }

    /// Set how many decoded image bytes each screen of a terminal may hold,
    /// for existing terminals and any created later; 0 turns images off.
    pub fn set_image_storage_limit(&mut self, bytes: usize) {
        for terminal in &self.terminals {
            terminal
                .event_loop
                .terminal()
                .set_image_storage_limit(bytes);
        }
        self.image_storage_limit = bytes;
    }

    /// Set whether a pane closes when its program exits. When false,
    /// [`TerminalManager::cleanup_exited`] keeps exited panes and the window
    /// stays open until every pane has been closed by hand.
    pub fn set_close_on_exit(&mut self, close: bool) {
        self.close_on_exit = close;
    }

    /// Set whether a new tab or split opens in the directory of the pane it
    /// was made from (the default) or in the directory Ultty started in.
    pub fn set_inherit_working_directory(&mut self, inherit: bool) {
        self.inherit_working_directory = inherit;
    }

    /// The directory a new pane made from `source` should start in: the
    /// source's last local OSC 7 directory, else its foreground process's
    /// directory. `None` means Ultty's own directory (or inheriting is off).
    fn inherited_directory(&self, source: Option<TerminalId>) -> Option<String> {
        if !self.inherit_working_directory {
            return None;
        }
        let terminal = self.terminal(source?)?;
        let state = terminal.event_loop.terminal();
        let uri = state.working_directory_uri();
        let dir = crate::inherit_dir::pick(uri, &self.hostname, || {
            terminal
                .event_loop
                .foreground_working_directory()
                .map(std::path::PathBuf::from)
        });
        dir.and_then(|dir| dir.to_str().map(str::to_string))
    }

    /// Whether panes close when their program exits.
    pub fn close_on_exit(&self) -> bool {
        self.close_on_exit
    }

    fn window_record(&self, window: WindowKey) -> Option<&WindowRecord> {
        self.windows.iter().find(|record| record.id == window)
    }

    fn window_record_mut(&mut self, window: WindowKey) -> Option<&mut WindowRecord> {
        self.windows.iter_mut().find(|record| record.id == window)
    }

    fn ensure_window_record(&mut self, window: WindowKey) -> &mut WindowRecord {
        if let Some(pos) = self.windows.iter().position(|record| record.id == window) {
            return &mut self.windows[pos];
        }
        let record = WindowRecord::new(window, self.default_cols, self.default_rows);
        self.windows.push(record);
        self.windows.last_mut().expect("window record just pushed")
    }

    fn push_terminal(&mut self, terminal: ManagedTerminal, window: WindowKey) {
        if let Some(waker) = &self.waker {
            terminal.event_loop.set_waker(Arc::clone(waker));
        }
        let area = self
            .window_record(window)
            .and_then(|record| record.pane_area);
        {
            let mut state = terminal.event_loop.terminal();
            state.set_theme_colors(self.theme_colors.clone());
            state.set_scrollback_limit(self.scrollback_lines);
            state.set_image_storage_limit(self.image_storage_limit);
            if let Some(area) = area {
                let (width, height) = area.cell_pixel_size();
                state.set_cell_pixel_size(width, height);
            }
        }
        self.terminals.push(terminal);
    }

    /// Create a new terminal manager with one tab and one terminal.
    ///
    /// The initial tab's shell gets no extra environment; use
    /// [`TerminalManager::with_tab_env`] when it needs some.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        Self::with_tab_env(cols, rows, &mut no_tab_env)
    }

    /// Create a new terminal manager with one tab whose shell environment
    /// comes from `tab_env`.
    pub fn with_tab_env(cols: u16, rows: u16, tab_env: TabEnvFn) -> io::Result<Self> {
        let mut manager = Self::empty(cols, rows);
        manager.create_tab_with_env(tab_env)?;
        Ok(manager)
    }

    /// Create a new terminal manager whose one tab runs `command` with
    /// `args` instead of the shell, as [`Self::create_tab_with_command`]
    /// does with no working directory or title.
    pub fn with_command_tab(
        cols: u16,
        rows: u16,
        command: &str,
        args: &[String],
        tab_env: TabEnvFn,
    ) -> io::Result<Self> {
        let mut manager = Self::empty(cols, rows);
        manager.create_tab_with_command(command, args, None, None, tab_env)?;
        Ok(manager)
    }

    /// Create a manager with no tabs or child processes.
    pub fn empty(cols: u16, rows: u16) -> Self {
        Self {
            terminals: Vec::new(),
            completed_jobs: VecDeque::new(),
            tabs: Vec::new(),
            windows: vec![WindowRecord::new(WindowKey::PRIMARY, cols, rows)],
            focused_window: WindowKey::PRIMARY,
            next_window_id: 1,
            next_terminal_id: 0,
            next_tab_id: 0,
            default_cols: cols,
            default_rows: rows,
            waker: None,
            theme_colors: ThemeColors::default(),
            scrollback_lines: crate::grid::DEFAULT_SCROLLBACK,
            image_storage_limit: crate::grid::MAX_IMAGE_STORE_BYTES,
            close_on_exit: true,
            inherit_working_directory: true,
            hostname: crate::inherit_dir::hostname(),
            layout_generation: 0,
        }
    }

    /// The primary window created when the manager starts.
    pub fn primary_window(&self) -> WindowKey {
        self.windows
            .first()
            .map(|record| record.id)
            .unwrap_or(WindowKey::PRIMARY)
    }

    /// The currently focused window.
    pub fn focused_window(&self) -> WindowKey {
        self.focused_window
    }

    /// Focus an open window. Returns `false` if `window` is not known.
    pub fn set_focused_window(&mut self, window: WindowKey) -> bool {
        if self.window_record(window).is_none() {
            return false;
        }
        self.focused_window = window;
        true
    }

    /// Open window keys in creation order.
    pub fn window_ids(&self) -> Vec<WindowKey> {
        self.windows.iter().map(|record| record.id).collect()
    }

    /// Number of tracked windows.
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Allocate a new window with one initial tab and focus it.
    pub fn create_window(
        &mut self,
        cols: u16,
        rows: u16,
        tab_env: TabEnvFn,
    ) -> io::Result<(WindowKey, TabId)> {
        self.create_window_in_dir(cols, rows, None, tab_env)
    }

    /// Allocate a new window with one initial shell tab, starting in
    /// `working_dir` when given, and focus it.
    pub fn create_window_in_dir(
        &mut self,
        cols: u16,
        rows: u16,
        working_dir: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<(WindowKey, TabId)> {
        let window_id = WindowKey(self.next_window_id);
        self.next_window_id += 1;
        self.windows.push(WindowRecord::new(window_id, cols, rows));
        match self.create_tab_in_window_with_dir(window_id, working_dir, tab_env) {
            Ok(tab_id) => {
                self.focused_window = window_id;
                Ok((window_id, tab_id))
            }
            Err(err) => {
                self.windows.retain(|record| record.id != window_id);
                Err(err)
            }
        }
    }

    /// Allocate a new window running `command` in its initial tab and focus it.
    #[allow(clippy::too_many_arguments)]
    pub fn create_window_with_command(
        &mut self,
        cols: u16,
        rows: u16,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<(WindowKey, TabId)> {
        let window_id = WindowKey(self.next_window_id);
        self.next_window_id += 1;
        self.windows.push(WindowRecord::new(window_id, cols, rows));
        match self.create_tab_with_command_in_window(
            window_id,
            command,
            args,
            working_dir,
            title,
            tab_env,
        ) {
            Ok(tab_id) => {
                self.focused_window = window_id;
                Ok((window_id, tab_id))
            }
            Err(err) => {
                self.windows.retain(|record| record.id != window_id);
                Err(err)
            }
        }
    }

    /// Close a window and every tab and terminal belonging to it.
    pub fn close_window(&mut self, window: WindowKey) -> bool {
        let Some(pos) = self.windows.iter().position(|record| record.id == window) else {
            return false;
        };
        let record = self.windows.remove(pos);
        for tab_id in record.tabs {
            if let Some(tab_pos) = self.tabs.iter().position(|tab| tab.id == tab_id) {
                let tab = self.tabs.remove(tab_pos);
                for terminal_id in tab.terminals {
                    self.remember_closed_job(terminal_id);
                    self.terminals.retain(|t| t.id != terminal_id);
                }
            }
        }
        if self.focused_window == window {
            self.focused_window = self
                .windows
                .first()
                .map(|record| record.id)
                .unwrap_or(WindowKey::PRIMARY);
        }
        true
    }

    /// The window that owns `tab_id`.
    pub fn window_for_tab(&self, tab_id: TabId) -> Option<WindowKey> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.window_id)
    }

    /// The window that owns `pane`.
    pub fn window_for_pane(&self, pane: TerminalId) -> Option<WindowKey> {
        let tab_id = self.terminal(pane)?.tab_id;
        self.window_for_tab(tab_id)
    }

    /// Tabs owned by `window` in that window's tab-bar order.
    pub fn window_tabs(&self, window: WindowKey) -> Vec<&Tab> {
        let Some(record) = self.window_record(window) else {
            return Vec::new();
        };
        record
            .tabs
            .iter()
            .filter_map(|&tab_id| self.tabs.iter().find(|tab| tab.id == tab_id))
            .collect()
    }

    /// Number of tabs owned by `window`.
    pub fn window_tab_count(&self, window: WindowKey) -> usize {
        self.window_record(window)
            .map(|record| record.tabs.len())
            .unwrap_or(0)
    }

    /// Active tab of every open window.
    pub fn active_tab_ids(&self) -> Vec<TabId> {
        self.windows
            .iter()
            .filter_map(|record| record.active_tab)
            .collect()
    }

    fn is_active_tab(&self, tab_id: TabId) -> bool {
        self.windows
            .iter()
            .any(|record| record.active_tab == Some(tab_id))
    }

    /// Whether `window` has no live terminals remaining.
    pub fn window_is_empty(&self, window: WindowKey) -> bool {
        let Some(record) = self.window_record(window) else {
            return true;
        };
        let mut saw_terminal = false;
        for &tab_id in &record.tabs {
            let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
                continue;
            };
            for &pane in &tab.terminals {
                if let Some(terminal) = self.terminal(pane) {
                    saw_terminal = true;
                    if !self.close_on_exit || !terminal.has_exited() {
                        return false;
                    }
                }
            }
        }
        !saw_terminal || self.close_on_exit
    }

    /// Create a new tab with a new terminal and no extra shell environment.
    pub fn create_tab(&mut self) -> io::Result<TabId> {
        self.create_tab_with_env(&mut no_tab_env)
    }

    /// Create a new tab in the focused window whose shell environment comes
    /// from `tab_env`.
    pub fn create_tab_with_env(&mut self, tab_env: TabEnvFn) -> io::Result<TabId> {
        self.create_tab_in_window(self.focused_window, tab_env)
    }

    /// Create a new tab in `window` whose shell environment comes from `tab_env`.
    pub fn create_tab_in_window(
        &mut self,
        window: WindowKey,
        tab_env: TabEnvFn,
    ) -> io::Result<TabId> {
        self.create_tab_in_window_with_dir(window, None, tab_env)
    }

    /// Create a new tab in `window` whose shell starts in `working_dir` when
    /// given (otherwise inheriting from the focused pane).
    pub fn create_tab_in_window_with_dir(
        &mut self,
        window: WindowKey,
        working_dir: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<TabId> {
        self.ensure_window_record(window);
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let env = tab_env(tab_id)?;
        let source = self
            .window_focused_terminal(window)
            .or_else(|| self.focused_terminal())
            .map(|t| t.id);
        let terminal_id =
            self.create_terminal_with_env_and_dir(window, tab_id, &env, source, working_dir)?;

        let mut tab = Tab::new_in_window(tab_id, terminal_id, window);
        tab.shell_env = env;
        self.tabs.push(tab);
        if let Some(record) = self.window_record_mut(window) {
            record.tabs.push(tab_id);
            record.active_tab = Some(tab_id);
        }
        self.relayout_window(window);

        Ok(tab_id)
    }

    /// Set a tab's explicit display title.
    pub fn set_tab_title(&mut self, tab_id: TabId, title: &str) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };

        tab.title = title.to_string();
        true
    }

    /// Close a tab and all its terminals.
    pub fn close_tab(&mut self, tab_id: TabId) -> bool {
        let Some(tab_idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return false;
        };

        let tab = self.tabs.remove(tab_idx);
        let window_id = tab.window_id;

        for terminal_id in tab.terminals {
            self.remember_closed_job(terminal_id);
            self.terminals.retain(|t| t.id != terminal_id);
        }

        if let Some(record) = self.window_record_mut(window_id) {
            let removed_pos = record.tabs.iter().position(|&id| id == tab_id);
            if let Some(pos) = removed_pos {
                record.tabs.remove(pos);
            }
            if record.active_tab == Some(tab_id) {
                record.active_tab = if record.tabs.is_empty() {
                    None
                } else {
                    let new_idx = removed_pos
                        .unwrap_or(0)
                        .min(record.tabs.len().saturating_sub(1));
                    Some(record.tabs[new_idx])
                };
            }
        }

        true
    }

    /// Reorder tabs in the focused window by moving a tab from one index to another.
    pub fn reorder_tabs(&mut self, from: usize, to: usize) -> bool {
        self.reorder_window_tabs(self.focused_window, from, to)
    }

    /// Reorder tabs owned by `window` by moving a tab from `from` to `to`.
    pub fn reorder_window_tabs(&mut self, window: WindowKey, from: usize, to: usize) -> bool {
        let Some(record) = self.window_record_mut(window) else {
            return false;
        };
        if from >= record.tabs.len() || to >= record.tabs.len() || from == to {
            return false;
        }
        let tab_id = record.tabs.remove(from);
        record.tabs.insert(to, tab_id);

        if self.windows.len() == 1 && from < self.tabs.len() && to < self.tabs.len() {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
        }
        true
    }

    /// Move `tab_id` to an existing `destination` window, preserving its PTYs,
    /// scrollback, and native protocol sessions, and resizing its panes to
    /// `destination`'s pane area.
    pub fn move_tab_to_window(
        &mut self,
        tab_id: TabId,
        destination: WindowKey,
    ) -> Result<MoveTabOutcome, &'static str> {
        let Some(tab_pos) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return Err("Tab not found");
        };
        let source_window = self.tabs[tab_pos].window_id;
        if self.window_record(destination).is_none() {
            return Err("Destination window not found");
        }
        if source_window == destination {
            if let Some(record) = self.window_record_mut(destination) {
                record.active_tab = Some(tab_id);
            }
            self.focused_window = destination;
            return Ok(MoveTabOutcome {
                source_window,
                destination_window: destination,
                source_empty: false,
            });
        }

        let mut source_empty = false;
        if let Some(source) = self.window_record_mut(source_window) {
            let removed_pos = source.tabs.iter().position(|&id| id == tab_id);
            if let Some(pos) = removed_pos {
                source.tabs.remove(pos);
            }
            if source.active_tab == Some(tab_id) {
                source.active_tab = if source.tabs.is_empty() {
                    None
                } else {
                    let next = removed_pos
                        .unwrap_or(0)
                        .min(source.tabs.len().saturating_sub(1));
                    Some(source.tabs[next])
                };
            }
            source_empty = source.tabs.is_empty();
        }

        self.tabs[tab_pos].window_id = destination;
        if let Some(dest) = self.window_record_mut(destination) {
            dest.tabs.push(tab_id);
            dest.active_tab = Some(tab_id);
        }
        self.focused_window = destination;
        self.relayout_window(destination);

        Ok(MoveTabOutcome {
            source_window,
            destination_window: destination,
            source_empty,
        })
    }

    /// Allocate a new window record and move `tab_id` into it.
    ///
    /// Refuses when `tab_id` is the only tab of its source window.
    pub fn detach_tab_to_new_window(
        &mut self,
        tab_id: TabId,
        cols: u16,
        rows: u16,
    ) -> Result<(WindowKey, MoveTabOutcome), &'static str> {
        let Some(source) = self.window_for_tab(tab_id) else {
            return Err("Tab not found");
        };
        if self.window_tab_count(source) <= 1 {
            return Err("Cannot move the only tab of a window to a new window");
        }
        let destination = WindowKey(self.next_window_id);
        self.next_window_id += 1;
        self.windows
            .push(WindowRecord::new(destination, cols, rows));
        match self.move_tab_to_window(tab_id, destination) {
            Ok(outcome) => Ok((destination, outcome)),
            Err(err) => {
                self.windows.retain(|record| record.id != destination);
                Err(err)
            }
        }
    }

    /// Create a new terminal (pane) in an existing tab, inheriting the
    /// tab's shell environment.
    fn create_terminal(&mut self, tab_id: TabId, source: TerminalId) -> io::Result<TerminalId> {
        let (window, env) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| (tab.window_id, tab.shell_env.clone()))
            .unwrap_or_else(|| (self.focused_window, Vec::new()));
        self.create_terminal_with_env(window, tab_id, &env, Some(source))
    }

    /// Create a new terminal in `tab_id` whose shell gets `env`, starting in
    /// the directory of `source` when there is one to inherit.
    fn create_terminal_with_env(
        &mut self,
        window: WindowKey,
        tab_id: TabId,
        env: &[(OsString, OsString)],
        source: Option<TerminalId>,
    ) -> io::Result<TerminalId> {
        self.create_terminal_with_env_and_dir(window, tab_id, env, source, None)
    }

    fn create_terminal_with_env_and_dir(
        &mut self,
        window: WindowKey,
        tab_id: TabId,
        env: &[(OsString, OsString)],
        source: Option<TerminalId>,
        explicit_dir: Option<&str>,
    ) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        let (cols, rows, cell_px) = self
            .window_record(window)
            .map(|record| {
                (
                    record.default_cols,
                    record.default_rows,
                    record.pane_area.map(|area| area.cell_pixel_size()),
                )
            })
            .unwrap_or((self.default_cols, self.default_rows, None));
        let chosen_dir = explicit_dir
            .map(str::to_owned)
            .or_else(|| self.inherited_directory(source));
        let terminal = match chosen_dir {
            Some(dir) => {
                ManagedTerminal::new_with_dir(terminal_id, tab_id, cols, rows, cell_px, &dir, env)?
            }
            None => ManagedTerminal::new(terminal_id, tab_id, cols, rows, cell_px, env)?,
        };

        self.push_terminal(terminal, window);
        Ok(terminal_id)
    }

    /// Get the active tab of `window`.
    pub fn window_active_tab(&self, window: WindowKey) -> Option<&Tab> {
        let active = self.window_record(window)?.active_tab?;
        self.tabs.iter().find(|t| t.id == active)
    }

    /// Get the active tab of `window` mutably.
    pub fn window_active_tab_mut(&mut self, window: WindowKey) -> Option<&mut Tab> {
        let active = self.window_record(window)?.active_tab?;
        self.tabs.iter_mut().find(|t| t.id == active)
    }

    /// Get the active tab of the focused window.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.window_active_tab(self.focused_window)
    }

    /// Get the active tab of the focused window mutably.
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.window_active_tab_mut(self.focused_window)
    }

    /// Get the currently focused terminal of `window`.
    pub fn window_focused_terminal(&self, window: WindowKey) -> Option<&ManagedTerminal> {
        let tab = self.window_active_tab(window)?;
        self.terminals.iter().find(|t| t.id == tab.focused_terminal)
    }

    /// Get the currently focused terminal of `window` mutably.
    pub fn window_focused_terminal_mut(
        &mut self,
        window: WindowKey,
    ) -> Option<&mut ManagedTerminal> {
        let focused_id = self.window_active_tab(window)?.focused_terminal;
        self.terminals.iter_mut().find(|t| t.id == focused_id)
    }

    /// Get the focused terminal ID in `window`'s active tab.
    pub fn window_focused_terminal_id(&self, window: WindowKey) -> Option<TerminalId> {
        self.window_active_tab(window).map(|t| t.focused_terminal)
    }

    /// Get the currently focused terminal.
    pub fn focused_terminal(&self) -> Option<&ManagedTerminal> {
        self.window_focused_terminal(self.focused_window)
    }

    /// Get the currently focused terminal mutably.
    pub fn focused_terminal_mut(&mut self) -> Option<&mut ManagedTerminal> {
        self.window_focused_terminal_mut(self.focused_window)
    }

    /// Get a terminal by ID.
    pub fn terminal(&self, id: TerminalId) -> Option<&ManagedTerminal> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// Get a terminal by ID mutably.
    pub fn terminal_mut(&mut self, id: TerminalId) -> Option<&mut ManagedTerminal> {
        self.terminals.iter_mut().find(|t| t.id == id)
    }

    /// Clear dirty flags on all terminals after rendering.
    pub fn clear_dirty(&mut self) {
        for terminal in &mut self.terminals {
            terminal.event_loop.terminal_mut().clear_dirty();
        }
    }

    /// Clear dirty flags only on terminals shown in `window`'s active tab.
    pub fn clear_window_active_dirty(&mut self, window: WindowKey) {
        let Some(panes) = self
            .window_active_tab(window)
            .map(|tab| tab.terminals.clone())
        else {
            return;
        };
        for pane in panes {
            if let Some(terminal) = self.terminal_mut(pane) {
                terminal.event_loop.terminal_mut().clear_dirty();
            }
        }
    }

    /// Get all tabs across every window.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Get the display title for a tab.
    pub fn tab_display_title(&self, tab_id: TabId) -> String {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return String::new();
        };

        if !tab.has_default_title() {
            return tab.title.clone();
        }

        let Some(terminal) = self.terminal(tab.focused_terminal) else {
            return tab.default_title();
        };

        if self.is_active_tab(tab.id) {
            if let Some(app) = terminal.event_loop.foreground_process_name() {
                return app;
            }
        }

        let terminal_state = terminal.event_loop.terminal();
        let title = terminal_state.title().trim();
        if !title.is_empty() && !is_shell_label(title) {
            return title.to_string();
        }

        let cwd = terminal_state
            .working_directory()
            .map(str::to_string)
            .or_else(|| terminal.event_loop.child_working_directory());

        if let Some(cwd) = cwd.and_then(|path| compact_working_directory(&path)) {
            return cwd;
        }

        if !title.is_empty() {
            return title.to_string();
        }

        tab.default_title()
    }

    /// Get display titles for every tab in order.
    pub fn tab_display_titles(&self) -> Vec<String> {
        self.tabs
            .iter()
            .map(|tab| self.tab_display_title(tab.id))
            .collect()
    }

    /// Get display titles for every tab owned by `window`, in tab-bar order.
    pub fn window_tab_display_titles(&self, window: WindowKey) -> Vec<String> {
        self.window_tabs(window)
            .into_iter()
            .map(|tab| self.tab_display_title(tab.id))
            .collect()
    }

    /// Get number of tabs across all windows.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Switch to the next tab in `window`.
    pub fn next_tab_in_window(&mut self, window: WindowKey) {
        let Some(record) = self.window_record_mut(window) else {
            return;
        };
        if record.tabs.is_empty() {
            return;
        }
        let current_idx = record
            .active_tab
            .and_then(|active| record.tabs.iter().position(|&id| id == active))
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % record.tabs.len();
        record.active_tab = Some(record.tabs[next_idx]);
    }

    /// Switch to the previous tab in `window`.
    pub fn prev_tab_in_window(&mut self, window: WindowKey) {
        let Some(record) = self.window_record_mut(window) else {
            return;
        };
        if record.tabs.is_empty() {
            return;
        }
        let current_idx = record
            .active_tab
            .and_then(|active| record.tabs.iter().position(|&id| id == active))
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 {
            record.tabs.len() - 1
        } else {
            current_idx - 1
        };
        record.active_tab = Some(record.tabs[prev_idx]);
    }

    /// Switch to a specific tab by index within `window`.
    pub fn switch_to_tab_in_window(&mut self, window: WindowKey, index: usize) {
        if let Some(record) = self.window_record_mut(window) {
            if index < record.tabs.len() {
                record.active_tab = Some(record.tabs[index]);
            }
        }
    }

    /// Switch to a specific tab by ID within its owning window.
    pub fn switch_to_tab_id(&mut self, tab_id: TabId) -> bool {
        let Some(window) = self.window_for_tab(tab_id) else {
            return false;
        };
        if let Some(record) = self.window_record_mut(window) {
            record.active_tab = Some(tab_id);
            return true;
        }
        false
    }

    /// Switch to the next tab.
    pub fn next_tab(&mut self) {
        self.next_tab_in_window(self.focused_window);
    }

    /// Switch to the previous tab.
    pub fn prev_tab(&mut self) {
        self.prev_tab_in_window(self.focused_window);
    }

    /// Switch to a specific tab by index.
    pub fn switch_to_tab(&mut self, index: usize) {
        self.switch_to_tab_in_window(self.focused_window, index);
    }

    /// Size every pane's terminal in `window` to its share of `area` and
    /// remember `area` for later splits, closes, and divider drags in that
    /// window. Unchanged grids avoid reflow; pixel metrics still reach the PTY.
    pub fn layout_window_panes(&mut self, window: WindowKey, area: PaneArea) -> io::Result<()> {
        self.ensure_window_record(window);
        self.layout_generation = self.layout_generation.wrapping_add(1);
        let (cols, rows) = crate::geometry::grid_dimensions_for(
            area.width,
            area.height,
            area.cell_width,
            area.cell_height,
        );
        if let Some(record) = self.window_record_mut(window) {
            record.pane_area = Some(area);
            record.default_cols = cols;
            record.default_rows = rows;
        }
        if window == self.focused_window {
            self.default_cols = cols;
            self.default_rows = rows;
        }

        let mut sizes = Vec::new();
        for tab in self.tabs.iter().filter(|tab| tab.window_id == window) {
            for (terminal_id, (_, _, width, height)) in
                tab.visible_rects((0.0, 0.0, area.width, area.height))
            {
                let grid = crate::geometry::grid_dimensions_for(
                    width,
                    height,
                    area.cell_width,
                    area.cell_height,
                );
                sizes.push((terminal_id, grid));
            }
        }
        let (cell_width, cell_height) = area.cell_pixel_size();
        for (terminal_id, (cols, rows)) in sizes {
            let Some(terminal) = self.terminals.iter_mut().find(|t| t.id == terminal_id) else {
                continue;
            };
            let (current, cell_changed) = {
                let mut state = terminal.event_loop.terminal();
                let cell_changed = state.cell_pixel_size() != Some((cell_width, cell_height));
                state.set_cell_pixel_size(cell_width, cell_height);
                (state.dimensions(), cell_changed)
            };
            if cell_changed || (current.cols, current.rows) != (cols, rows) {
                terminal.resize(cols, rows)?;
            }
        }
        Ok(())
    }

    /// Size every pane's terminal in the focused window to its share of `area`.
    pub fn layout_panes(&mut self, area: PaneArea) -> io::Result<()> {
        self.layout_window_panes(self.focused_window, area)
    }

    /// How many times panes have been laid out; see `layout_generation`.
    pub fn layout_generation(&self) -> u64 {
        self.layout_generation
    }

    /// The grid of `pane`, `(cols, rows)`. Locks the pane's terminal
    /// briefly.
    pub fn pane_grid_size(&self, pane: TerminalId) -> Option<(u16, u16)> {
        let terminal = self.terminal(pane)?;
        let dims = terminal.event_loop.terminal().dimensions();
        Some((dims.cols, dims.rows))
    }

    /// The pane of `tab_id` a program in process group `pgrp` runs in
    /// (ADR 0025 §1): a tab's only pane whatever the group, else the pane
    /// whose PTY has `pgrp` in the foreground. `None` for a group in no
    /// pane's foreground (a background job) or an unknown tab.
    pub fn pane_for_process_group(&self, tab_id: TabId, pgrp: Option<i32>) -> Option<TerminalId> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if let [only] = tab.terminals.as_slice() {
            return Some(*only);
        }
        let pgrp = pgrp?;
        tab.terminals.iter().copied().find(|&id| {
            self.terminal(id)
                .and_then(|t| t.event_loop.foreground_pgrp())
                == Some(pgrp)
        })
    }

    /// Lay `window`'s panes out again after their number or ratios changed.
    fn relayout_window(&mut self, window: WindowKey) {
        if let Some(area) = self
            .window_record(window)
            .and_then(|record| record.pane_area)
        {
            if let Err(e) = self.layout_window_panes(window, area) {
                tracing::warn!("Failed to resize panes: {}", e);
            }
        }
    }

    /// Lay every window's panes out again using each window's own pane area.
    fn relayout(&mut self) {
        let areas: Vec<(WindowKey, PaneArea)> = self
            .windows
            .iter()
            .filter_map(|record| record.pane_area.map(|area| (record.id, area)))
            .collect();
        for (window, area) in areas {
            if let Err(e) = self.layout_window_panes(window, area) {
                tracing::warn!("Failed to resize panes for window {}: {}", window.raw(), e);
            }
        }
    }

    pub fn resize_all(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.default_cols = cols;
        self.default_rows = rows;

        for terminal in &mut self.terminals {
            terminal.resize(cols, rows)?;
        }
        Ok(())
    }

    /// Process events for all terminals.
    ///
    /// Returns events keyed by terminal ID.
    pub fn process_all(&mut self) -> io::Result<Vec<(TerminalId, Vec<TerminalEvent>)>> {
        let mut all_events = Vec::new();
        let mut completed = Vec::new();

        for terminal in &mut self.terminals {
            let events = terminal.process()?;
            for event in &events {
                if let TerminalEvent::ChildExited { status } = event {
                    completed.push(ProcessJob {
                        id: terminal.id,
                        pid: terminal.event_loop.child_pid(),
                        outcome: ProcessJobOutcome::Exited { status: *status },
                    });
                }
            }
            if !self.close_on_exit {
                for event in &events {
                    if let TerminalEvent::ChildExited { status } = event {
                        // The pane stays open; say why nothing answers.
                        let note =
                            format!("\r\n\x1b[0;7m[process exited with status {status}]\x1b[0m");
                        terminal.event_loop.write_local(note.as_bytes());
                    }
                }
            }
            if !events.is_empty() {
                if let Some(title) = events.iter().rev().find_map(|event| match event {
                    TerminalEvent::TitleChanged(title) => Some(title),
                    _ => None,
                }) {
                    terminal.title = title.clone();
                }

                all_events.push((terminal.id, events));
            }
        }

        for job in completed {
            self.remember_job(job);
        }
        Ok(all_events)
    }

    /// Query a running or recently completed process, even after pane cleanup.
    /// Returns `None` for unknown IDs or results older than the 256-entry window.
    pub fn job(&self, id: TerminalId) -> Option<ProcessJob> {
        if let Some(job) = self.completed_jobs.iter().find(|job| job.id == id) {
            return Some(*job);
        }
        self.terminal(id).map(|terminal| ProcessJob {
            id,
            pid: terminal.event_loop.child_pid(),
            outcome: terminal
                .has_exited()
                .then(|| terminal.event_loop.exit_status())
                .flatten()
                .map_or(ProcessJobOutcome::Running, |status| {
                    ProcessJobOutcome::Exited { status }
                }),
        })
    }

    fn remember_job(&mut self, job: ProcessJob) {
        if self.completed_jobs.iter().any(|old| old.id == job.id) {
            return;
        }
        if self.completed_jobs.len() == 256 {
            self.completed_jobs.pop_front();
        }
        self.completed_jobs.push_back(job);
    }

    fn remember_closed_job(&mut self, id: TerminalId) {
        if let Some(terminal) = self.terminal(id) {
            self.remember_job(ProcessJob {
                id,
                pid: terminal.event_loop.child_pid(),
                outcome: ProcessJobOutcome::Closed,
            });
        }
    }

    /// Remove exited terminals and their empty tabs, unless panes are set
    /// to stay open after their program exits.
    pub fn cleanup_exited(&mut self) {
        if !self.close_on_exit {
            return;
        }
        // Find exited terminals
        let exited: Vec<TerminalId> = self
            .terminals
            .iter()
            .filter(|t| t.has_exited())
            .map(|t| t.id)
            .collect();

        for &terminal_id in &exited {
            // Find and update the tab
            if let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|t| t.terminals.contains(&terminal_id))
            {
                tab.remove_terminal(terminal_id);
            }

            // Remove the terminal
            self.terminals.retain(|t| t.id != terminal_id);
        }

        let multi_window = self.windows.len() > 1;
        let mut drop_tabs = Vec::new();
        for record in &mut self.windows {
            let allow_empty_removal = multi_window || record.tabs.len() > 1;
            if allow_empty_removal {
                record.tabs.retain(|&tab_id| {
                    let keep = self
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .is_some_and(|tab| !tab.is_empty());
                    if !keep {
                        drop_tabs.push(tab_id);
                    }
                    keep
                });
            }
        }
        if !drop_tabs.is_empty() {
            self.tabs.retain(|tab| !drop_tabs.contains(&tab.id));
        }

        self.repair_active_tab();
        if !exited.is_empty() {
            self.relayout();
        }
    }

    fn repair_active_tab(&mut self) {
        for record in &mut self.windows {
            if record
                .active_tab
                .is_some_and(|active| record.tabs.contains(&active))
            {
                continue;
            }
            record.active_tab = record.tabs.first().copied();
        }
    }

    /// Whether the window has nothing left to show: no terminals, or, when
    /// panes close on exit, only terminals whose program has exited.
    pub fn all_exited(&self) -> bool {
        self.terminals.is_empty()
            || (self.close_on_exit && self.terminals.iter().all(|t| t.has_exited()))
    }

    /// Split the focused pane side by side.
    pub fn split_horizontal(&mut self) -> io::Result<Option<TerminalId>> {
        self.split_focused(SplitDirection::Horizontal)
    }

    /// Split the focused pane top and bottom.
    pub fn split_vertical(&mut self) -> io::Result<Option<TerminalId>> {
        self.split_focused(SplitDirection::Vertical)
    }

    /// Split the focused pane of `window` side by side or stacked.
    pub fn split_window_focused(
        &mut self,
        window: WindowKey,
        direction: SplitDirection,
    ) -> io::Result<Option<TerminalId>> {
        let Some(target) = self.window_focused_terminal_id(window) else {
            return Ok(None);
        };
        self.split_pane(target, direction)
    }

    /// Put a new pane next to the focused one. Splitting along the focused
    /// pane's own split adds a sibling; splitting across it nests.
    fn split_focused(&mut self, direction: SplitDirection) -> io::Result<Option<TerminalId>> {
        self.split_window_focused(self.focused_window, direction)
    }

    /// Put a new shell pane next to `target`, in whatever tab holds it.
    /// `None` when no tab does.
    pub fn split_pane(
        &mut self,
        target: TerminalId,
        direction: SplitDirection,
    ) -> io::Result<Option<TerminalId>> {
        self.split_pane_with(target, direction, |this, tab_id| {
            this.create_terminal(tab_id, target)
        })
    }

    /// Like [`Self::split_pane`], the new pane running `command` (with the
    /// tab's shell environment) instead of the shell.
    pub fn split_pane_with_command(
        &mut self,
        target: TerminalId,
        direction: SplitDirection,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
    ) -> io::Result<Option<TerminalId>> {
        self.split_pane_with(target, direction, |this, tab_id| {
            let env = this
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .map(|tab| tab.shell_env.clone())
                .unwrap_or_default();
            this.create_terminal_with_command(tab_id, command, args, working_dir, &env)
        })
    }

    fn split_pane_with(
        &mut self,
        target: TerminalId,
        direction: SplitDirection,
        spawn: impl FnOnce(&mut Self, TabId) -> io::Result<TerminalId>,
    ) -> io::Result<Option<TerminalId>> {
        let Some((tab_id, window_id)) = self
            .terminal(target)
            .map(|t| t.tab_id)
            .and_then(|tab_id| Some((tab_id, self.window_for_tab(tab_id)?)))
        else {
            return Ok(None);
        };
        let terminal_id = spawn(self, tab_id)?;
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.split(target, direction, terminal_id);
        }
        self.relayout_window(window_id);
        Ok(Some(terminal_id))
    }

    /// [`Terminal::uid`] of every open pane, in every tab. Per-terminal GPU
    /// caches keep only these.
    pub fn terminal_uids(&self) -> std::collections::HashSet<u64> {
        self.terminals
            .iter()
            .map(|t| t.event_loop.terminal_uid())
            .collect()
    }

    /// Number of panes in `window`'s active tab.
    pub fn window_pane_count(&self, window: WindowKey) -> usize {
        self.window_active_tab(window)
            .map(|t| t.terminals.len())
            .unwrap_or(0)
    }

    pub fn pane_count(&self) -> usize {
        self.window_pane_count(self.focused_window)
    }

    /// `window`'s active tab panes and their rectangles within `width` x
    /// `height`, in layout order.
    pub fn window_active_pane_rects(
        &self,
        window: WindowKey,
        width: f32,
        height: f32,
    ) -> Vec<(TerminalId, Rect)> {
        self.window_active_tab(window)
            .map(|tab| tab.visible_rects((0.0, 0.0, width, height)))
            .unwrap_or_default()
    }

    /// The active tab's panes and their rectangles within a `width` x
    /// `height` area, in layout order.
    pub fn active_pane_rects(&self, width: f32, height: f32) -> Vec<(TerminalId, Rect)> {
        self.window_active_pane_rects(self.focused_window, width, height)
    }

    /// Move focus to the nearest pane on one side of the focused one in `window`.
    pub fn window_focus_direction(
        &mut self,
        window: WindowKey,
        direction: FocusDirection,
        width: f32,
        height: f32,
    ) {
        let Some(tab) = self.window_active_tab_mut(window) else {
            return;
        };
        if let Some(next) =
            tab.layout
                .neighbor((0.0, 0.0, width, height), tab.focused_terminal, direction)
        {
            if tab.focus(next) {
                self.relayout_window(window);
            }
        }
    }

    /// Move focus to the nearest pane on one side of the focused one.
    pub fn focus_direction(&mut self, direction: FocusDirection, width: f32, height: f32) {
        self.window_focus_direction(self.focused_window, direction, width, height);
    }

    /// Focus the next pane in `window`'s active tab.
    pub fn window_focus_next_pane(&mut self, window: WindowKey) {
        let Some(tab) = self.window_active_tab_mut(window) else {
            return;
        };

        if tab.terminals.len() <= 1 {
            return;
        }

        let current_idx = tab
            .terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % tab.terminals.len();
        if tab.focus(tab.terminals[next_idx]) {
            self.relayout_window(window);
        }
    }

    /// Focus the next pane in the active tab.
    pub fn focus_next_pane(&mut self) {
        self.window_focus_next_pane(self.focused_window);
    }

    /// Focus the previous pane in `window`'s active tab.
    pub fn window_focus_prev_pane(&mut self, window: WindowKey) {
        let Some(tab) = self.window_active_tab_mut(window) else {
            return;
        };

        if tab.terminals.len() <= 1 {
            return;
        }

        let current_idx = tab
            .terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            tab.terminals.len() - 1
        } else {
            current_idx - 1
        };
        if tab.focus(tab.terminals[prev_idx]) {
            self.relayout_window(window);
        }
    }

    /// Focus the previous pane in the active tab.
    pub fn focus_prev_pane(&mut self) {
        self.window_focus_prev_pane(self.focused_window);
    }

    /// Close the currently focused pane in `window`.
    pub fn close_window_focused_pane(&mut self, window: WindowKey) -> bool {
        let Some(tab) = self.window_active_tab(window) else {
            return false;
        };

        if tab.terminals.len() <= 1 {
            return false;
        }

        let focused_id = tab.focused_terminal;
        let tab_id = tab.id;

        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.remove_terminal(focused_id);
        }

        self.remember_closed_job(focused_id);
        self.terminals.retain(|t| t.id != focused_id);
        self.relayout_window(window);

        true
    }

    /// Close the currently focused pane.
    ///
    /// Returns true if a pane was closed, false if it was the last pane.
    pub fn close_focused_pane(&mut self) -> bool {
        self.close_window_focused_pane(self.focused_window)
    }

    /// Get all terminals in `window`'s active tab for rendering.
    pub fn window_active_tab_terminals(&self, window: WindowKey) -> Vec<&ManagedTerminal> {
        let Some(tab) = self.window_active_tab(window) else {
            return Vec::new();
        };

        tab.terminals
            .iter()
            .filter_map(|&id| self.terminal(id))
            .collect()
    }

    /// Get all terminals in the active tab for rendering.
    pub fn active_tab_terminals(&self) -> Vec<&ManagedTerminal> {
        self.window_active_tab_terminals(self.focused_window)
    }

    /// Whether `id` is shown on screen in `window`.
    pub fn is_on_screen_in_window(&self, window: WindowKey, id: TerminalId) -> bool {
        self.window_active_tab(window).is_some_and(|tab| {
            tab.terminals.contains(&id) && tab.zoomed_pane.is_none_or(|zoomed| zoomed == id)
        })
    }

    /// Whether the pane is shown in any open window's active tab.
    pub fn is_on_screen(&self, id: TerminalId) -> bool {
        self.windows
            .iter()
            .any(|record| self.is_on_screen_in_window(record.id, id))
    }

    /// Get the focused terminal ID in the active tab.
    pub fn focused_terminal_id(&self) -> Option<TerminalId> {
        self.window_focused_terminal_id(self.focused_window)
    }

    /// Create a new tab in `window` running a custom command, with the shell
    /// environment supplied by `tab_env`.
    pub fn create_tab_with_command_in_window(
        &mut self,
        window: WindowKey,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<TabId> {
        self.ensure_window_record(window);
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let env = tab_env(tab_id)?;
        let terminal_id = self.create_terminal_with_command_in_window(
            window,
            tab_id,
            command,
            args,
            working_dir,
            &env,
        )?;

        let mut tab = Tab::new_in_window(tab_id, terminal_id, window);
        tab.shell_env = env;
        if let Some(title) = title {
            tab.title = title.to_string();
        }

        self.tabs.push(tab);
        if let Some(record) = self.window_record_mut(window) {
            record.tabs.push(tab_id);
            record.active_tab = Some(tab_id);
        }

        if let Some(terminal) = self.terminal_mut(terminal_id) {
            if let Some(title) = title {
                terminal.title = title.to_string();
            }
        }

        self.relayout_window(window);
        Ok(tab_id)
    }

    /// Create a new tab running a custom command, with the shell
    /// environment supplied by `tab_env`.
    pub fn create_tab_with_command(
        &mut self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_env: TabEnvFn,
    ) -> io::Result<TabId> {
        self.create_tab_with_command_in_window(
            self.focused_window,
            command,
            args,
            working_dir,
            title,
            tab_env,
        )
    }

    /// Create a terminal with a custom command.
    fn create_terminal_with_command(
        &mut self,
        tab_id: TabId,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        env: &[(OsString, OsString)],
    ) -> io::Result<TerminalId> {
        let window = self.window_for_tab(tab_id).unwrap_or(self.focused_window);
        self.create_terminal_with_command_in_window(window, tab_id, command, args, working_dir, env)
    }

    fn create_terminal_with_command_in_window(
        &mut self,
        window: WindowKey,
        tab_id: TabId,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        env: &[(OsString, OsString)],
    ) -> io::Result<TerminalId> {
        let terminal_id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;

        let mut full_args: Vec<&str> = vec![command];
        for arg in args {
            full_args.push(arg);
        }
        let injected = crate::pty::shell_integration::spawn_plan(command, &full_args, env);
        let (full_args, env): (Vec<&str>, &[(OsString, OsString)]) = match &injected {
            Some((injected_args, injected_env)) => (
                injected_args.iter().map(String::as_str).collect(),
                injected_env,
            ),
            None => (full_args, env),
        };

        let (cols, rows, cell_px) = self
            .window_record(window)
            .map(|record| {
                (
                    record.default_cols,
                    record.default_rows,
                    record.pane_area.map(|area| area.cell_pixel_size()),
                )
            })
            .unwrap_or((self.default_cols, self.default_rows, None));
        let dir = working_dir
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::pty::current_dir);
        let child = Child::spawn_checked(
            command,
            &full_args,
            &dir,
            PtySize::for_cells(cols, rows, cell_px),
            &pane_env(terminal_id, env),
        )
        .map_err(io::Error::other)?;
        let event_loop = TerminalEventLoop::from_child(child, cols, rows)?;

        let terminal = ManagedTerminal {
            id: terminal_id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", terminal_id.0),
            status: None,
            progress: None,
            program_status: None,
            program_status_snapshot: ultty_vt::program_status::ProgramStatusStore::default()
                .snapshot(),
        };

        self.push_terminal(terminal, window);
        Ok(terminal_id)
    }

    /// Focus a pane of `window`'s active tab by its terminal ID.
    pub fn window_focus_terminal(&mut self, window: WindowKey, terminal_id: TerminalId) {
        if let Some(tab) = self.window_active_tab_mut(window) {
            if tab.terminals.contains(&terminal_id) && tab.focus(terminal_id) {
                self.relayout_window(window);
            }
        }
    }

    /// Focus a pane of the active tab by its terminal ID.
    pub fn focus_terminal(&mut self, terminal_id: TerminalId) {
        self.window_focus_terminal(self.focused_window, terminal_id);
    }

    /// Focus an existing pane, switching to its tab and window when needed.
    pub fn focus_pane_by_id(&mut self, terminal_id: TerminalId) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.terminals.contains(&terminal_id))
        else {
            return false;
        };
        let resized = tab.focus(terminal_id);
        let tab_id = tab.id;
        let window_id = tab.window_id;
        if let Some(record) = self.window_record_mut(window_id) {
            record.active_tab = Some(tab_id);
        }
        self.focused_window = window_id;
        if resized {
            self.relayout_window(window_id);
        }
        true
    }

    /// Set a pane's zoom state without modifying its saved layout or siblings.
    pub fn set_pane_zoom(&mut self, terminal_id: TerminalId, zoomed: bool) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.terminals.contains(&terminal_id))
        else {
            return false;
        };
        let window_id = tab.window_id;
        if zoomed {
            tab.focused_terminal = terminal_id;
            tab.zoomed_pane = Some(terminal_id);
        } else if tab.zoomed_pane == Some(terminal_id) {
            tab.zoomed_pane = None;
        }
        self.relayout_window(window_id);
        true
    }

    /// Swap two panes in one tab while preserving their IDs and children.
    pub fn swap_panes(&mut self, first: TerminalId, second: TerminalId) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.terminals.contains(&first))
        else {
            return false;
        };
        let window_id = tab.window_id;
        if !tab.layout.swap(first, second) {
            return false;
        }
        tab.terminals = tab.layout.panes();
        self.relayout_window(window_id);
        true
    }

    /// Focus a specific pane by index.
    pub fn focus_pane(&mut self, index: usize) {
        let window = self.focused_window;
        let Some(tab) = self.window_active_tab_mut(window) else {
            return;
        };

        if index < tab.terminals.len() && tab.focus(tab.terminals[index]) {
            self.relayout_window(window);
        }
    }

    /// Get the index of the focused pane.
    pub fn focused_pane_index(&self) -> usize {
        let Some(tab) = self.active_tab() else {
            return 0;
        };

        tab.terminals
            .iter()
            .position(|&id| id == tab.focused_terminal)
            .unwrap_or(0)
    }

    /// The pane at (x, y) in `window`'s `width` x `height` content area.
    pub fn window_pane_at(
        &self,
        window: WindowKey,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Option<TerminalId> {
        self.window_active_pane_rects(window, width, height)
            .into_iter()
            .find(|(_, (px, py, pw, ph))| x >= *px && x < px + pw && y >= *py && y < py + ph)
            .map(|(id, _)| id)
    }

    /// The pane at (x, y) in a `width` x `height` content area.
    pub fn pane_at(&self, x: f32, y: f32, width: f32, height: f32) -> Option<TerminalId> {
        self.window_pane_at(self.focused_window, x, y, width, height)
    }

    /// The divider within `tolerance` pixels of (x, y) in `window`, if any.
    pub fn window_divider_at(
        &self,
        window: WindowKey,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        tolerance: f32,
    ) -> Option<Divider> {
        let tab = self.window_active_tab(window)?;
        if tab.zoomed_pane.is_some() {
            return None;
        }
        tab.layout
            .divider_at((0.0, 0.0, width, height), x, y, tolerance)
    }

    /// The divider within `tolerance` pixels of (x, y), if any.
    pub fn divider_at(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        tolerance: f32,
    ) -> Option<Divider> {
        self.window_divider_at(self.focused_window, x, y, width, height, tolerance)
    }

    /// Move a divider of `window`'s active tab by `delta` pixels and resize
    /// the panes on either side.
    pub fn window_drag_divider(&mut self, window: WindowKey, divider: &Divider, delta: f32) {
        if let Some(tab) = self.window_active_tab_mut(window) {
            if tab.zoomed_pane.is_some() {
                return;
            }
            tab.layout.drag(divider, delta);
        }
        self.relayout_window(window);
    }

    /// Move a divider of the active tab by `delta` pixels and resize the
    /// panes on either side.
    pub fn drag_divider(&mut self, divider: &Divider, delta: f32) {
        self.window_drag_divider(self.focused_window, divider, delta);
    }

    /// Extract v2 session data from the current state without window sizes.
    pub fn to_session_data(&self) -> crate::session::SessionData {
        self.to_session_data_with_sizes(&std::collections::HashMap::new())
    }

    /// Extract v2 session data from the current state, recording each window's
    /// logical inner size when supplied.
    pub fn to_session_data_with_sizes(
        &self,
        sizes: &std::collections::HashMap<WindowKey, crate::session::WindowSize>,
    ) -> crate::session::SessionData {
        use crate::session::{SessionData, TabData, TerminalData, WindowData, SESSION_VERSION};

        let windows = self
            .windows
            .iter()
            .filter_map(|record| {
                let mut tabs = Vec::new();
                let mut active_tab_index = 0;

                for &tab_id in &record.tabs {
                    let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else {
                        continue;
                    };
                    if record.active_tab == Some(tab_id) {
                        active_tab_index = tabs.len();
                    }
                    let terminals = tab
                        .terminals
                        .iter()
                        .filter_map(|&tid| {
                            self.terminals.iter().find(|t| t.id == tid).map(|t| {
                                let working_dir: Option<String> = t
                                    .event_loop
                                    .terminal()
                                    .working_directory()
                                    .map(|s| s.to_string())
                                    .or_else(|| t.event_loop.child_working_directory());
                                TerminalData {
                                    title: t.title.clone(),
                                    working_directory: working_dir,
                                    is_focused: tid == tab.focused_terminal,
                                }
                            })
                        })
                        .collect();

                    let working_directory: Option<String> = tab
                        .terminals
                        .first()
                        .and_then(|&tid| self.terminals.iter().find(|t| t.id == tid))
                        .and_then(|t| {
                            t.event_loop
                                .terminal()
                                .working_directory()
                                .map(str::to_string)
                                .or_else(|| t.event_loop.child_working_directory())
                        });

                    tabs.push(TabData {
                        title: tab.title.clone(),
                        working_directory,
                        terminals,
                        split_direction: direction_name(tab.layout.root_direction()).to_string(),
                        layout: Some(layout_data(tab.layout.root(), &tab.terminals)),
                    });
                }

                if tabs.is_empty() {
                    return None;
                }

                Some(WindowData {
                    tabs,
                    active_tab_index,
                    size: sizes.get(&record.id).copied(),
                })
            })
            .collect();

        SessionData {
            version: SESSION_VERSION,
            timestamp: crate::session::current_timestamp(),
            windows,
        }
    }

    /// Create a terminal manager from session data (v2 multi-window or
    /// upgraded v1 single-window).
    ///
    /// `tab_env` supplies the shell environment of every restored tab.
    pub fn from_session(
        session: &crate::session::SessionData,
        cols: u16,
        rows: u16,
        tab_env: TabEnvFn,
    ) -> io::Result<Self> {
        let non_empty_windows: Vec<_> = session
            .windows
            .iter()
            .filter(|window| !window.tabs.is_empty())
            .collect();
        if non_empty_windows.is_empty() {
            return Self::with_tab_env(cols, rows, tab_env);
        }

        let mut manager = Self::empty(cols, rows);

        for (win_idx, win_data) in non_empty_windows.into_iter().enumerate() {
            let window_key = if win_idx == 0 {
                WindowKey::PRIMARY
            } else {
                let key = WindowKey(manager.next_window_id);
                manager.next_window_id += 1;
                manager.windows.push(WindowRecord::new(key, cols, rows));
                key
            };

            for (i, tab_data) in win_data.tabs.iter().enumerate() {
                let tab_id = TabId(manager.next_tab_id);
                manager.next_tab_id += 1;
                let env = tab_env(tab_id)?;

                let terminal_count = tab_data.terminals.len().max(1);
                let mut terminal_ids = Vec::new();
                let mut focused_terminal_id = None;

                for (j, term_data) in tab_data.terminals.iter().take(terminal_count).enumerate() {
                    let tid = TerminalId(manager.next_terminal_id);
                    manager.next_terminal_id += 1;

                    let mut terminal = if let Some(ref wd) = term_data.working_directory {
                        ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, None, wd, &env)?
                    } else if let Some(ref wd) = tab_data.working_directory {
                        ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, None, wd, &env)?
                    } else {
                        ManagedTerminal::new(tid, tab_id, cols, rows, None, &env)?
                    };

                    terminal.title = term_data.title.clone();
                    manager.terminals.push(terminal);
                    terminal_ids.push(tid);

                    if term_data.is_focused || j == 0 {
                        focused_terminal_id = Some(tid);
                    }
                }

                if terminal_ids.is_empty() {
                    let tid = TerminalId(manager.next_terminal_id);
                    manager.next_terminal_id += 1;
                    let mut terminal = if let Some(ref wd) = tab_data.working_directory {
                        ManagedTerminal::new_with_dir(tid, tab_id, cols, rows, None, wd, &env)?
                    } else {
                        ManagedTerminal::new(tid, tab_id, cols, rows, None, &env)?
                    };
                    terminal.title = tab_data.title.clone();
                    manager.terminals.push(terminal);
                    terminal_ids.push(tid);
                    focused_terminal_id = Some(tid);
                }

                let split_direction = parse_direction(&tab_data.split_direction);
                let layout = tab_data
                    .layout
                    .as_ref()
                    .and_then(|data| restore_layout(data, &terminal_ids))
                    .unwrap_or_else(|| flat_layout(split_direction, &terminal_ids));

                let tab = Tab {
                    id: tab_id,
                    window_id: window_key,
                    focused_terminal: focused_terminal_id.ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "no focused terminal in restored tab",
                        )
                    })?,
                    title: tab_data.title.clone(),
                    terminals: layout.panes(),
                    zoomed_pane: None,
                    layout,
                    shell_env: env,
                };

                manager.tabs.push(tab);
                if let Some(record) = manager.window_record_mut(window_key) {
                    record.tabs.push(tab_id);
                    if i == win_data.active_tab_index || record.active_tab.is_none() {
                        record.active_tab = Some(tab_id);
                    }
                }
            }
        }

        manager.focused_window = WindowKey::PRIMARY;
        Ok(manager)
    }
}

fn direction_name(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Horizontal => "Horizontal",
        SplitDirection::Vertical => "Vertical",
    }
}

fn parse_direction(name: &str) -> SplitDirection {
    match name {
        "Vertical" => SplitDirection::Vertical,
        _ => SplitDirection::Horizontal,
    }
}

/// A layout tree for saving, panes as positions in `panes`.
fn layout_data(node: &PaneNode, panes: &[TerminalId]) -> crate::session::LayoutData {
    use crate::session::LayoutData;
    match node {
        PaneNode::Leaf(id) => LayoutData::Pane {
            index: panes.iter().position(|p| p == id).unwrap_or(0),
        },
        PaneNode::Split {
            direction,
            children,
        } => LayoutData::Split {
            direction: direction_name(*direction).to_string(),
            children: children
                .iter()
                .map(|(child, ratio)| (layout_data(child, panes), *ratio))
                .collect(),
        },
    }
}

/// Rebuild a saved tree over `panes`; None unless it names each pane once.
fn restore_layout(data: &crate::session::LayoutData, panes: &[TerminalId]) -> Option<PaneLayout> {
    use crate::session::LayoutData;
    fn build(data: &LayoutData, panes: &[TerminalId], used: &mut Vec<bool>) -> Option<PaneNode> {
        match data {
            LayoutData::Pane { index } => {
                let seen = used.get_mut(*index)?;
                if *seen {
                    return None;
                }
                *seen = true;
                Some(PaneNode::Leaf(panes[*index]))
            }
            LayoutData::Split {
                direction,
                children,
            } => {
                if children.len() < 2 {
                    return None;
                }
                let total: f32 = children.iter().map(|(_, r)| r).sum();
                if !(total.is_finite() && total > 0.0) {
                    return None;
                }
                let children = children
                    .iter()
                    .map(|(child, ratio)| Some((build(child, panes, used)?, ratio / total)))
                    .collect::<Option<Vec<_>>>()?;
                Some(PaneNode::Split {
                    direction: parse_direction(direction),
                    children,
                })
            }
        }
    }
    let mut used = vec![false; panes.len()];
    let root = build(data, panes, &mut used)?;
    used.iter().all(|u| *u).then(|| PaneLayout::from_root(root))
}

/// `panes` in one row (Horizontal) or column (Vertical), equal shares.
fn flat_layout(direction: SplitDirection, panes: &[TerminalId]) -> PaneLayout {
    match panes {
        [] => unreachable!("a restored tab always has a pane"),
        [only] => PaneLayout::new(*only),
        many => PaneLayout::from_root(PaneNode::Split {
            direction,
            children: many
                .iter()
                .map(|id| (PaneNode::Leaf(*id), 1.0 / many.len() as f32))
                .collect(),
        }),
    }
}

fn compact_working_directory(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let home = std::env::var("HOME").ok();
    if let Some(home) = home.as_deref().filter(|home| !home.is_empty()) {
        if path == home {
            return Some("~".to_string());
        }

        if let Some(rest) = path
            .strip_prefix(home)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return Some(format!("~/{rest}"));
        }
    }

    Some(path.to_string())
}

fn is_shell_label(label: &str) -> bool {
    let command = Path::new(label)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label);

    matches!(
        command,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish"
    )
}

impl ManagedTerminal {
    /// Create a new managed terminal with a specific working directory and
    /// extra shell environment.
    pub fn new_with_dir(
        id: TerminalId,
        tab_id: TabId,
        cols: u16,
        rows: u16,
        cell_px: Option<(u16, u16)>,
        working_dir: &str,
        env: &[(OsString, OsString)],
    ) -> io::Result<Self> {
        let child = Child::spawn_shell_in_dir(
            PtySize::for_cells(cols, rows, cell_px),
            working_dir,
            &pane_env(id, env),
        )
        .map_err(io::Error::other)?;
        let event_loop = TerminalEventLoop::from_child(child, cols, rows)?;
        Ok(Self {
            id,
            event_loop,
            tab_id,
            title: format!("Terminal {}", id.0),
            status: None,
            progress: None,
            program_status: None,
            program_status_snapshot: ultty_vt::program_status::ProgramStatusStore::default()
                .snapshot(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reports_map_to_pane_statuses() {
        use crate::protocol::ansi::Progress;
        use ultty_protocol::control::PaneState;
        let shown = |progress| progress_status(progress).map(|s| (s.state, s.progress));
        assert_eq!(shown(Progress::Clear), None);
        assert_eq!(
            shown(Progress::Normal(40)),
            Some((PaneState::Working, Some(0.4)))
        );
        assert_eq!(
            shown(Progress::Indeterminate),
            Some((PaneState::Working, None))
        );
        assert_eq!(
            shown(Progress::Error(Some(70))),
            Some((PaneState::Error, Some(0.7)))
        );
        assert_eq!(
            shown(Progress::Paused(None)),
            Some((PaneState::Waiting, None))
        );
    }

    #[test]
    fn terminals_report_the_cell_size_and_theme_they_are_drawn_with() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let theme = ThemeColors {
            background: [1, 2, 3],
            ..ThemeColors::default()
        };
        manager.set_theme_colors(theme.clone());
        manager.layout_panes(AREA).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let second = manager.split_horizontal().unwrap().unwrap();
        manager.create_tab().unwrap();
        let third = manager.focused_terminal_id().unwrap();
        for id in [first, second, third] {
            let state = manager.terminal(id).unwrap().event_loop.terminal();
            assert_eq!(state.cell_pixel_size(), Some((10, 20)), "{id:?}");
            assert_eq!(state.palette().theme(), &theme, "{id:?}");
        }
    }

    #[test]
    fn mixed_splits_size_each_pane_to_its_place_in_the_tree() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        // [first | right], then split `right` down: [first | (right / below)].
        let right = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(right);
        let below = manager.split_vertical().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (40, 24));
        assert_eq!(grid(&manager, right), (40, 12));
        assert_eq!(grid(&manager, below), (40, 12));

        // Focus follows the geometry: left of `below` is `first`.
        manager.focus_terminal(below);
        manager.focus_direction(FocusDirection::Left, AREA.width, AREA.height);
        assert_eq!(manager.focused_terminal_id(), Some(first));
    }

    #[test]
    fn sessions_keep_nested_splits() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let right = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(right);
        manager.split_vertical().unwrap().unwrap();
        let saved = manager.to_session_data();
        assert_eq!(saved.version, crate::session::SESSION_VERSION);
        assert_eq!(saved.windows.len(), 1);
        let layout = saved.windows[0].tabs[0].layout.clone().unwrap();
        let restored =
            restore_layout(&layout, &[TerminalId(10), TerminalId(11), TerminalId(12)]).unwrap();
        let rects = restored.rects((0.0, 0.0, 800.0, 480.0));
        assert_eq!(rects[0], (TerminalId(10), (0.0, 0.0, 400.0, 480.0)));
        assert_eq!(rects[1], (TerminalId(11), (400.0, 0.0, 400.0, 240.0)));
        assert_eq!(rects[2], (TerminalId(12), (400.0, 240.0, 400.0, 240.0)));
        // A tree that does not cover the panes falls back to a flat layout.
        assert!(restore_layout(&layout, &[TerminalId(10), TerminalId(11)]).is_none());
    }

    #[test]
    fn multi_window_session_round_trips_windows_tabs_and_sizes() {
        use crate::session::WindowSize;
        use std::collections::HashMap;

        let mut manager = TerminalManager::new(80, 24).unwrap();
        let w0_tab2 = manager
            .create_tab_in_window(WindowKey::PRIMARY, &mut no_tab_env)
            .unwrap();
        manager.set_tab_title(w0_tab2, "win0-second");
        let (w1, w1_tab1) = manager.create_window(80, 24, &mut no_tab_env).unwrap();
        manager.set_tab_title(w1_tab1, "win1-first");

        let sizes = HashMap::from([
            (
                WindowKey::PRIMARY,
                WindowSize {
                    width: 960.0,
                    height: 640.0,
                },
            ),
            (
                w1,
                WindowSize {
                    width: 720.0,
                    height: 480.0,
                },
            ),
        ]);
        let saved = manager.to_session_data_with_sizes(&sizes);
        assert_eq!(saved.version, 2);
        assert_eq!(saved.windows.len(), 2);
        assert_eq!(saved.windows[0].tabs.len(), 2);
        assert_eq!(saved.windows[0].active_tab_index, 1);
        assert_eq!(
            saved.windows[0].size,
            sizes.get(&WindowKey::PRIMARY).copied()
        );
        assert_eq!(saved.windows[1].tabs.len(), 1);
        assert_eq!(saved.windows[1].active_tab_index, 0);
        assert_eq!(saved.windows[1].size, sizes.get(&w1).copied());

        let restored = TerminalManager::from_session(&saved, 80, 24, &mut no_tab_env).unwrap();
        assert_eq!(restored.window_count(), 2);
        let keys = restored.window_ids();
        assert_eq!(keys.len(), 2);
        assert_eq!(restored.window_tab_count(keys[0]), 2);
        assert_eq!(restored.window_tab_count(keys[1]), 1);
        assert_eq!(
            restored.window_active_tab(keys[0]).unwrap().title,
            "win0-second"
        );
        assert_eq!(
            restored.window_active_tab(keys[1]).unwrap().title,
            "win1-first"
        );
    }

    #[test]
    fn move_tab_to_window_preserves_pid_and_resizes_to_destination_area() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let tab0 = manager.tabs()[0].id;
        let tab1 = manager
            .create_tab_in_window(WindowKey::PRIMARY, &mut no_tab_env)
            .unwrap();
        let (w1, _tab2) = manager.create_window(80, 24, &mut no_tab_env).unwrap();

        let big_area = PaneArea {
            width: 800.0,
            height: 480.0,
            cell_width: 10.0,
            cell_height: 20.0,
        };
        let small_area = PaneArea {
            width: 400.0,
            height: 240.0,
            cell_width: 10.0,
            cell_height: 20.0,
        };
        manager
            .layout_window_panes(WindowKey::PRIMARY, big_area)
            .unwrap();
        manager.layout_window_panes(w1, small_area).unwrap();

        let moved_pane = manager
            .tabs()
            .iter()
            .find(|t| t.id == tab1)
            .unwrap()
            .focused_terminal;
        let pid_before = manager.terminal(moved_pane).unwrap().event_loop.child_pid();
        assert_eq!(grid(&manager, moved_pane), (80, 24));

        let outcome = manager.move_tab_to_window(tab1, w1).unwrap();
        assert_eq!(
            outcome,
            MoveTabOutcome {
                source_window: WindowKey::PRIMARY,
                destination_window: w1,
                source_empty: false,
            }
        );
        assert_eq!(manager.window_for_tab(tab1), Some(w1));
        assert_eq!(manager.window_tab_count(WindowKey::PRIMARY), 1);
        assert_eq!(manager.window_tab_count(w1), 2);
        assert_eq!(manager.window_active_tab(w1).map(|t| t.id), Some(tab1));
        assert_eq!(
            manager.terminal(moved_pane).unwrap().event_loop.child_pid(),
            pid_before
        );
        assert_eq!(grid(&manager, moved_pane), (40, 12));

        let second = manager.move_tab_to_window(tab0, w1).unwrap();
        assert!(second.source_empty);
        assert_eq!(manager.window_tab_count(WindowKey::PRIMARY), 0);
        assert_eq!(manager.window_tab_count(w1), 3);
    }

    #[test]
    fn detach_tab_to_new_window_moves_tab_and_rejects_sole_tab() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let tab0 = manager.tabs()[0].id;
        assert!(manager.detach_tab_to_new_window(tab0, 80, 24).is_err());

        let tab1 = manager
            .create_tab_in_window(WindowKey::PRIMARY, &mut no_tab_env)
            .unwrap();
        let pane1 = manager
            .tabs()
            .iter()
            .find(|t| t.id == tab1)
            .unwrap()
            .focused_terminal;
        let pid_before = manager.terminal(pane1).unwrap().event_loop.child_pid();

        let (new_win, outcome) = manager.detach_tab_to_new_window(tab1, 80, 24).unwrap();
        assert_ne!(new_win, WindowKey::PRIMARY);
        assert!(!outcome.source_empty);
        assert_eq!(manager.window_count(), 2);
        assert_eq!(manager.window_for_tab(tab1), Some(new_win));
        assert_eq!(manager.window_tab_count(WindowKey::PRIMARY), 1);
        assert_eq!(manager.window_tab_count(new_win), 1);
        assert_eq!(
            manager.terminal(pane1).unwrap().event_loop.child_pid(),
            pid_before
        );
    }

    fn grid(manager: &TerminalManager, id: TerminalId) -> (u16, u16) {
        let terminal = manager.terminals.iter().find(|t| t.id == id).unwrap();
        let dims = terminal.event_loop.terminal().dimensions();
        (dims.cols, dims.rows)
    }

    /// 80 x 24 cells of 10 x 20 px.
    const AREA: PaneArea = PaneArea {
        width: 800.0,
        height: 480.0,
        cell_width: 10.0,
        cell_height: 20.0,
    };

    #[test]
    fn split_panes_get_grids_the_size_of_their_share() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        assert_eq!(grid(&manager, first), (80, 24));

        // Side by side: each pane gets half the columns, all the rows.
        let second = manager.split_horizontal().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (40, 24));
        assert_eq!(grid(&manager, second), (40, 24));

        // A third pane takes half of the pane it splits (the focused,
        // first one); the other pane keeps its size.
        let third = manager.split_horizontal().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (20, 24));
        assert_eq!(grid(&manager, third), (20, 24));
        assert_eq!(grid(&manager, second), (40, 24));

        // Closing a pane gives its space back to the two that remain.
        assert!(manager.close_focused_pane());
        let remaining = manager.active_tab().unwrap().terminals.clone();
        assert_eq!(remaining.len(), 2);
        for id in remaining {
            assert_eq!(grid(&manager, id), (40, 24));
        }
    }

    #[test]
    fn layout_generation_counts_every_pane_layout() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let start = manager.layout_generation();
        manager.layout_panes(AREA).unwrap();
        assert_eq!(manager.layout_generation(), start + 1);
        // A split lays panes out again; a tab switch does not.
        manager.split_horizontal().unwrap().unwrap();
        assert_eq!(manager.layout_generation(), start + 2);
        manager.next_tab();
        assert_eq!(manager.layout_generation(), start + 2);
        assert!(manager.close_focused_pane());
        assert_eq!(manager.layout_generation(), start + 3);
    }

    #[test]
    fn pane_grid_size_is_the_pane_share() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        let second = manager.split_horizontal().unwrap().unwrap();
        assert_eq!(manager.pane_grid_size(first), Some((40, 24)));
        assert_eq!(manager.pane_grid_size(second), Some((40, 24)));
        assert_eq!(manager.pane_grid_size(TerminalId::new(999)), None);
    }

    #[test]
    fn a_program_is_in_the_pane_whose_pty_has_its_group_in_the_foreground() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        // Panes running `sleep`, whose foreground group never changes once
        // set (a starting shell's does, as its startup files run).
        let sleep = ["30".to_string()];
        let tab = manager
            .create_tab_with_command("sleep", &sleep, None, None, &mut no_tab_env)
            .unwrap();
        let first = manager
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .unwrap()
            .focused_terminal;
        // One pane: any group, even none, is that pane's.
        assert_eq!(manager.pane_for_process_group(tab, None), Some(first));
        assert_eq!(manager.pane_for_process_group(tab, Some(1)), Some(first));

        let second = manager
            .split_pane_with_command(first, SplitDirection::Horizontal, "sleep", &sleep, None)
            .unwrap()
            .unwrap();
        // The program takes its PTY as controlling terminal in the child,
        // after spawn returns: wait for it.
        let group_of = |manager: &TerminalManager, id: TerminalId| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(group) = manager.terminal(id).unwrap().event_loop.foreground_pgrp() {
                    return group;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the program never became its PTY's foreground group"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let first_group = group_of(&manager, first);
        let second_group = group_of(&manager, second);
        assert_ne!(first_group, second_group);
        assert_eq!(
            manager.pane_for_process_group(tab, Some(first_group)),
            Some(first)
        );
        assert_eq!(
            manager.pane_for_process_group(tab, Some(second_group)),
            Some(second)
        );
        // A group in no pane's foreground (a background job) stays unbound,
        // as does an unknown tab.
        assert_eq!(manager.pane_for_process_group(tab, Some(1)), None);
        assert_eq!(manager.pane_for_process_group(tab, None), None);
        assert_eq!(
            manager.pane_for_process_group(TabId::new(99), Some(first_group)),
            None
        );
    }

    #[test]
    fn stacked_panes_split_the_rows() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        manager.layout_panes(AREA).unwrap();
        let second = manager.split_vertical().unwrap().unwrap();
        assert_eq!(grid(&manager, first), (80, 12));
        assert_eq!(grid(&manager, second), (80, 12));
    }

    #[test]
    fn test_zoom_and_swap_preserve_children_and_saved_layout() {
        let mut manager = quiet_manager();
        manager.layout_panes(AREA).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let second = manager.split_horizontal().unwrap().unwrap();
        let first_pid = manager.terminal(first).unwrap().event_loop.child_pid();
        let second_pid = manager.terminal(second).unwrap().event_loop.child_pid();
        let layout = manager.active_tab().unwrap().layout.clone();

        assert!(manager.set_pane_zoom(first, true));
        assert_eq!(
            manager.active_pane_rects(800.0, 480.0),
            vec![(first, (0.0, 0.0, 800.0, 480.0))]
        );
        assert!(manager.is_on_screen(first));
        assert!(!manager.is_on_screen(second));
        assert_eq!(manager.pane_at(799.0, 100.0, 800.0, 480.0), Some(first));
        assert!(manager
            .divider_at(400.0, 100.0, 800.0, 480.0, 10.0)
            .is_none());
        assert_eq!(grid(&manager, first), (80, 24));
        assert_eq!(manager.active_tab().unwrap().layout, layout);

        manager.focus_next_pane();
        assert_eq!(manager.active_tab().unwrap().zoomed_pane, Some(second));
        assert!(!manager.is_on_screen(first));
        assert!(manager.set_pane_zoom(second, false));
        assert_eq!(grid(&manager, first), (40, 24));
        assert_eq!(grid(&manager, second), (40, 24));
        assert!(manager.swap_panes(first, second));
        assert_eq!(manager.active_tab().unwrap().terminals, vec![second, first]);
        assert_eq!(
            manager.terminal(first).unwrap().event_loop.child_pid(),
            first_pid
        );
        assert_eq!(
            manager.terminal(second).unwrap().event_loop.child_pid(),
            second_pid
        );
        assert!(manager.swap_panes(first, second));
        assert_eq!(manager.active_tab().unwrap().layout, layout);

        manager.create_tab().unwrap();
        let other = manager.focused_terminal_id().unwrap();
        assert!(!manager.swap_panes(first, other));
        assert!(manager.focus_pane_by_id(first));
        assert_eq!(manager.focused_terminal_id(), Some(first));
        assert!(!manager.focus_pane_by_id(TerminalId::new(usize::MAX)));
        assert_eq!(manager.focused_terminal_id(), Some(first));
    }

    #[test]
    fn test_program_status_projection_preserves_explicit_override() {
        use ultty_protocol::control::{PaneState, PaneStatus};
        use ultty_vt::program_status::{ProgramState, ProgramStatusRecord, ProgramStatusReport};

        let mut manager = quiet_manager();
        let terminal = manager.focused_terminal_mut().unwrap();
        terminal
            .event_loop
            .terminal()
            .apply_program_status(ProgramStatusReport::Set(ProgramStatusRecord {
                id: "build".to_owned(),
                state: ProgramState::Blocked,
                kind: None,
                progress: Some(25),
                app: None,
                title: Some("needs input".to_owned()),
                msg: None,
                updated: 0,
            }));
        terminal.process().unwrap();
        assert_eq!(terminal.shown_status().unwrap().state, PaneState::Waiting);
        assert_eq!(terminal.shown_status().unwrap().progress, Some(0.25));
        terminal.status = Some(PaneStatus {
            state: PaneState::Error,
            label: "override".to_owned(),
            progress: None,
            attention: false,
        });
        assert_eq!(terminal.shown_status().unwrap().label, "override");
        terminal
            .event_loop
            .terminal()
            .apply_program_status(ProgramStatusReport::Clear(String::new()));
        terminal.process().unwrap();
        terminal.status = None;
        assert!(terminal.shown_status().is_none());
    }

    #[test]
    fn test_job_result_survives_immediate_process_and_pane_exit() {
        let mut manager = quiet_manager();
        manager
            .create_tab_with_command(
                "/bin/sh",
                &[
                    "-c".to_owned(),
                    "(sleep 0.05; printf FINAL; sleep 0.1) & exit 7".to_owned(),
                ],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        let job = manager.focused_terminal_id().unwrap();
        let pid = manager.terminal(job).unwrap().event_loop.child_pid();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            manager.process_all().unwrap();
            // Match the App loop: cleanup runs before the next process poll.
            manager.cleanup_exited();
            if manager.job(job).unwrap().outcome == (ProcessJobOutcome::Exited { status: 7 }) {
                break;
            }
            assert!(Instant::now() < deadline, "process did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        manager.cleanup_exited();
        assert!(manager.terminal(job).is_none());
        assert_eq!(
            manager.job(job),
            Some(ProcessJob {
                id: job,
                pid,
                outcome: ProcessJobOutcome::Exited { status: 7 }
            })
        );
        assert!(manager.job(TerminalId::new(usize::MAX)).is_none());

        let tab = manager
            .create_tab_with_command("sleep", &["30".to_owned()], None, None, &mut no_tab_env)
            .unwrap();
        let closed = manager.focused_terminal_id().unwrap();
        manager.close_tab(tab);
        assert_eq!(
            manager.job(closed).unwrap().outcome,
            ProcessJobOutcome::Closed
        );
    }

    /// A manager whose only tab runs a shell that prints nothing.
    ///
    /// The I/O thread parses child output as it arrives, so a login shell's
    /// prompt (which on many systems sets the window title) would race with
    /// tests that set terminal state directly. `sh` stays a shell, so the
    /// foreground-process name does not stand in for the title either.
    fn quiet_manager() -> TerminalManager {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.active_tab().unwrap().id;
        manager
            .create_tab_with_command(
                "/bin/sh",
                &["-c".to_string(), "read never".to_string()],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        manager.close_tab(first);
        manager
    }

    use std::time::{Duration, Instant};

    /// Process output until the focused pane's screen contains `text`.
    fn wait_for_screen(manager: &mut TerminalManager, text: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            manager.process_all().unwrap();
            let screen = {
                let state = manager.focused_terminal().unwrap().event_loop.terminal();
                let grid = state.grid();
                (0..grid.dimensions().rows)
                    .filter_map(|r| grid.row(r).map(|row| row.text_trimmed()))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if screen.contains(text) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_program_that_asked_hears_the_theme_turn_light() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_theme_colors(ThemeColors {
            background: [0x20, 0x20, 0x20],
            ..ThemeColors::default()
        });
        // Turn on mode 2031, then print the next 9 bytes of input.
        let script = "stty raw -echo; printf '\\033[?2031hready'; \
                      r=$(dd bs=1 count=9 2>/dev/null | tr '\\033' E); \
                      printf ' got:%s' \"$r\"; sleep 5";
        manager
            .create_tab_with_command(
                "/bin/sh",
                &["-c".to_string(), script.to_string()],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        assert!(wait_for_screen(&mut manager, "ready"));
        manager.set_theme_colors(ThemeColors {
            background: [0xf8, 0xf8, 0xf8],
            ..ThemeColors::default()
        });
        assert!(wait_for_screen(&mut manager, "got:E[?997;2n"));
    }

    #[test]
    fn only_the_active_tabs_panes_are_on_screen() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let split = manager.split_horizontal().unwrap().unwrap();
        assert!(manager.is_on_screen(first) && manager.is_on_screen(split));
        manager.create_tab().unwrap();
        let other = manager.focused_terminal_id().unwrap();
        assert!(manager.is_on_screen(other));
        assert!(!manager.is_on_screen(first) && !manager.is_on_screen(split));
        manager.switch_to_tab(0);
        assert!(manager.is_on_screen(first) && !manager.is_on_screen(other));
    }

    #[test]
    fn a_tab_command_starts_in_the_working_directory_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager
            .create_tab_with_command(
                "sleep",
                &["5".to_string()],
                Some(dir.to_str().unwrap()),
                None,
                &mut no_tab_env,
            )
            .unwrap();
        // The child changes directory between fork and exec, so poll
        // briefly rather than racing it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let cwd = loop {
            let cwd = manager
                .focused_terminal()
                .unwrap()
                .event_loop
                .child_working_directory();
            if cwd.as_deref() == dir.to_str() || Instant::now() > deadline {
                break cwd;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(cwd.as_deref(), dir.to_str());
    }

    /// A pane running `script` under `sh`, with nothing else touching its
    /// OSC 7 state (an interactive shell's rc files might), made the focus.
    fn tab_running(manager: &mut TerminalManager, script: &str, dir: Option<&str>) -> TerminalId {
        manager
            .create_tab_with_command(
                "sh",
                &["-c".to_string(), script.to_string()],
                dir,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        manager.focused_terminal_id().unwrap()
    }

    /// Wait until the pane's shell is in `expected`. The child changes
    /// directory between fork and exec, so poll rather than race it.
    fn wait_for_cwd(manager: &TerminalManager, id: TerminalId, expected: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut cwd = None;
        while Instant::now() < deadline {
            cwd = manager
                .terminal(id)
                .unwrap()
                .event_loop
                .child_working_directory();
            if cwd.as_deref() == expected.to_str() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("pane cwd is {cwd:?}, wanted {expected:?}");
    }

    fn wait_for_osc7(manager: &mut TerminalManager, id: TerminalId) {
        run_until(manager, |m| {
            m.terminal(id)
                .unwrap()
                .event_loop
                .terminal()
                .working_directory_uri()
                .is_some()
        });
    }

    #[test]
    fn new_tab_opens_in_the_focused_panes_osc7_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let script = format!(
            "printf '\\033]7;file://%s\\007' '{}'; sleep 30",
            dir.display()
        );
        let source = tab_running(&mut manager, &script, None);
        wait_for_osc7(&mut manager, source);

        manager.create_tab().unwrap();

        let new = manager.focused_terminal_id().unwrap();
        wait_for_cwd(&manager, new, &dir);
    }

    #[test]
    fn split_opens_in_the_target_panes_osc7_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let script = format!(
            "printf '\\033]7;file://%s\\007' '{}'; sleep 30",
            dir.display()
        );
        let source = tab_running(&mut manager, &script, None);
        wait_for_osc7(&mut manager, source);

        let split = manager
            .split_pane(source, SplitDirection::Vertical)
            .unwrap()
            .unwrap();

        wait_for_cwd(&manager, split, &dir);
    }

    #[test]
    fn without_osc7_a_new_tab_uses_the_foreground_process_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let source = tab_running(&mut manager, "sleep 30", dir.to_str());
        // The child needs a moment to become the terminal's foreground group.
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager
            .terminal(source)
            .unwrap()
            .event_loop
            .foreground_working_directory()
            .as_deref()
            != dir.to_str()
        {
            assert!(Instant::now() < deadline, "source never settled");
            std::thread::sleep(Duration::from_millis(10));
        }

        manager.create_tab().unwrap();

        let new = manager.focused_terminal_id().unwrap();
        wait_for_cwd(&manager, new, &dir);
    }

    #[test]
    fn a_remote_or_missing_osc7_directory_falls_back_to_the_process() {
        let proc_dir = tempfile::tempdir().unwrap().keep().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let script = "printf '\\033]7;file://elsewhere/etc\\007'; sleep 30";
        let source = tab_running(&mut manager, script, proc_dir.to_str());
        wait_for_osc7(&mut manager, source);

        manager.create_tab().unwrap();

        let new = manager.focused_terminal_id().unwrap();
        wait_for_cwd(&manager, new, &proc_dir);
        std::fs::remove_dir_all(&proc_dir).unwrap();
    }

    #[test]
    fn inheriting_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_inherit_working_directory(false);
        let script = format!(
            "printf '\\033]7;file://%s\\007' '{}'; sleep 30",
            dir.display()
        );
        let source = tab_running(&mut manager, &script, None);
        wait_for_osc7(&mut manager, source);

        manager.create_tab().unwrap();

        let new = manager.focused_terminal_id().unwrap();
        let started_in = std::env::current_dir().unwrap().canonicalize().unwrap();
        wait_for_cwd(&manager, new, &started_in);
    }

    #[test]
    fn create_manager() {
        let manager = TerminalManager::new(80, 24).unwrap();
        assert_eq!(manager.tab_count(), 1);
        assert!(manager.focused_terminal().is_some());
    }

    #[test]
    fn a_manager_can_start_with_its_one_tab_running_a_command() {
        let script = "printf 'started-%s' \"$0\"; sleep 5";
        let mut manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sh",
            &["-c".to_string(), script.to_string(), "here".to_string()],
            &mut no_tab_env,
        )
        .unwrap();
        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.terminals.len(), 1);
        assert!(wait_for_screen(&mut manager, "started-here"));
    }

    #[test]
    fn tab_display_title_uses_explicit_title() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let tab_id = manager.active_tab().unwrap().id;
        manager.set_tab_title(tab_id, "editor");
        manager.terminals[0]
            .event_loop
            .terminal_mut()
            .set_title("nvim".to_string());

        assert_eq!(manager.tab_display_title(tab_id), "editor");
    }

    #[test]
    fn tab_display_title_uses_osc_title_for_default_tabs() {
        let mut manager = quiet_manager();
        let tab_id = manager.active_tab().unwrap().id;
        manager.terminals[0]
            .event_loop
            .terminal_mut()
            .set_title("nvim".to_string());

        assert_eq!(manager.tab_display_title(tab_id), "nvim");
    }

    #[test]
    fn tab_display_title_falls_back_to_working_directory_for_shell_titles() {
        let mut manager = quiet_manager();
        let tab_id = manager.active_tab().unwrap().id;
        {
            let mut terminal = manager.terminals[0].event_loop.terminal_mut();
            terminal.set_title("zsh".to_string());
            terminal.set_working_directory("/tmp/ultty-project".to_string());
        }

        assert_eq!(manager.tab_display_title(tab_id), "/tmp/ultty-project");
    }

    #[test]
    fn compact_working_directory_ignores_empty_paths() {
        assert_eq!(compact_working_directory(""), None);
        assert_eq!(compact_working_directory("   "), None);
    }

    #[test]
    fn shell_label_filter_handles_paths() {
        assert!(is_shell_label("/bin/zsh"));
        assert!(!is_shell_label("nvim"));
    }

    #[test]
    fn create_multiple_tabs() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        assert_eq!(manager.tab_count(), 1);

        manager.create_tab().unwrap();
        assert_eq!(manager.tab_count(), 2);

        manager.create_tab().unwrap();
        assert_eq!(manager.tab_count(), 3);
    }

    #[test]
    fn tab_navigation() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let second_tab = manager.tabs[1].id;
        let third_tab = manager.tabs[2].id;

        assert_eq!(manager.active_tab().map(|t| t.id), Some(third_tab));

        manager.prev_tab();
        assert_eq!(manager.active_tab().map(|t| t.id), Some(second_tab));

        manager.prev_tab();
        assert_eq!(manager.active_tab().map(|t| t.id), Some(first_tab));

        manager.prev_tab();
        assert_eq!(manager.active_tab().map(|t| t.id), Some(third_tab));

        manager.next_tab();
        assert_eq!(manager.active_tab().map(|t| t.id), Some(first_tab));
    }

    #[test]
    fn close_tab() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let second_tab = manager.tabs[1].id;

        assert!(manager.switch_to_tab_id(first_tab));
        manager.close_tab(first_tab);

        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.active_tab().map(|t| t.id), Some(second_tab));
    }

    /// Open a tab running `true` and return its terminal.
    fn tab_that_exits(manager: &mut TerminalManager) -> TerminalId {
        #[cfg(target_os = "macos")]
        const TRUE_BIN: &str = "/usr/bin/true";
        #[cfg(not(target_os = "macos"))]
        const TRUE_BIN: &str = "/bin/true";
        manager
            .create_tab_with_command(TRUE_BIN, &[], None, None, &mut no_tab_env)
            .unwrap();
        manager.focused_terminal_id().unwrap()
    }

    /// Process and clean up until `done`, or panic after a generous deadline.
    fn run_until(manager: &mut TerminalManager, done: impl Fn(&TerminalManager) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(manager) {
            assert!(std::time::Instant::now() < deadline, "timed out");
            manager.process_all().unwrap();
            manager.cleanup_exited();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_pane_closes_when_its_program_exits_by_default() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let exiting = tab_that_exits(&mut manager);
        assert_eq!(manager.tab_count(), 2);

        run_until(&mut manager, |m| m.terminal(exiting).is_none());

        assert_eq!(manager.tab_count(), 1);
    }

    #[test]
    fn without_close_on_exit_an_exited_pane_stays_with_a_note() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_close_on_exit(false);
        let exiting = tab_that_exits(&mut manager);

        let shows_note = |m: &TerminalManager| {
            let terminal = m.terminal(exiting).unwrap().event_loop.terminal();
            let shown = terminal
                .grid()
                .rows()
                .any(|row| row.text().contains("[process exited with status 0]"));
            shown
        };
        run_until(&mut manager, shows_note);

        assert_eq!(manager.tab_count(), 2);
        assert!(manager.terminal(exiting).unwrap().has_exited());
        assert!(!manager.all_exited(), "the window would close");

        // Closing the tab by hand still works.
        let tab = manager.active_tab().unwrap().id;
        assert!(manager.close_tab(tab));
        assert!(manager.terminal(exiting).is_none());
    }

    #[test]
    fn scrollback_lines_apply_to_open_and_new_terminals() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let first = manager.focused_terminal_id().unwrap();
        let max_history = |m: &TerminalManager, id| {
            m.terminal(id)
                .unwrap()
                .event_loop
                .terminal()
                .buffer()
                .max_history()
        };
        assert_eq!(
            max_history(&manager, first),
            crate::grid::DEFAULT_SCROLLBACK
        );

        manager.set_scrollback_lines(500);
        manager.create_tab().unwrap();
        let second = manager.focused_terminal_id().unwrap();

        assert_eq!(max_history(&manager, first), 500);
        assert_eq!(max_history(&manager, second), 500);
    }

    #[test]
    fn cleanup_exited_repairs_active_tab_when_active_tab_was_removed() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;
        let first_terminal = manager.tabs[0].focused_terminal;
        let second_tab = manager.tabs[1].id;
        let second_terminal = manager.tabs[1].focused_terminal;

        assert!(manager.switch_to_tab_id(second_tab));
        manager.tabs[1].remove_terminal(second_terminal);
        manager
            .terminals
            .retain(|terminal| terminal.id != second_terminal);

        manager.cleanup_exited();

        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.active_tab().map(|t| t.id), Some(first_tab));
        assert_eq!(
            manager.focused_terminal().map(|terminal| terminal.id),
            Some(first_terminal)
        );
    }

    #[test]
    fn switch_to_tab_by_index() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.create_tab().unwrap();
        manager.create_tab().unwrap();

        let first_tab = manager.tabs[0].id;

        manager.switch_to_tab(0);
        assert_eq!(manager.active_tab().map(|t| t.id), Some(first_tab));
    }

    #[test]
    fn test_per_window_relayout_sizes_each_window_independently() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let win_a = manager.primary_window();
        let pane_a1 = manager.window_focused_terminal_id(win_a).unwrap();

        let (win_b, _tab_b) = manager.create_window(80, 24, &mut no_tab_env).unwrap();
        let pane_b1 = manager.window_focused_terminal_id(win_b).unwrap();
        assert_ne!(win_a, win_b);
        assert_eq!(manager.window_count(), 2);
        assert_eq!(manager.window_tab_count(win_a), 1);
        assert_eq!(manager.window_tab_count(win_b), 1);

        manager
            .layout_window_panes(
                win_a,
                PaneArea {
                    width: 800.0,
                    height: 400.0,
                    cell_width: 10.0,
                    cell_height: 20.0,
                },
            )
            .unwrap();
        manager
            .layout_window_panes(
                win_b,
                PaneArea {
                    width: 1200.0,
                    height: 600.0,
                    cell_width: 10.0,
                    cell_height: 20.0,
                },
            )
            .unwrap();

        assert_eq!(manager.pane_grid_size(pane_a1), Some((80, 20)));
        assert_eq!(manager.pane_grid_size(pane_b1), Some((120, 30)));

        let pane_a2 = manager
            .split_window_focused(win_a, SplitDirection::Horizontal)
            .unwrap()
            .unwrap();
        assert_eq!(manager.pane_grid_size(pane_a1), Some((40, 20)));
        assert_eq!(manager.pane_grid_size(pane_a2), Some((40, 20)));
        assert_eq!(manager.pane_grid_size(pane_b1), Some((120, 30)));

        assert!(manager.is_on_screen(pane_a1));
        assert!(manager.is_on_screen(pane_a2));
        assert!(manager.is_on_screen(pane_b1));
        assert!(manager.is_on_screen_in_window(win_a, pane_a1));
        assert!(!manager.is_on_screen_in_window(win_a, pane_b1));

        assert!(manager.close_window(win_b));
        assert_eq!(manager.window_count(), 1);
        assert!(manager.terminal(pane_b1).is_none());
        assert_eq!(manager.focused_window(), win_a);
        assert_eq!(manager.pane_grid_size(pane_a1), Some((40, 20)));
    }

    #[test]
    fn test_multi_window_close_and_dirty_isolation() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let win_a = WindowKey::PRIMARY;
        let tab_a = manager.window_active_tab(win_a).unwrap().id;
        let pane_a = manager.window_focused_terminal(win_a).unwrap().id;

        let (win_b, tab_b) = manager
            .create_window(80, 24, &mut |_| Ok(Vec::new()))
            .unwrap();
        let pane_b = manager.window_focused_terminal(win_b).unwrap().id;

        assert_eq!(manager.window_tabs(win_a).len(), 1);
        assert_eq!(manager.window_tabs(win_b).len(), 1);
        assert!(manager.is_on_screen_in_window(win_a, pane_a));
        assert!(!manager.is_on_screen_in_window(win_a, pane_b));
        assert!(manager.is_on_screen_in_window(win_b, pane_b));
        assert!(!manager.is_on_screen_in_window(win_b, pane_a));
        assert_eq!(manager.active_tab_ids(), vec![tab_a, tab_b]);

        assert!(
            manager
                .terminal(pane_a)
                .unwrap()
                .event_loop
                .terminal()
                .has_dirty(),
            "new terminal A should start dirty"
        );
        assert!(
            manager
                .terminal(pane_b)
                .unwrap()
                .event_loop
                .terminal()
                .has_dirty(),
            "new terminal B should start dirty"
        );

        manager.clear_window_active_dirty(win_a);
        assert!(!manager
            .terminal(pane_a)
            .unwrap()
            .event_loop
            .terminal()
            .has_dirty());
        assert!(
            manager
                .terminal(pane_b)
                .unwrap()
                .event_loop
                .terminal()
                .has_dirty(),
            "clearing one window's dirty flags must not clear another window"
        );

        manager.close_window(win_b);
        assert!(manager.window_is_empty(win_b));
        assert!(!manager.window_is_empty(win_a));
        assert_eq!(manager.tab_count(), 1);
        assert_eq!(manager.window_tabs(win_a).len(), 1);
        assert_eq!(manager.focused_window(), win_a);
        assert_eq!(manager.active_tab_ids(), vec![tab_a]);
    }

    #[test]
    fn test_multi_window_cleanup_exited_only_empties_affected_window() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let win_a = WindowKey::PRIMARY;
        let (win_b, initial_tab_b) = manager
            .create_window(80, 24, &mut |_| Ok(Vec::new()))
            .unwrap();
        let _tab_b = manager
            .create_tab_with_command_in_window(
                win_b,
                "/bin/sh",
                &["-c".to_string(), "exit 0".to_string()],
                None,
                None,
                &mut |_| Ok(Vec::new()),
            )
            .unwrap();
        assert!(manager.close_tab(initial_tab_b));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !manager.window_is_empty(win_b) && std::time::Instant::now() < deadline {
            let _ = manager.process_all();
            manager.cleanup_exited();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(manager.window_is_empty(win_b));
        assert!(
            !manager.window_is_empty(win_a),
            "exiting window B's shell must leave window A open"
        );
        assert_eq!(manager.window_tabs(win_a).len(), 1);
    }

    /// Value of `key` in the child's environment once it has exec'd, or
    /// `None` if it never shows up within the timeout.
    #[cfg(target_os = "linux")]
    fn wait_for_child_env(pid: u32, key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) {
                for entry in environ.split(|byte| *byte == 0) {
                    let entry = String::from_utf8_lossy(entry);
                    if let Some(value) = entry.strip_prefix(&prefix) {
                        return Some(value.to_string());
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[cfg(target_os = "linux")]
    fn env_for_tab(tab_id: TabId) -> io::Result<TabEnv> {
        Ok(vec![(
            OsString::from("ULTTY_TAB_ENV_TEST"),
            OsString::from(format!("tab-{}", tab_id.raw())),
        )])
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn tab_env_reaches_the_tab_shell_and_its_split_panes() {
        let mut seen = Vec::new();
        let mut manager = TerminalManager::with_tab_env(80, 24, &mut |tab_id| {
            seen.push(tab_id);
            env_for_tab(tab_id)
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![TabId::new(0)],
            "env fn is called once with the new tab id"
        );

        let first_tab = manager.active_tab().unwrap();
        assert_eq!(first_tab.shell_env, env_for_tab(TabId::new(0)).unwrap());
        let first_pid = manager.focused_terminal().unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(first_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-0"),
            "the tab's shell sees its env"
        );

        // A pane split off the tab inherits the tab's env without the env
        // fn being consulted again.
        let pane = manager.split_horizontal().unwrap().unwrap();
        let pane_pid = manager.terminal(pane).unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(pane_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-0")
        );

        // A second tab gets its own env, and the first tab's is unchanged.
        let second = manager.create_tab_with_env(&mut env_for_tab).unwrap();
        assert_eq!(second, TabId::new(1));
        let second_pid = manager.focused_terminal().unwrap().event_loop.child_pid();
        assert_eq!(
            wait_for_child_env(second_pid, "ULTTY_TAB_ENV_TEST").as_deref(),
            Some("tab-1")
        );
        assert_eq!(
            manager.active_tab().unwrap().shell_env,
            env_for_tab(TabId::new(1)).unwrap()
        );
    }

    #[test]
    fn tab_env_error_aborts_tab_creation() {
        let mut manager = TerminalManager::new(80, 24).unwrap();
        let err = manager
            .create_tab_with_env(&mut |_| Err(io::Error::other("no socket")))
            .unwrap_err();
        assert_eq!(err.to_string(), "no socket");
        assert_eq!(manager.tab_count(), 1, "failed tab is not added");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn create_window_in_dir_starts_shell_in_requested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let mut manager = TerminalManager::new(80, 24).unwrap();
        manager.set_inherit_working_directory(false);

        let (win_key, _tab_id) = manager
            .create_window_in_dir(80, 24, Some(expected.to_str().unwrap()), &mut no_tab_env)
            .unwrap();
        let pid = manager
            .window_focused_terminal(win_key)
            .unwrap()
            .event_loop
            .child_pid();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut observed = None;
        while std::time::Instant::now() < deadline {
            if let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                if cwd == expected {
                    observed = Some(cwd);
                    break;
                }
                observed = Some(cwd);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(observed, Some(expected));
    }
}
