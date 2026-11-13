//! Terminal state management with alternate screen support.

pub mod snapshot;

use super::{
    Anchor, AnchorRefusal, Buffer, CursorPosition, Dimensions, Grid, ImageStore, Palette, RowMarks,
    TerminalImage, ThemeColors,
};

/// Most anchor events kept for the embedder; past it the oldest go.
pub const MAX_ANCHOR_EVENTS: usize = 4096;

/// Most `Refused` anchor events queued at once; later refusals are dropped
/// until the queue is taken, so they never push out `Placed` or `Lost`.
pub const MAX_QUEUED_REFUSALS: usize = 64;

/// What happened to inline surface anchors (ADR 0020), for the embedder to
/// pair with sessions. See [`Terminal::take_anchor_events`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorEvent {
    /// Rows were reserved and tagged `tag` on the main screen.
    Placed {
        tag: u16,
        /// See [`Buffer::anchor_serial`].
        serial: u64,
        anchor: Anchor,
    },
    /// The anchor reserved nothing.
    Refused {
        anchor: Anchor,
        reason: AnchorRefusal,
    },
    /// The anchor's first row is gone: erased, cleared, reset, or evicted
    /// from scrollback. Its tag is free again.
    Lost {
        tag: u16,
        /// The serial it was placed with.
        serial: u64,
        anchor: Anchor,
    },
}

/// Maximum number of distinct OSC 8 hyperlink URIs remembered per terminal.
///
/// Identical URIs share one id, so only genuinely different links count.
/// When the map is full the oldest link is forgotten; cells that still
/// reference it simply stop resolving to a URI.
pub const MAX_HYPERLINKS: usize = 4096;

/// Deepest the title stack (XTWINOPS 22/23) grows; a push beyond it
/// forgets the oldest entry.
pub const MAX_TITLE_STACK: usize = 10;

/// Entries the Kitty keyboard mode stack keeps per screen; older ones are
/// dropped.
pub const KITTY_KEYBOARD_STACK_LIMIT: usize = 8;

/// The `TERM` a terminal reports until its embedder says otherwise.
pub const DEFAULT_TERMINAL_NAME: &str = "xterm-256color";

/// Largest value a mode 1005 mouse report encodes: xterm stops at 2047,
/// the last two-byte UTF-8 code point, so coordinates top out at 2015.
const MAX_UTF8_MOUSE_VALUE: u32 = 2047;

/// Mouse tracking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum MouseMode {
    /// No mouse tracking
    #[default]
    None,
    /// X10 mouse tracking (button press only)
    X10,
    /// Normal tracking (press and release)
    Normal,
    /// Button event tracking (press, release, motion with button)
    ButtonEvent,
    /// Any event tracking (all motion)
    AnyEvent,
}

/// Mouse encoding format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum MouseEncoding {
    /// Default X10 encoding (limited to 223 cols/rows)
    #[default]
    X10,
    /// UTF-8 encoding
    Utf8,
    /// SGR extended encoding (CSI < ... M/m)
    Sgr,
    /// URXVT encoding
    Urxvt,
}

/// Terminal modes that affect behavior.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct TerminalModes {
    /// Application cursor keys (DECCKM)
    pub application_cursor_keys: bool,
    /// Bracketed paste mode
    pub bracketed_paste: bool,
    /// Focus reporting
    pub focus_reporting: bool,
    /// Auto-wrap at end of line
    pub auto_wrap: bool,
    /// Origin mode (cursor relative to scroll region)
    pub origin_mode: bool,
    /// Insert mode (insert vs replace)
    pub insert_mode: bool,
    /// Mouse tracking mode
    pub mouse_mode: MouseMode,
    /// Mouse encoding format
    pub mouse_encoding: MouseEncoding,
    /// Reverse video mode (swap fg/bg colors)
    pub reverse_video: bool,
    /// Kitty keyboard protocol flags
    /// Bits: 0=disambiguate, 1=report event types, 2=report alternate keys,
    ///       3=report all keys as escape, 4=report associated text
    pub kitty_keyboard_flags: u8,
    /// xterm modifyOtherKeys level, 0 (off) to 2 (XTMODKEYS)
    pub modify_other_keys: u8,
    /// Application keypad mode (DECKPAM/DECKPNM — ESC = / ESC >)
    pub application_keypad: bool,
    /// Alternate scroll mode - when enabled, scroll events scroll the alternate screen
    /// instead of being sent as arrow keys
    pub alternate_scroll: bool,
    /// Synchronized update mode - batches screen updates for flicker-free rendering
    pub synchronized_update: bool,
    /// Mode 2031: report light/dark theme changes unasked.
    pub color_scheme_updates: bool,
    /// Line feed/new line mode (LNM, ANSI mode 20): LF, VT and FF also
    /// return the carriage, and Enter should send CR LF.
    pub linefeed_newline: bool,
    /// DECCOLM may switch between 80 and 132 columns (mode 40). Off by
    /// default, so `reset` and `tput init` (which send `CSI ?3 l`) leave
    /// the window alone, as in xterm.
    pub allow_column_mode_switch: bool,
    /// 132-column mode (DECCOLM, mode 3) was last set.
    pub column_mode_132: bool,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            application_cursor_keys: false,
            bracketed_paste: false,
            focus_reporting: false,
            auto_wrap: true,
            origin_mode: false,
            insert_mode: false,
            application_keypad: false,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            reverse_video: false,
            kitty_keyboard_flags: 0,
            modify_other_keys: 0,
            // Enable by default so mouse wheel works in less, vim, etc.
            alternate_scroll: true,
            synchronized_update: false,
            color_scheme_updates: false,
            linefeed_newline: false,
            allow_column_mode_switch: false,
            column_mode_132: false,
        }
    }
}

#[allow(dead_code)]
pub mod kitty_keyboard {
    pub const DISAMBIGUATE: u8 = 1 << 0;
    pub const REPORT_EVENTS: u8 = 1 << 1;
    pub const REPORT_ALTERNATE: u8 = 1 << 2;
    pub const REPORT_ALL: u8 = 1 << 3;
    pub const REPORT_TEXT: u8 = 1 << 4;
}

/// Shell integration state for tracking command execution.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ShellIntegration {
    /// Current working directory (from OSC 7)
    pub working_directory: Option<String>,
    /// The OSC 7 URI as the shell sent it, host included. The host decides
    /// whether the directory exists on this machine, which `working_directory`
    /// no longer says.
    pub working_directory_uri: Option<String>,
    /// Line where current prompt started
    pub prompt_line: Option<u16>,
    /// Line where current command started
    pub command_line: Option<u16>,
    /// Whether a command is currently executing
    pub command_executing: bool,
    /// Last command exit code
    pub last_exit_code: Option<i32>,
    /// Commands started (OSC 133;C) so far; tags the row marks of the
    /// current one (ADR 0019).
    pub command_number: u32,
    /// Column command input started at (OSC 133;B), until output starts.
    pub input_col: Option<u16>,
    /// The current or last command line, read when its output started.
    pub command: Option<String>,
    /// Output the last command left on the row it finished on, before the
    /// cursor, when it did not end in a newline. The prompt that follows
    /// usually shares that row, so it is saved when the command finishes.
    pub output_tail: Option<String>,
}

/// The last command a shell ran, as `last_command` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCommand {
    /// The command line, as typed after the prompt.
    pub command: String,
    /// What it printed; empty when `output_available` is false.
    pub output: String,
    /// Its exit status, once finished, if the shell reported one.
    pub exit_code: Option<i32>,
    /// Still running: `output` is what it has printed so far.
    pub running: bool,
    /// False when the rows holding the output are gone: the screen was
    /// cleared or reset, or they left the scrollback.
    pub output_available: bool,
}

/// How far above the output to look for the command line's row.
const MAX_COMMAND_LINE_ROWS: usize = 256;

/// Terminal state with main and alternate screen buffers.
///
/// This is the top-level terminal emulation state, managing:
/// - Main screen buffer with scrollback
/// - Alternate screen buffer (for full-screen apps)
/// - Terminal modes
/// - Selection state
/// - Shell integration state
/// - Inline images
#[derive(Debug)]
pub struct Terminal {
    /// Program records belong to the PTY, not either screen.
    program_status: crate::program_status::ProgramStatusStore,
    /// Main screen buffer (with scrollback)
    main: Buffer,
    /// Alternate screen buffer (no scrollback)
    alternate: Buffer,
    /// Currently using alternate screen
    using_alternate: bool,
    /// Terminal modes
    modes: TerminalModes,
    /// Current selection (if any)
    selection: Option<Selection>,
    /// Content version of the active grid when the selection was made.
    /// A selection is only valid while the content it refers to is unchanged.
    selection_content_version: u64,
    /// [`Buffer::rewrite_count`] when the selection was made.
    selection_rewrite_count: u64,
    /// Whether the view was scrolled back when the selection started. New
    /// output snaps the view to the bottom, so such a selection cannot be
    /// kept per row.
    selection_started_scrolled: bool,
    /// Window title
    title: String,
    /// Titles saved by XTWINOPS 22, newest last
    title_stack: Vec<String>,
    /// Size of one character cell in pixels, `(width, height)`, as the
    /// embedder last reported it; answers XTWINOPS 14 and 16.
    cell_pixel_size: Option<(u16, u16)>,
    /// The embedder's colours and the application's palette changes
    palette: Palette,
    /// Shell integration state
    shell_integration: ShellIntegration,
    /// Inline images (Sixel, etc.)
    images: ImageStore,
    /// Pending responses to send back to PTY
    pending_responses: Vec<Vec<u8>>,
    /// Hyperlink ID to URI mapping (OSC 8)
    hyperlinks: std::collections::HashMap<u32, String>,
    /// Reverse map so identical URIs share an id
    hyperlink_ids: std::collections::HashMap<String, u32>,
    /// Hyperlink ids in insertion order (front = oldest) for eviction
    hyperlink_order: std::collections::VecDeque<u32>,
    /// Next hyperlink ID to assign
    next_hyperlink_id: u32,
    /// Current hyperlink ID (0 = none, applied to new cells)
    current_hyperlink_id: u32,
    /// Kitty keyboard flags saved by pushes on the active screen; the
    /// current flags are `modes.kitty_keyboard_flags`.
    kitty_keyboard_stack: Vec<u8>,
    /// The other screen's current flags and stack: the main and alternate
    /// screens keep independent keyboard modes (kitty spec).
    inactive_kitty_keyboard: (u8, Vec<u8>),
    /// Pending column mode resize request (132 or 80 columns, DECCOLM)
    pending_column_resize: Option<u16>,
    /// The `TERM` the program in this terminal was started with, which
    /// XTGETTCAP reports as `TN`.
    terminal_name: String,
    /// Dimensions the inactive alternate buffer still has to be resized to.
    /// Reflowing it on every drag frame is wasted work while it is hidden.
    pending_alternate_resize: Option<Dimensions>,
    /// Anchor events not yet taken by the embedder, oldest first.
    anchor_events: std::collections::VecDeque<AnchorEvent>,
    /// `Refused` events in `anchor_events`.
    queued_refusals: usize,
    /// Events were dropped since [`Terminal::take_anchor_overflow`].
    anchor_events_overflowed: bool,
    /// Anchors in the output reserve rows; see
    /// [`Terminal::set_inline_anchors`].
    inline_anchors: bool,
}

impl Terminal {
    /// The authoritative per-terminal program records.
    pub fn program_status(&self) -> &crate::program_status::ProgramStatusStore {
        &self.program_status
    }

    /// A bounded clone for a consumer whose notifications may have been lost.
    pub fn program_status_snapshot(&self) -> crate::program_status::ProgramStatusSnapshot {
        self.program_status.snapshot()
    }

    /// Apply a report validated by the OSC parser.
    pub fn apply_program_status(&mut self, report: crate::program_status::ProgramStatusReport) {
        self.program_status.apply(report);
    }

    /// Apply process-exit cleanup after final output has been parsed.
    pub fn process_exited(&mut self) {
        self.program_status.on_process_exit();
    }

    /// Check every structural invariant of the terminal state.
    ///
    /// Returns a description of the first violation. Nothing in normal
    /// operation calls this; it exists so tests and fuzzers can assert that
    /// arbitrary input never leaves the state inconsistent.
    pub fn check_integrity(&self) -> Result<(), String> {
        self.main
            .check_integrity()
            .map_err(|e| format!("main screen: {e}"))?;
        self.alternate
            .check_integrity()
            .map_err(|e| format!("alternate screen: {e}"))?;
        if self.hyperlinks.len() > MAX_HYPERLINKS {
            return Err(format!(
                "{} hyperlinks stored, limit is {MAX_HYPERLINKS}",
                self.hyperlinks.len()
            ));
        }
        if self.images.len() > super::image::MAX_IMAGE_STORE_COUNT
            || self.images.total_bytes() > super::image::MAX_IMAGE_STORE_BYTES
        {
            return Err(format!(
                "image store holds {} images / {} bytes, beyond its limits",
                self.images.len(),
                self.images.total_bytes()
            ));
        }
        Ok(())
    }

    /// Create a new terminal with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        Self {
            program_status: crate::program_status::ProgramStatusStore::default(),
            main: Buffer::new(dims),
            alternate: Buffer::with_scrollback(dims, 0), // No scrollback for alternate
            using_alternate: false,
            modes: TerminalModes::default(),
            selection: None,
            selection_content_version: 0,
            selection_rewrite_count: 0,
            selection_started_scrolled: false,
            title: String::new(),
            title_stack: Vec::new(),
            cell_pixel_size: None,
            palette: Palette::default(),
            shell_integration: ShellIntegration::default(),
            images: ImageStore::new(),
            pending_responses: Vec::new(),
            hyperlinks: std::collections::HashMap::new(),
            hyperlink_ids: std::collections::HashMap::new(),
            hyperlink_order: std::collections::VecDeque::new(),
            next_hyperlink_id: 1, // 0 means no hyperlink
            current_hyperlink_id: 0,
            kitty_keyboard_stack: Vec::new(),
            inactive_kitty_keyboard: (0, Vec::new()),
            pending_column_resize: None,
            terminal_name: DEFAULT_TERMINAL_NAME.to_string(),
            pending_alternate_resize: None,
            anchor_events: std::collections::VecDeque::new(),
            queued_refusals: 0,
            anchor_events_overflowed: false,
            inline_anchors: false,
        }
    }

    /// Tell the terminal which `TERM` its program was started with, so
    /// XTGETTCAP reports that name.
    pub fn set_terminal_name(&mut self, name: impl Into<String>) {
        self.terminal_name = name.into();
    }

    /// The `TERM` this terminal reports; [`DEFAULT_TERMINAL_NAME`] unless
    /// [`Terminal::set_terminal_name`] changed it.
    pub fn terminal_name(&self) -> &str {
        &self.terminal_name
    }

    /// Queue a response to send back to the PTY.
    pub fn queue_response(&mut self, response: Vec<u8>) {
        self.pending_responses.push(response);
    }

    /// Take all pending responses (clears the queue).
    pub fn take_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_responses)
    }

    /// Check if there are pending responses.
    pub fn has_pending_responses(&self) -> bool {
        !self.pending_responses.is_empty()
    }

    /// Whether inline surface anchors in the output reserve rows (ADR
    /// 0020). Off by default: until the embedder pairs anchors with
    /// sessions and releases them, anchors in output (a `cat`ed recording,
    /// say) are parsed and ignored.
    pub fn set_inline_anchors(&mut self, enabled: bool) {
        self.inline_anchors = enabled;
    }

    /// Whether inline surface anchors reserve rows; see
    /// [`Terminal::set_inline_anchors`].
    pub fn inline_anchors(&self) -> bool {
        self.inline_anchors
    }

    /// Apply an inline surface anchor at the cursor (ADR 0020). On the main
    /// screen its rows are reserved and tagged; on the alternate screen
    /// nothing is. Either way an [`AnchorEvent`] says what happened. With
    /// inline anchors off this does nothing.
    pub fn place_anchor(&mut self, anchor: Anchor) {
        if !self.inline_anchors {
            return;
        }
        // Free the tags of anchors already gone, so they can be reused.
        self.sweep_anchors();
        let event = if self.using_alternate {
            AnchorEvent::Refused {
                anchor,
                reason: AnchorRefusal::AlternateScreen,
            }
        } else {
            match self.main.place_anchor(anchor) {
                Ok((tag, serial)) => AnchorEvent::Placed {
                    tag,
                    serial,
                    anchor,
                },
                Err(reason) => AnchorEvent::Refused { anchor, reason },
            }
        };
        tracing::trace!(?event, "inline anchor");
        self.push_anchor_event(event);
    }

    /// Anchor events since the last call, oldest first. Anchors whose first
    /// row is gone are found now, so call this after feeding output.
    pub fn take_anchor_events(&mut self) -> Vec<AnchorEvent> {
        self.sweep_anchors();
        self.queued_refusals = 0;
        self.anchor_events.drain(..).collect()
    }

    /// Drop anchor `tag` from the main screen and take its tag off its
    /// rows, leaving their text: the surface is closed, refused or never
    /// claimed (ADR 0020). Returns the anchor if it was live.
    pub fn release_anchor(&mut self, tag: u16) -> Option<Anchor> {
        self.main.release_anchor(tag)
    }

    fn sweep_anchors(&mut self) {
        for (tag, serial, anchor) in self.main.sweep_anchors() {
            self.push_anchor_event(AnchorEvent::Lost {
                tag,
                serial,
                anchor,
            });
        }
    }

    /// Whether anchor events were lost since the last call because more
    /// than [`MAX_ANCHOR_EVENTS`] were queued. The embedder then reconciles
    /// with [`Buffer::anchors`] of the [`Terminal::main_buffer`] instead of
    /// trusting the events it got.
    pub fn take_anchor_overflow(&mut self) -> bool {
        std::mem::take(&mut self.anchor_events_overflowed)
    }

    fn push_anchor_event(&mut self, event: AnchorEvent) {
        if matches!(event, AnchorEvent::Refused { .. }) {
            // A refused anchor holds nothing, so losing word of it costs at
            // most a waiting surface its timeout; a flood of them must not
            // push out the events that do hold tags.
            if self.queued_refusals >= MAX_QUEUED_REFUSALS {
                return;
            }
            self.queued_refusals += 1;
        }
        if self.anchor_events.len() >= MAX_ANCHOR_EVENTS {
            if let Some(AnchorEvent::Refused { .. }) = self.anchor_events.pop_front() {
                self.queued_refusals -= 1;
            }
            self.anchor_events_overflowed = true;
        }
        self.anchor_events.push_back(event);
    }

    /// Keep at most `lines` lines of main-screen scrollback, dropping the
    /// oldest lines beyond it. The alternate screen has no scrollback.
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        if self.main.set_max_history(lines) > 0 && !self.using_alternate {
            // Rows the selection refers to may be gone or renumbered.
            self.clear_selection();
        }
    }

    /// Get the active buffer.
    pub fn buffer(&self) -> &Buffer {
        if self.using_alternate {
            &self.alternate
        } else {
            &self.main
        }
    }

    /// The main screen's buffer, whichever screen is shown. Inline surface
    /// anchors live there.
    pub fn main_buffer(&self) -> &Buffer {
        &self.main
    }

    /// Mutable access to the main screen's buffer, whichever screen is
    /// shown, for writing an inline surface into its rows (ADR 0020).
    pub fn main_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.main
    }

    /// Get mutable access to the active buffer.
    ///
    /// This does not touch the selection: cursor moves and viewport scrolling
    /// keep it, and content mutations invalidate it through the grid's
    /// content version (see [`Grid::content_version`]).
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        if self.using_alternate {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }

    /// Get the active grid.
    pub fn grid(&self) -> &Grid {
        self.buffer().grid()
    }

    /// Get mutable access to the active grid.
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.buffer_mut().grid_mut()
    }

    /// Get terminal dimensions.
    pub fn dimensions(&self) -> Dimensions {
        self.main.dimensions()
    }

    /// Check if using alternate screen.
    pub fn is_alternate_screen(&self) -> bool {
        self.using_alternate
    }

    /// Enter alternate screen buffer (mode 47 — no clear).
    pub fn enter_alternate_screen(&mut self) {
        if !self.using_alternate {
            self.apply_pending_alternate_resize();
            self.using_alternate = true;
            self.swap_kitty_keyboard_state();
            self.selection = None;
        }
    }

    /// Enter alternate screen buffer and clear it (modes 1047, 1049).
    pub fn enter_alternate_screen_clear(&mut self) {
        if !self.using_alternate {
            self.apply_pending_alternate_resize();
            self.using_alternate = true;
            self.swap_kitty_keyboard_state();
            self.alternate.grid_mut().clear();
            self.alternate.grid_mut().cursor_mut().position = CursorPosition::origin();
            self.selection = None;
        }
    }

    /// Leave alternate screen buffer.
    pub fn leave_alternate_screen(&mut self) {
        if self.using_alternate {
            self.using_alternate = false;
            self.swap_kitty_keyboard_state();
            self.selection = None;
        }
    }

    /// Get terminal modes.
    pub fn modes(&self) -> &TerminalModes {
        &self.modes
    }

    /// Get mutable terminal modes.
    pub fn modes_mut(&mut self) -> &mut TerminalModes {
        &mut self.modes
    }

    /// Enable or disable grapheme cluster processing (mode 2027) on both
    /// the main and alternate screens.
    pub fn set_grapheme_clustering(&mut self, enabled: bool) {
        self.main.grid_mut().set_grapheme_clustering(enabled);
        self.alternate.grid_mut().set_grapheme_clustering(enabled);
    }

    /// Get current selection.
    ///
    /// Returns `None` once a row the selection covers has been mutated (text
    /// written, scrolled, erased, or resized) since the selection was made.
    /// Changes to other rows, such as a spinner or status line an app keeps
    /// repainting, leave it alone. A selection made or viewed while scrolled
    /// back is dropped by any change, because new output moves the view;
    /// rows rewritten in place (an inline surface written into its rows)
    /// do not move it, so they drop only a selection that covers them.
    pub fn selection(&self) -> Option<&Selection> {
        let selection = self.selection.as_ref()?;
        let grid = self.grid();
        let (start, end) = selection.ordered();
        let max_row = i32::from(grid.dimensions().rows) - 1;
        let on_screen = end.row >= 0 && start.row <= max_row;
        let first = start.row.clamp(0, max_row.max(0)) as u16;
        let last = end.row.clamp(0, max_row.max(0)) as u16;
        if self.rows_rewritten_since_selection(start.row, end.row) {
            return None;
        }
        if self.selection_content_version == grid.content_version() {
            return Some(selection);
        }
        if self.selection_started_scrolled || self.buffer().is_scrolled() {
            return None;
        }
        if !on_screen {
            return Some(selection);
        }
        if grid.rows_changed_since(self.selection_content_version, first..=last) {
            None
        } else {
            Some(selection)
        }
    }

    /// Whether any row the selection covers, visual rows `first..=last`
    /// counted from the top of the view (negative above it, past the
    /// screen below it), was rewritten in place since it was made.
    fn rows_rewritten_since_selection(&self, first: i32, last: i32) -> bool {
        let buffer = self.buffer();
        let top = buffer.history_len().saturating_sub(buffer.scroll_offset()) as i64;
        let line = |row: i32| (top + i64::from(row)).max(0) as usize;
        let (first, last) = (line(first), line(last));
        let end = buffer.total_lines().saturating_sub(1);
        first <= end
            && buffer.rows_rewritten_since(self.selection_rewrite_count, first..=last.min(end))
    }

    /// Start a new selection.
    pub fn start_selection(&mut self, pos: SelectionPoint, mode: SelectionMode) {
        self.selection = Some(Selection {
            start: pos,
            end: pos,
            mode,
        });
        self.selection_content_version = self.grid().content_version();
        self.selection_rewrite_count = self.buffer().rewrite_count();
        self.selection_started_scrolled = self.buffer().is_scrolled();
    }

    /// Update selection end point.
    ///
    /// A selection invalidated by content changes is dropped instead.
    pub fn update_selection(&mut self, end: SelectionPoint) {
        if self.selection().is_none() {
            self.selection = None;
            return;
        }
        if let Some(sel) = &mut self.selection {
            sel.end = end;
        }
    }

    /// Clear selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Get selected text.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let (start, end) = sel.ordered();

        let mut text = String::new();
        let buffer = self.buffer();
        let dims = buffer.dimensions();
        let max_col = dims.cols as i32 - 1;
        let max_row = dims.rows as i32 - 1;

        // Clamp row range to valid grid bounds
        let first_row = start.row.max(0);
        let last_row = end.row.min(max_row);

        match sel.mode {
            SelectionMode::Normal => {
                // Character-wise selection
                for row_idx in first_row..=last_row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        let start_col = if row_idx == start.row {
                            start.col.max(0)
                        } else {
                            0
                        };
                        let end_col = if row_idx == end.row {
                            end.col.min(max_col)
                        } else {
                            max_col
                        };

                        if start_col <= end_col {
                            text.push_str(&row.copy_text(start_col as u16, end_col as u16));
                        }

                        // Add newline between rows (except last)
                        if row_idx < last_row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Line => {
                // Line-wise selection
                for row_idx in first_row..=last_row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        text.push_str(row.copy_text(0, row.width()).trim_end());
                        if row_idx < last_row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Block => {
                // Block/rectangular selection
                let (left, right) = if start.col <= end.col {
                    (start.col.max(0), end.col.min(max_col))
                } else {
                    (end.col.max(0), start.col.min(max_col))
                };

                for row_idx in first_row..=last_row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        if left <= right {
                            text.push_str(&row.copy_text(left as u16, right as u16));
                        }
                        if row_idx < last_row {
                            text.push('\n');
                        }
                    }
                }
            }
        }

        // Trim trailing whitespace from each line
        let trimmed: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
        Some(trimmed.join("\n"))
    }

    /// Check if a cell is within the selection.
    pub fn is_selected(&self, col: u16, row: u16) -> bool {
        self.selection().is_some_and(|sel| sel.contains(col, row))
    }

    /// Get window title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set window title.
    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Save the title on the title stack (XTWINOPS 22).
    pub fn push_title(&mut self) {
        if self.title_stack.len() >= MAX_TITLE_STACK {
            self.title_stack.remove(0);
        }
        self.title_stack.push(self.title.clone());
    }

    /// Restore the most recently saved title (XTWINOPS 23). Returns whether
    /// there was one.
    pub fn pop_title(&mut self) -> bool {
        match self.title_stack.pop() {
            Some(title) => {
                self.title = title;
                true
            }
            None => false,
        }
    }

    /// Tell the terminal how big a character cell is on screen, in pixels,
    /// so it can answer the XTWINOPS size reports image tools ask for.
    pub fn set_cell_pixel_size(&mut self, width: u16, height: u16) {
        self.cell_pixel_size = Some((width, height));
    }

    /// Size of a character cell in pixels, `(width, height)`, if known.
    pub fn cell_pixel_size(&self) -> Option<(u16, u16)> {
        self.cell_pixel_size
    }

    /// Tell the terminal which colours it is drawn with, so colour queries
    /// report the live theme. Palette changes applications made stay.
    ///
    /// With mode 2031 set, a switch between a dark and a light theme queues
    /// the same report `CSI ? 996 n` asks for.
    pub fn set_theme_colors(&mut self, theme: ThemeColors) {
        let was_dark = self.is_dark();
        self.palette.set_theme(theme);
        if self.modes.color_scheme_updates && self.is_dark() != was_dark {
            self.queue_response(self.color_scheme_report());
        }
    }

    /// Whether the theme's background is dark (relative luminance below
    /// one half).
    pub fn is_dark(&self) -> bool {
        let [r, g, b] = self
            .palette
            .theme()
            .background
            .map(|c| f32::from(c) / 255.0);
        0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5
    }

    /// The answer to `CSI ? 996 n`: `CSI ? 997 ; 1 n` for a dark theme,
    /// `CSI ? 997 ; 2 n` for a light one.
    pub fn color_scheme_report(&self) -> Vec<u8> {
        if self.is_dark() {
            b"\x1b[?997;1n".to_vec()
        } else {
            b"\x1b[?997;2n".to_vec()
        }
    }

    /// The palette: theme colours and the entries applications changed
    /// with OSC 4. Renderers draw indexed colours through
    /// [`Palette::override_for`].
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Mutable palette, for OSC 4 / 104.
    pub fn palette_mut(&mut self) -> &mut Palette {
        &mut self.palette
    }

    /// Get shell integration state.
    pub fn shell_integration(&self) -> &ShellIntegration {
        &self.shell_integration
    }

    /// Get the working directory (from OSC 7).
    pub fn working_directory(&self) -> Option<&str> {
        self.shell_integration.working_directory.as_deref()
    }

    /// The last OSC 7 value exactly as received (`file://host/path`).
    pub fn working_directory_uri(&self) -> Option<&str> {
        self.shell_integration.working_directory_uri.as_deref()
    }

    /// Set working directory (from OSC 7 file://host/path).
    pub fn set_working_directory(&mut self, uri: String) {
        self.shell_integration.working_directory_uri = Some(uri.clone());
        // Parse file:// URI to extract path
        let path = if let Some(stripped) = uri.strip_prefix("file://") {
            // Skip file://hostname/ prefix
            if let Some(path_start) = stripped.find('/') {
                stripped[path_start..].to_string()
            } else {
                stripped.to_string()
            }
        } else {
            uri
        };

        // URL decode the path
        let decoded = percent_decode(&path);
        self.shell_integration.working_directory = Some(decoded);
        tracing::debug!(
            "Working directory set to: {:?}",
            self.shell_integration.working_directory
        );
    }

    /// Mark prompt start (OSC 133;A).
    pub fn mark_prompt_start(&mut self) {
        self.program_status.on_prompt();
        let row = self.grid().cursor().position.row;
        self.shell_integration.prompt_line = Some(row);
        self.shell_integration.command_executing = false;
        tracing::trace!("Prompt started at line {}", row);
    }

    /// Mark command start (OSC 133;B): input begins at the cursor.
    pub fn mark_command_start(&mut self) {
        let position = self.grid().cursor().position;
        self.shell_integration.command_line = Some(position.row);
        self.shell_integration.input_col = Some(position.col);
        let tag = RowMarks::tag(self.shell_integration.command_number.wrapping_add(1));
        self.update_cursor_row_marks(|marks| marks.input_start = tag);
        tracing::trace!("Command started at line {}", position.row);
    }

    /// Mark command executed (OSC 133;C): output begins at the cursor's
    /// row. Reads the command line typed since OSC 133;B.
    pub fn mark_command_executed(&mut self) {
        let shell = &mut self.shell_integration;
        shell.command_number = shell.command_number.wrapping_add(1);
        shell.command_executing = true;
        shell.last_exit_code = None;
        shell.output_tail = None;
        let tag = RowMarks::tag(shell.command_number);
        let input_col = shell.input_col.take();
        self.shell_integration.command = Some(self.command_line_before_cursor(tag, input_col));
        self.update_cursor_row_marks(|marks| marks.output_start = tag);
        tracing::trace!("Command executing");
    }

    /// Mark command finished (OSC 133;D). A D with no command running (the
    /// one shells send before their first prompt) marks nothing.
    pub fn mark_command_finished(&mut self, exit_code: Option<i32>) {
        if self.shell_integration.command_executing {
            let tag = RowMarks::tag(self.shell_integration.command_number);
            self.update_cursor_row_marks(|marks| marks.command_end = tag);
            let cursor = self.grid().cursor().position;
            self.shell_integration.output_tail = (cursor.col > 0)
                .then(|| self.grid().row(cursor.row))
                .flatten()
                .map(|row| row.copy_text(0, cursor.col - 1).trim_end().to_string())
                .filter(|tail| !tail.is_empty());
        }
        self.shell_integration.command_executing = false;
        self.shell_integration.last_exit_code = exit_code;
        tracing::trace!("Command finished with exit code: {:?}", exit_code);
    }

    fn update_cursor_row_marks(&mut self, update: impl FnOnce(&mut RowMarks)) {
        let row = self.grid().cursor().position.row;
        self.grid_mut().update_row_marks(row, update);
    }

    /// The command line typed after the prompt: from the row marked as
    /// input start for `tag` (at `input_col`) up to the cursor's row.
    fn command_line_before_cursor(&self, tag: u8, input_col: Option<u16>) -> String {
        let buffer = self.buffer();
        let cursor = buffer.history_len() + self.grid().cursor().position.row as usize;
        let input_row = (cursor.saturating_sub(MAX_COMMAND_LINE_ROWS)..=cursor)
            .rev()
            .find(|&index| buffer.marks_at(index).is_some_and(|m| m.input_start == tag));
        match (input_row, input_col) {
            (Some(row), Some(col)) => buffer.text_of_rows(row, cursor.max(row + 1), col),
            _ => String::new(),
        }
    }

    /// The main screen's absolute rows `start..end` that hold the last
    /// command's output, from the row its OSC 133;C mark is on to the row its
    /// OSC 133;D mark is on (a command still running has no end yet, so
    /// that is the end of the screen). `None` when there is no command or its
    /// marks have left the scrollback.
    fn last_output_rows(&self) -> Option<(usize, usize)> {
        let shell = &self.shell_integration;
        if shell.command_number == 0 {
            return None;
        }
        let tag = RowMarks::tag(shell.command_number);
        let buffer = &self.main;
        let total = buffer.total_lines();
        let start = (0..total).rev().find(|&index| {
            buffer
                .marks_at(index)
                .is_some_and(|m| m.output_start == tag)
        })?;
        let end = (start..total)
            .find(|&index| buffer.marks_at(index).is_some_and(|m| m.command_end == tag));
        match end {
            Some(end) => Some((start, end)),
            None if shell.command_executing => Some((start, total)),
            None => None,
        }
    }

    /// Scroll the view to the previous (`Direction::Back`) or next
    /// (`Direction::Forward`) shell prompt, found through the OSC 133;B marks
    /// on the main screen's rows. The prompt lands on the top row of the
    /// view, except that one in the live screen cannot go higher than where
    /// it is, so the view then goes to the bottom. Going forward past the
    /// last prompt goes to the bottom. Returns whether the view moved. Does
    /// nothing on the alternate screen, which has no scrollback.
    pub fn jump_to_prompt(&mut self, direction: PromptJump) -> bool {
        if self.using_alternate {
            return false;
        }
        let before = self.main.scroll_offset();
        match direction {
            PromptJump::Previous => {
                if let Some(row) = self.main.prompt_row_above_view() {
                    self.main.scroll_to_row(row);
                }
            }
            PromptJump::Next => match self.main.prompt_row_below_view() {
                Some(row) => self.main.scroll_to_row(row),
                None => self.main.scroll_to_bottom(),
            },
        }
        self.main.scroll_offset() != before
    }

    /// Select, line by line, the output of the last command, if all of it
    /// fits on screen: the view scrolls to show it, and the selection covers
    /// the rows from the one its output started on to the one before its
    /// OSC 133;D mark. A command that has not finished is selected as far as
    /// the cursor. Returns whether a selection was made. It is not when
    /// there is no command, its marks have left the scrollback, its output
    /// is empty or taller than the view, or the alternate screen is shown.
    ///
    /// Selections are in view rows, so longer output cannot be selected;
    /// `ulttyctl last-command` reads it whole.
    pub fn select_last_output(&mut self) -> bool {
        if self.using_alternate {
            return false;
        }
        let Some((start, mut end)) = self.last_output_rows() else {
            return false;
        };
        if self.shell_integration.command_executing {
            let cursor = self.main.history_len() + self.main.grid().cursor().position.row as usize;
            end = end.min(cursor + 1);
        }
        let view_rows = usize::from(self.main.dimensions().rows);
        if end <= start || end - start > view_rows {
            return false;
        }
        let top = self.main.top_row();
        if start < top || end > top + view_rows {
            self.main.scroll_to_row(start);
        }
        let top = self.main.top_row() as i32;
        let (first, last) = (start as i32 - top, end as i32 - 1 - top);
        self.start_selection(SelectionPoint::new(0, first), SelectionMode::Line);
        self.update_selection(SelectionPoint::new(0, last));
        true
    }

    /// The last command the shell ran (or is running), found through the
    /// OSC 133 marks on the main screen's rows (ADR 0019). `None` before
    /// the first command. `tail_lines` keeps only the end of long output.
    pub fn last_command(&self, tail_lines: Option<usize>) -> Option<LastCommand> {
        let shell = &self.shell_integration;
        if shell.command_number == 0 {
            return None;
        }
        let buffer = &self.main;
        let rows = self.last_output_rows();
        let running = shell.command_executing;
        let mut output = rows
            .map(|(start, end)| buffer.text_of_rows(start, end, 0))
            .unwrap_or_default();
        if let (Some(_), false, Some(tail)) = (rows, running, &shell.output_tail) {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(tail);
        }
        if let Some(tail) = tail_lines {
            let lines: Vec<&str> = output.lines().collect();
            output = lines[lines.len().saturating_sub(tail)..].join("\n");
        }
        Some(LastCommand {
            command: shell.command.clone().unwrap_or_default(),
            output,
            exit_code: if running { None } else { shell.last_exit_code },
            running,
            output_available: rows.is_some(),
        })
    }

    /// Get the image store.
    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    /// Get mutable access to the image store.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.images
    }

    // === Hyperlink support (OSC 8) ===

    /// Start a hyperlink with the given URI.
    ///
    /// All subsequent characters will be associated with this hyperlink
    /// until `end_hyperlink` is called.
    pub fn start_hyperlink(&mut self, uri: String) {
        if let Some(&id) = self.hyperlink_ids.get(&uri) {
            self.current_hyperlink_id = id;
            return;
        }

        while self.hyperlink_order.len() >= MAX_HYPERLINKS {
            let Some(oldest) = self.hyperlink_order.pop_front() else {
                break;
            };
            if let Some(old_uri) = self.hyperlinks.remove(&oldest) {
                self.hyperlink_ids.remove(&old_uri);
            }
        }

        let id = self.next_hyperlink_id;
        self.next_hyperlink_id = self.next_hyperlink_id.wrapping_add(1);
        if self.next_hyperlink_id == 0 {
            self.next_hyperlink_id = 1; // Skip 0, which means no hyperlink
        }
        self.hyperlinks.insert(id, uri.clone());
        self.hyperlink_ids.insert(uri, id);
        self.hyperlink_order.push_back(id);
        self.current_hyperlink_id = id;
    }

    /// End the current hyperlink.
    pub fn end_hyperlink(&mut self) {
        self.current_hyperlink_id = 0;
    }

    /// Get the current hyperlink ID to apply to new cells.
    pub fn current_hyperlink_id(&self) -> u32 {
        self.current_hyperlink_id
    }

    /// Look up a hyperlink URI by ID.
    pub fn hyperlink(&self, id: u32) -> Option<&str> {
        self.hyperlinks.get(&id).map(|s| s.as_str())
    }

    /// Get hyperlink at a specific cell position.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<&str> {
        let cell = self.buffer().grid().cell(col, row)?;
        let id = cell.hyperlink_id();
        if id != 0 {
            self.hyperlink(id)
        } else {
            None
        }
    }

    /// Get all hyperlinks (for URL detection).
    pub fn all_hyperlinks(&self) -> &std::collections::HashMap<u32, String> {
        &self.hyperlinks
    }

    /// Push the current Kitty keyboard flags and switch to `flags`
    /// (`CSI > flags u`). The stack keeps the newest
    /// [`KITTY_KEYBOARD_STACK_LIMIT`] entries.
    pub fn push_kitty_keyboard_mode(&mut self, flags: u8) {
        if self.kitty_keyboard_stack.len() == KITTY_KEYBOARD_STACK_LIMIT {
            self.kitty_keyboard_stack.remove(0);
        }
        self.kitty_keyboard_stack
            .push(self.modes.kitty_keyboard_flags);
        self.modes.kitty_keyboard_flags = flags;
        tracing::debug!("Pushed Kitty keyboard mode: flags={}", flags);
    }

    /// Pop `count` entries (`CSI < count u`), restoring the flags saved by
    /// the matching push. Popping more than was pushed empties the stack
    /// and turns the protocol off, so a program that set flags without
    /// pushing still leaves the shell with legacy keys.
    pub fn pop_kitty_keyboard_modes(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            match self.kitty_keyboard_stack.pop() {
                Some(flags) => self.modes.kitty_keyboard_flags = flags,
                None => {
                    self.modes.kitty_keyboard_flags = 0;
                    break;
                }
            }
        }
        tracing::debug!(
            "Popped Kitty keyboard mode: flags={}",
            self.modes.kitty_keyboard_flags
        );
    }

    /// Swap the active screen's Kitty keyboard state with the other screen's.
    fn swap_kitty_keyboard_state(&mut self) {
        std::mem::swap(
            &mut self.modes.kitty_keyboard_flags,
            &mut self.inactive_kitty_keyboard.0,
        );
        std::mem::swap(
            &mut self.kitty_keyboard_stack,
            &mut self.inactive_kitty_keyboard.1,
        );
    }

    /// Change the current Kitty keyboard flags in place (`CSI = flags ; mode u`):
    /// mode 1 replaces them, 2 sets the given bits, 3 clears them. Other
    /// modes are ignored. The stack is left alone.
    pub fn set_kitty_keyboard_mode(&mut self, flags: u8, mode: u8) {
        let current = self.modes.kitty_keyboard_flags;
        let new = match mode {
            1 => flags,
            2 => current | flags,
            3 => current & !flags,
            _ => return,
        };
        self.modes.kitty_keyboard_flags = new;
        tracing::debug!("Set Kitty keyboard mode: flags={new}");
    }

    /// Check if Kitty keyboard protocol is active.
    pub fn kitty_keyboard_active(&self) -> bool {
        self.modes.kitty_keyboard_flags != 0
    }

    /// Add a Sixel image at the current cursor position.
    ///
    /// The image will be placed starting at the cursor position.
    /// The cursor advances past the image after placement.
    pub fn add_image(&mut self, image: TerminalImage) {
        let cell_width = image.cell_width;
        let cell_height = image.cell_height;
        let start_col = image.start_col;

        self.images.add(image);

        let dims = self.dimensions();
        let cursor = self.grid_mut().cursor_mut();

        if cell_height <= 1 {
            let new_col = cursor.position.col.saturating_add(cell_width);
            cursor.position.col = new_col.min(dims.cols.saturating_sub(1));
        } else {
            let new_row = cursor
                .position
                .row
                .saturating_add(cell_height.saturating_sub(1));
            if new_row >= dims.rows {
                cursor.position.row = dims.rows.saturating_sub(1);
            } else {
                cursor.position.row = new_row;
            }
            cursor.position.col = start_col;
        }

        tracing::debug!(
            "Added image: {}x{} cells, cursor now at ({}, {})",
            cell_width,
            cell_height,
            cursor.position.col,
            cursor.position.row
        );
    }

    /// Resize the terminal.
    ///
    /// The visible buffer is reflowed now; a hidden alternate buffer is
    /// resized lazily when it next becomes active.
    pub fn resize(&mut self, new_dims: Dimensions) {
        self.main.resize(new_dims);
        if self.using_alternate {
            self.alternate.resize(new_dims);
            self.pending_alternate_resize = None;
        } else {
            self.pending_alternate_resize = Some(new_dims);
        }
        self.selection = None;
    }

    fn apply_pending_alternate_resize(&mut self) {
        if let Some(dims) = self.pending_alternate_resize.take() {
            self.alternate.resize(dims);
        }
    }

    /// Drop everything a hard reset (RIS) must forget besides the active
    /// grid: the alternate screen, scrollback, images, hyperlinks, the
    /// selection and any pushed keyboard modes.
    pub fn reset_auxiliary_state(&mut self) {
        self.program_status.on_ris();
        self.apply_pending_alternate_resize();
        self.alternate.grid_mut().clear();
        self.alternate.grid_mut().cursor_mut().position = CursorPosition::origin();
        self.alternate.clear_history();
        self.main.clear_history();
        // The main grid keeps its text when the reset came on the
        // alternate screen, but no anchor survives a reset.
        for (tag, serial, anchor) in self.main.drop_all_anchors() {
            self.push_anchor_event(AnchorEvent::Lost {
                tag,
                serial,
                anchor,
            });
        }
        self.images.clear();
        self.hyperlinks.clear();
        self.hyperlink_ids.clear();
        self.hyperlink_order.clear();
        self.current_hyperlink_id = 0;
        self.selection = None;
        self.kitty_keyboard_stack.clear();
        self.inactive_kitty_keyboard = (0, Vec::new());
        self.title_stack.clear();
        self.palette.reset(None);
        self.pending_column_resize = None;
    }

    /// Request a column mode resize (DECCOLM - 132 or 80 columns).
    ///
    /// This sets a pending resize request that should be consumed by the
    /// application layer to resize the window/terminal accordingly.
    pub fn request_column_resize(&mut self, columns: u16) {
        self.pending_column_resize = Some(columns);
        tracing::debug!("Requested column resize to {} columns", columns);
    }

    /// Take the pending column resize request, if any.
    ///
    /// Returns the requested column count (132 or 80) and clears the request.
    pub fn take_column_resize_request(&mut self) -> Option<u16> {
        self.pending_column_resize.take()
    }

    /// Check if there's a pending column resize request.
    pub fn has_column_resize_request(&self) -> bool {
        self.pending_column_resize.is_some()
    }

    // === Damage tracking ===

    /// Check if the active buffer has any dirty content.
    pub fn has_dirty(&self) -> bool {
        self.buffer().has_dirty()
    }

    /// Clear dirty flags after rendering.
    pub fn clear_dirty(&mut self) {
        if self.using_alternate {
            self.alternate.clear_dirty();
        } else {
            self.main.clear_dirty();
        }
    }

    /// Check if mouse tracking is enabled.
    pub fn mouse_tracking_enabled(&self) -> bool {
        self.modes.mouse_mode != MouseMode::None
    }

    /// Check if motion events should be tracked.
    pub fn track_mouse_motion(&self) -> bool {
        matches!(
            self.modes.mouse_mode,
            MouseMode::ButtonEvent | MouseMode::AnyEvent
        )
    }

    /// Generate mouse event bytes for the terminal.
    ///
    /// Returns None if mouse tracking is disabled.
    ///
    /// The `modifiers` parameter allows including keyboard modifier state
    /// (shift, alt, ctrl, meta) in the mouse report, which is used by
    /// applications like neovim for shift+click functionality.
    pub fn encode_mouse_event(
        &self,
        button: MouseButton,
        x: u16,
        y: u16,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> Option<Vec<u8>> {
        if self.modes.mouse_mode == MouseMode::None {
            return None;
        }

        // X10 mode only reports button presses
        if self.modes.mouse_mode == MouseMode::X10 && !pressed {
            return None;
        }

        // Motion is reported in button-event mode only while a button is
        // held (a drag), and in any-event mode always. `Release` with motion
        // means no button is held.
        if motion {
            let button_held = pressed && button != MouseButton::Release;
            match self.modes.mouse_mode {
                MouseMode::AnyEvent => {}
                MouseMode::ButtonEvent if button_held => {}
                _ => return None,
            }
        }

        // Encode based on format
        match self.modes.mouse_encoding {
            MouseEncoding::Sgr => {
                Some(self.encode_mouse_sgr(button, x, y, pressed, motion, modifiers))
            }
            MouseEncoding::X10 => {
                Some(self.encode_mouse_x10(button, x, y, pressed, motion, modifiers))
            }
            MouseEncoding::Utf8 => {
                Some(self.encode_mouse_utf8(button, x, y, pressed, motion, modifiers))
            }
            MouseEncoding::Urxvt => {
                Some(self.encode_mouse_urxvt(button, x, y, pressed, motion, modifiers))
            }
        }
    }

    /// Encode mouse event in SGR format (CSI < Cb ; Cx ; Cy M/m).
    fn encode_mouse_sgr(
        &self,
        button: MouseButton,
        x: u16,
        y: u16,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> Vec<u8> {
        let mut cb = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Release => 3,
            MouseButton::ScrollUp => 64,
            MouseButton::ScrollDown => 65,
        };

        // Add modifier bits: Shift=4, Meta/Alt=8, Control=16
        cb += modifiers.bits();

        if motion {
            cb += 32;
        }

        let suffix = if pressed || matches!(button, MouseButton::ScrollUp | MouseButton::ScrollDown)
        {
            'M'
        } else {
            'm'
        };

        format!("\x1b[<{};{};{}{}", cb, x + 1, y + 1, suffix).into_bytes()
    }

    /// The button code shared by the X10 and UTF-8 encodings: a release
    /// reports button 3, as those encodings cannot say which was released.
    fn mouse_button_code(
        &self,
        button: MouseButton,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> u8 {
        let mut cb = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Release => 3,
            MouseButton::ScrollUp => 64,
            MouseButton::ScrollDown => 65,
        };

        // Add modifier bits: Shift=4, Meta/Alt=8, Control=16
        cb += modifiers.bits();

        if motion {
            cb += 32;
        }
        if !pressed && !matches!(button, MouseButton::ScrollUp | MouseButton::ScrollDown) {
            cb = 3 + modifiers.bits(); // Release with modifiers
        }
        cb
    }

    /// Encode mouse event in X10 format.
    fn encode_mouse_x10(
        &self,
        button: MouseButton,
        x: u16,
        y: u16,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> Vec<u8> {
        // Add 32 to make printable, clamp to 223
        let cb = self.mouse_button_code(button, pressed, motion, modifiers) + 32;
        let cx = ((x + 1).min(223) + 32) as u8;
        let cy = ((y + 1).min(223) + 32) as u8;

        vec![b'\x1b', b'[', b'M', cb, cx, cy]
    }

    /// Encode mouse event in the UTF-8 format (mode 1005): the X10 report,
    /// but each value is sent as the UTF-8 encoding of the character with
    /// that code, so coordinates past 223 fit and the bytes stay valid
    /// UTF-8. Values below 128 are the same single bytes as in X10.
    fn encode_mouse_utf8(
        &self,
        button: MouseButton,
        x: u16,
        y: u16,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> Vec<u8> {
        let cb = u32::from(self.mouse_button_code(button, pressed, motion, modifiers));
        let mut out = String::from("\x1b[M");
        for value in [cb + 32, u32::from(x) + 33, u32::from(y) + 33] {
            // Every value up to the cap is a valid, non-surrogate scalar.
            out.extend(char::from_u32(value.min(MAX_UTF8_MOUSE_VALUE)));
        }
        out.into_bytes()
    }

    /// Encode mouse event in URXVT format.
    fn encode_mouse_urxvt(
        &self,
        button: MouseButton,
        x: u16,
        y: u16,
        pressed: bool,
        motion: bool,
        modifiers: MouseModifiers,
    ) -> Vec<u8> {
        let mut cb = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Release => 3,
            MouseButton::ScrollUp => 64,
            MouseButton::ScrollDown => 65,
        };

        // Add modifier bits: Shift=4, Meta/Alt=8, Control=16
        cb += modifiers.bits();

        if motion {
            cb += 32;
        }
        if !pressed && !matches!(button, MouseButton::ScrollUp | MouseButton::ScrollDown) {
            cb = 3 + modifiers.bits(); // Release with modifiers
        }

        format!("\x1b[{};{};{}M", cb + 32, x + 1, y + 1).into_bytes()
    }
}

/// Mouse button for tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Release,
    ScrollUp,
    ScrollDown,
}

/// Mouse modifiers for event encoding.
///
/// These modifiers are included in mouse event reports when held during
/// mouse actions. They add bits to the button code:
/// - Shift: +4
/// - Meta/Alt: +8
/// - Control: +16
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

impl MouseModifiers {
    /// Create new mouse modifiers.
    pub fn new(shift: bool, alt: bool, ctrl: bool, meta: bool) -> Self {
        Self {
            shift,
            alt,
            ctrl,
            meta,
        }
    }

    /// Get the modifier bits to add to the button code.
    pub fn bits(&self) -> u8 {
        let mut bits = 0u8;
        if self.shift {
            bits += 4;
        }
        if self.alt || self.meta {
            bits += 8;
        }
        if self.ctrl {
            bits += 16;
        }
        bits
    }
}

/// Which way [`Terminal::jump_to_prompt`] goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptJump {
    /// Towards older output.
    Previous,
    /// Towards the live screen.
    Next,
}

/// Selection point (can be negative for scroll offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub col: i32,
    pub row: i32,
}

impl SelectionPoint {
    pub fn new(col: i32, row: i32) -> Self {
        Self { col, row }
    }
}

/// Selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Normal character-wise selection
    Normal,
    /// Line-wise selection
    Line,
    /// Block/rectangular selection
    Block,
}

/// Selection state.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Start point of selection
    pub start: SelectionPoint,
    /// End point of selection (current position)
    pub end: SelectionPoint,
    /// Selection mode
    pub mode: SelectionMode,
}

impl Selection {
    /// Get ordered start and end (top-left to bottom-right).
    pub fn ordered(&self) -> (SelectionPoint, SelectionPoint) {
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Check if selection is empty (start == end).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Whether the cell at `col`, `row` (viewport coordinates) is inside
    /// the selection.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        let (start, end) = self.ordered();
        let row = row as i32;
        let col = col as i32;

        match self.mode {
            SelectionMode::Normal => {
                if row < start.row || row > end.row {
                    return false;
                }
                if row == start.row && row == end.row {
                    col >= start.col && col <= end.col
                } else if row == start.row {
                    col >= start.col
                } else if row == end.row {
                    col <= end.col
                } else {
                    true
                }
            }
            SelectionMode::Line => row >= start.row && row <= end.row,
            SelectionMode::Block => {
                let (left, right) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };
                row >= start.row && row <= end.row && col >= left && col <= right
            }
        }
    }
}

/// Simple percent-decoding for file paths (e.g., %20 -> space).
/// Handles multi-byte UTF-8 sequences correctly (e.g., %C3%A9 -> é).
fn percent_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.as_bytes().iter().copied().peekable();

    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                let hex = [h1, h2];
                if let Ok(decoded) = u8::from_str_radix(std::str::from_utf8(&hex).unwrap_or(""), 16)
                {
                    bytes.push(decoded);
                    continue;
                }
                // Invalid hex escape, keep as-is
                bytes.push(b'%');
                bytes.push(h1);
                bytes.push(h2);
            } else {
                // Truncated escape at end of string
                bytes.push(b'%');
                if let Some(h1) = h1 {
                    bytes.push(h1);
                }
            }
        } else {
            bytes.push(b);
        }
    }

    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::ImageId;

    #[test]
    fn new_terminal() {
        let term = Terminal::new(Dimensions::new(80, 24));
        assert_eq!(term.dimensions().cols, 80);
        assert_eq!(term.dimensions().rows, 24);
        assert!(!term.is_alternate_screen());
    }

    #[test]
    fn alternate_screen() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        // Write something to main screen
        term.grid_mut().write_char('M');

        // Enter alternate screen
        term.enter_alternate_screen();
        assert!(term.is_alternate_screen());

        // Alternate should be clear
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), ' ');

        // Write to alternate
        term.grid_mut().write_char('A');
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'A');

        // Return to main
        term.leave_alternate_screen();
        assert!(!term.is_alternate_screen());

        // Main should still have original content
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), 'M');
    }

    #[test]
    fn selection_normal() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write some text
        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }

        // Select "ell"
        term.start_selection(SelectionPoint::new(1, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(3, 0));

        assert!(term.is_selected(1, 0));
        assert!(term.is_selected(2, 0));
        assert!(term.is_selected(3, 0));
        assert!(!term.is_selected(0, 0));
        assert!(!term.is_selected(4, 0));

        assert_eq!(term.selected_text(), Some("ell".to_string()));
    }

    #[test]
    fn selection_multiline() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write text on two lines
        for c in "Line1".chars() {
            term.grid_mut().write_char(c);
        }
        term.grid_mut().carriage_return();
        term.grid_mut().newline();
        for c in "Line2".chars() {
            term.grid_mut().write_char(c);
        }

        // Select from "ne1" to "Lin" of second line
        term.start_selection(SelectionPoint::new(2, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(2, 1));

        let text = term.selected_text().unwrap();
        assert!(text.contains("ne1"));
        assert!(text.contains("Lin"));
    }

    #[test]
    fn selection_line_mode() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }

        // Line selection
        term.start_selection(SelectionPoint::new(2, 0), SelectionMode::Line);
        term.update_selection(SelectionPoint::new(2, 0));

        // Entire line should be selected
        assert!(term.is_selected(0, 0));
        assert!(term.is_selected(5, 0));
        assert!(!term.is_selected(0, 1));

        assert_eq!(term.selected_text(), Some("Hello".to_string()));
    }

    #[test]
    fn selection_block_mode() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        // Write a grid
        for row in 0..3 {
            term.grid_mut().set_cursor_position(row + 1, 1);
            for c in "ABCDE".chars() {
                term.grid_mut().write_char(c);
            }
        }

        // Block select middle column across all rows
        term.start_selection(SelectionPoint::new(1, 0), SelectionMode::Block);
        term.update_selection(SelectionPoint::new(3, 2));

        // Should select columns 1-3 on rows 0-2
        assert!(term.is_selected(1, 0));
        assert!(term.is_selected(2, 1));
        assert!(term.is_selected(3, 2));
        assert!(!term.is_selected(0, 0));
        assert!(!term.is_selected(4, 0));

        let text = term.selected_text().unwrap();
        assert_eq!(text, "BCD\nBCD\nBCD");
    }

    #[test]
    fn selection_survives_viewport_scroll_and_cursor_moves() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        for line in ["one", "two", "three", "four"] {
            for c in line.chars() {
                term.buffer_mut().write_char(c);
            }
            term.grid_mut().carriage_return();
            term.buffer_mut().newline();
        }
        assert!(term.buffer().history_len() > 0);

        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(2, 0));
        assert!(term.selection().is_some());

        term.buffer_mut().scroll_up(1);
        assert!(
            term.selection().is_some(),
            "viewport scroll must keep selection"
        );
        term.buffer_mut().scroll_down(1);

        term.grid_mut().cursor_down(1);
        term.grid_mut().cursor_right(2);
        term.grid_mut().set_cursor_position(1, 1);
        assert!(
            term.selection().is_some(),
            "cursor moves must keep selection"
        );
    }

    #[test]
    fn selection_cleared_when_content_changes() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }
        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(2, 0));
        assert_eq!(term.selected_text().as_deref(), Some("Hel"));

        term.buffer_mut().write_char('x');

        assert!(term.selection().is_none());
        assert!(!term.is_selected(0, 0));
        assert_eq!(term.selected_text(), None);
    }

    #[test]
    fn selection_cleared_by_grid_scroll_and_erase() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.buffer_mut().scroll_grid_up(1);
        assert!(term.selection().is_none());

        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.grid_mut().erase_line();
        assert!(term.selection().is_none());
    }

    #[test]
    fn identical_hyperlink_uris_share_an_id() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        term.start_hyperlink("https://example.com".to_string());
        let first = term.current_hyperlink_id();
        term.end_hyperlink();
        term.start_hyperlink("https://example.com".to_string());
        assert_eq!(term.current_hyperlink_id(), first);
        assert_eq!(term.all_hyperlinks().len(), 1);
    }

    #[test]
    fn hyperlink_map_is_bounded_and_evicts_oldest() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        term.start_hyperlink("https://example.com/0".to_string());
        let oldest = term.current_hyperlink_id();
        for i in 1..(MAX_HYPERLINKS + 10) {
            term.start_hyperlink(format!("https://example.com/{i}"));
        }
        let newest = term.current_hyperlink_id();

        assert_eq!(term.all_hyperlinks().len(), MAX_HYPERLINKS);
        assert!(term.hyperlink(oldest).is_none());
        assert_eq!(
            term.hyperlink(newest),
            Some(format!("https://example.com/{}", MAX_HYPERLINKS + 9).as_str())
        );
        term.end_hyperlink();
        assert_eq!(term.current_hyperlink_id(), 0);
    }

    #[test]
    fn reset_auxiliary_state_clears_alternate_images_and_links() {
        let mut term = Terminal::new(Dimensions::new(10, 3));
        term.enter_alternate_screen();
        term.grid_mut().write_char('Z');
        term.leave_alternate_screen();
        term.start_hyperlink("https://example.com".to_string());
        let image = TerminalImage {
            id: ImageId(0),
            width: 1,
            height: 1,
            data: vec![0; 4],
            start_col: 0,
            start_row: 0,
            cell_width: 1,
            cell_height: 1,
        };
        term.add_image(image);

        term.reset_auxiliary_state();

        assert!(term.images().is_empty());
        assert!(term.all_hyperlinks().is_empty());
        assert_eq!(term.current_hyperlink_id(), 0);
        term.enter_alternate_screen();
        assert_eq!(term.grid().cell(0, 0).unwrap().char(), ' ');
    }

    #[test]
    fn hidden_alternate_buffer_is_resized_when_it_becomes_active() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        term.resize(Dimensions::new(100, 30));
        term.resize(Dimensions::new(60, 20));
        assert_eq!(term.dimensions(), Dimensions::new(60, 20));

        term.enter_alternate_screen_clear();
        assert!(term.is_alternate_screen());
        assert_eq!(term.grid().dimensions(), Dimensions::new(60, 20));
        assert_eq!(term.grid().rows().count(), 20);

        // While active, the alternate buffer is resized eagerly
        term.resize(Dimensions::new(50, 10));
        assert_eq!(term.grid().dimensions(), Dimensions::new(50, 10));
        term.leave_alternate_screen();
        assert_eq!(term.grid().dimensions(), Dimensions::new(50, 10));
    }

    #[test]
    fn resize_clears_selection() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        assert!(term.selection().is_some());

        term.resize(Dimensions::new(100, 30));
        assert!(term.selection().is_none());
    }

    #[test]
    fn add_image_single_line() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        term.grid_mut().set_cursor_position(1, 5);

        let image = TerminalImage {
            id: ImageId(0),
            width: 24,
            height: 16,
            data: vec![0; 24 * 16 * 4],
            start_col: 4,
            start_row: 0,
            cell_width: 3,
            cell_height: 1,
        };
        term.add_image(image);

        assert_eq!(term.grid().cursor().position.col, 7);
        assert_eq!(term.grid().cursor().position.row, 0);
    }

    #[test]
    fn add_image_multi_line() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        term.grid_mut().set_cursor_position(3, 10);

        let image = TerminalImage {
            id: ImageId(0),
            width: 32,
            height: 64,
            data: vec![0; 32 * 64 * 4],
            start_col: 9,
            start_row: 2,
            cell_width: 4,
            cell_height: 4,
        };
        term.add_image(image);

        assert_eq!(term.grid().cursor().position.col, 9);
        assert_eq!(term.grid().cursor().position.row, 5);
    }

    #[test]
    fn selected_text_clamps_negative_coords() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        for c in "Hello".chars() {
            term.grid_mut().write_char(c);
        }

        // Selection starting at negative col/row (simulating scroll offset)
        term.start_selection(SelectionPoint::new(-3, -1), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(4, 0));

        // Should not panic, and should return text from clamped range
        let text = term.selected_text().unwrap();
        assert!(text.contains("Hello"));
    }

    #[test]
    fn selected_text_clamps_beyond_grid() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        for c in "ABC".chars() {
            term.grid_mut().write_char(c);
        }

        // Selection extending past grid bounds
        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        term.update_selection(SelectionPoint::new(100, 100));

        // Should not panic, and should include the written text
        let text = term.selected_text().unwrap();
        assert!(text.contains("ABC"));
    }

    #[test]
    fn selected_text_block_clamps_negative_cols() {
        let mut term = Terminal::new(Dimensions::new(10, 5));

        for c in "XYZ".chars() {
            term.grid_mut().write_char(c);
        }

        // Block selection with negative left col
        term.start_selection(SelectionPoint::new(-5, 0), SelectionMode::Block);
        term.update_selection(SelectionPoint::new(2, 0));

        // Should not panic
        let text = term.selected_text().unwrap();
        assert!(text.contains("XYZ"));
    }

    #[test]
    fn working_directory_keeps_the_uri_with_its_host() {
        let mut term = Terminal::new(Dimensions::new(80, 24));
        assert_eq!(term.working_directory_uri(), None);
        term.set_working_directory("file://box/home/a%20b".into());
        assert_eq!(term.working_directory(), Some("/home/a b"));
        assert_eq!(term.working_directory_uri(), Some("file://box/home/a%20b"));
    }

    #[test]
    fn percent_decode_ascii() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("no%25percent"), "no%percent");
    }

    #[test]
    fn percent_decode_multibyte_utf8() {
        // é is U+00E9, encoded as %C3%A9 in UTF-8
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // 日 is U+65E5, encoded as %E6%97%A5
        assert_eq!(percent_decode("%E6%97%A5"), "日");
    }

    #[test]
    fn percent_decode_invalid_hex() {
        // Invalid hex digits should be kept as-is
        assert_eq!(percent_decode("%GG"), "%GG");
        // Truncated at end
        assert_eq!(percent_decode("abc%2"), "abc%2");
    }
}
