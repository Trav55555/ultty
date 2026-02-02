//! Terminal state management with alternate screen support.

use super::{Buffer, CursorPosition, Dimensions, Grid, ImageStore, TerminalImage};

/// Mouse tracking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, Copy, Default)]
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
    /// Alternate scroll mode - when enabled, scroll events scroll the alternate screen
    /// instead of being sent as arrow keys
    pub alternate_scroll: bool,
    /// Synchronized update mode - batches screen updates for flicker-free rendering
    pub synchronized_update: bool,
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
#[derive(Debug, Clone, Default)]
pub struct ShellIntegration {
    /// Current working directory (from OSC 7)
    pub working_directory: Option<String>,
    /// Line where current prompt started
    pub prompt_line: Option<u16>,
    /// Line where current command started
    pub command_line: Option<u16>,
    /// Whether a command is currently executing
    pub command_executing: bool,
    /// Last command exit code
    pub last_exit_code: Option<i32>,
}

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
    /// Window title
    title: String,
    /// Shell integration state
    shell_integration: ShellIntegration,
    /// Inline images (Sixel, etc.)
    images: ImageStore,
    /// Pending responses to send back to PTY
    pending_responses: Vec<Vec<u8>>,
    /// Hyperlink ID to URI mapping (OSC 8)
    hyperlinks: std::collections::HashMap<u32, String>,
    /// Next hyperlink ID to assign
    next_hyperlink_id: u32,
    /// Current hyperlink ID (0 = none, applied to new cells)
    current_hyperlink_id: u32,
    /// Stack of Kitty keyboard mode flags
    kitty_keyboard_stack: Vec<u8>,
    /// Pending column mode resize request (132 or 80 columns, DECCOLM)
    pending_column_resize: Option<u16>,
}

impl Terminal {
    /// Create a new terminal with the given dimensions.
    pub fn new(dims: Dimensions) -> Self {
        Self {
            main: Buffer::new(dims),
            alternate: Buffer::with_scrollback(dims, 0), // No scrollback for alternate
            using_alternate: false,
            modes: TerminalModes::default(),
            selection: None,
            title: String::new(),
            shell_integration: ShellIntegration::default(),
            images: ImageStore::new(),
            pending_responses: Vec::new(),
            hyperlinks: std::collections::HashMap::new(),
            next_hyperlink_id: 1, // 0 means no hyperlink
            current_hyperlink_id: 0,
            kitty_keyboard_stack: Vec::new(),
            pending_column_resize: None,
        }
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

    /// Get the active buffer.
    pub fn buffer(&self) -> &Buffer {
        if self.using_alternate {
            &self.alternate
        } else {
            &self.main
        }
    }

    /// Get mutable access to the active buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        // Clear selection when buffer is modified
        self.selection = None;

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

    /// Enter alternate screen buffer.
    pub fn enter_alternate_screen(&mut self) {
        if !self.using_alternate {
            self.using_alternate = true;
            self.alternate.grid_mut().clear();
            self.alternate.grid_mut().cursor_mut().position = CursorPosition::origin();
            self.selection = None;
        }
    }

    /// Leave alternate screen buffer.
    pub fn leave_alternate_screen(&mut self) {
        if self.using_alternate {
            self.using_alternate = false;
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

    /// Get current selection.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Start a new selection.
    pub fn start_selection(&mut self, pos: SelectionPoint, mode: SelectionMode) {
        self.selection = Some(Selection {
            start: pos,
            end: pos,
            mode,
        });
    }

    /// Update selection end point.
    pub fn update_selection(&mut self, end: SelectionPoint) {
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
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.ordered();

        let mut text = String::new();
        let buffer = self.buffer();

        match sel.mode {
            SelectionMode::Normal => {
                // Character-wise selection
                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        let start_col = if row_idx == start.row { start.col } else { 0 };
                        let end_col = if row_idx == end.row {
                            end.col
                        } else {
                            buffer.dimensions().cols as i32 - 1
                        };

                        for col in start_col..=end_col {
                            if let Some(cell) = row.get(col as u16) {
                                text.push(cell.char());
                            }
                        }

                        // Add newline between rows (except last)
                        if row_idx < end.row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Line => {
                // Line-wise selection
                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        text.push_str(&row.text_trimmed());
                        if row_idx < end.row {
                            text.push('\n');
                        }
                    }
                }
            }
            SelectionMode::Block => {
                // Block/rectangular selection
                let (left, right) = if start.col <= end.col {
                    (start.col, end.col)
                } else {
                    (end.col, start.col)
                };

                for row_idx in start.row..=end.row {
                    if let Some(row) = buffer.visible_row(row_idx as u16) {
                        for col in left..=right {
                            if let Some(cell) = row.get(col as u16) {
                                text.push(cell.char());
                            }
                        }
                        if row_idx < end.row {
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
        let Some(sel) = &self.selection else {
            return false;
        };

        let (start, end) = sel.ordered();
        let row = row as i32;
        let col = col as i32;

        match sel.mode {
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

    /// Get window title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set window title.
    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Get shell integration state.
    pub fn shell_integration(&self) -> &ShellIntegration {
        &self.shell_integration
    }

    /// Get the working directory (from OSC 7).
    pub fn working_directory(&self) -> Option<&str> {
        self.shell_integration.working_directory.as_deref()
    }

    /// Set working directory (from OSC 7 file://host/path).
    pub fn set_working_directory(&mut self, uri: String) {
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
        let row = self.grid().cursor().position.row;
        self.shell_integration.prompt_line = Some(row);
        self.shell_integration.command_executing = false;
        tracing::trace!("Prompt started at line {}", row);
    }

    /// Mark command start (OSC 133;B).
    pub fn mark_command_start(&mut self) {
        let row = self.grid().cursor().position.row;
        self.shell_integration.command_line = Some(row);
        tracing::trace!("Command started at line {}", row);
    }

    /// Mark command executed (OSC 133;C).
    pub fn mark_command_executed(&mut self) {
        self.shell_integration.command_executing = true;
        tracing::trace!("Command executing");
    }

    /// Mark command finished (OSC 133;D).
    pub fn mark_command_finished(&mut self, exit_code: Option<i32>) {
        self.shell_integration.command_executing = false;
        self.shell_integration.last_exit_code = exit_code;
        tracing::trace!("Command finished with exit code: {:?}", exit_code);
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
        let id = self.next_hyperlink_id;
        self.next_hyperlink_id = self.next_hyperlink_id.wrapping_add(1);
        if self.next_hyperlink_id == 0 {
            self.next_hyperlink_id = 1; // Skip 0, which means no hyperlink
        }
        self.hyperlinks.insert(id, uri);
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
        let cell = self.buffer().grid().cell(row, col)?;
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

    /// Push Kitty keyboard mode with given flags onto the stack.
    pub fn push_kitty_keyboard_mode(&mut self, flags: u8) {
        self.kitty_keyboard_stack.push(flags);
        self.modes.kitty_keyboard_flags = flags;
        tracing::debug!("Pushed Kitty keyboard mode: flags={}", flags);
    }

    /// Pop Kitty keyboard mode from the stack.
    pub fn pop_kitty_keyboard_mode(&mut self) {
        if self.kitty_keyboard_stack.pop().is_some() {
            // Restore previous flags or 0 if stack is empty
            self.modes.kitty_keyboard_flags =
                self.kitty_keyboard_stack.last().copied().unwrap_or(0);
            tracing::debug!(
                "Popped Kitty keyboard mode: flags={}",
                self.modes.kitty_keyboard_flags
            );
        }
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

        self.images.add(image);

        // Move cursor past the image
        // For now, just advance by the width on the same line
        // TODO: Handle multi-line images properly
        let new_col = self.grid().cursor().position.col.saturating_add(cell_width);
        if new_col < self.dimensions().cols {
            self.grid_mut().cursor_mut().position.col = new_col;
        }

        tracing::debug!(
            "Added image: {}x{} cells at cursor",
            cell_width,
            cell_height
        );
    }

    /// Resize the terminal.
    pub fn resize(&mut self, new_dims: Dimensions) {
        self.main.resize(new_dims);
        self.alternate.resize(new_dims);
        self.selection = None;
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

        // Check if we should report motion
        if motion {
            match self.modes.mouse_mode {
                MouseMode::AnyEvent => {}
                MouseMode::ButtonEvent if pressed => {}
                _ => return None,
            }
        }

        // Encode based on format
        match self.modes.mouse_encoding {
            MouseEncoding::Sgr => {
                Some(self.encode_mouse_sgr(button, x, y, pressed, motion, modifiers))
            }
            MouseEncoding::X10 | MouseEncoding::Utf8 => {
                Some(self.encode_mouse_x10(button, x, y, pressed, motion, modifiers))
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

        // Add 32 to make printable, clamp to 223
        let cb = cb + 32;
        let cx = ((x + 1).min(223) + 32) as u8;
        let cy = ((y + 1).min(223) + 32) as u8;

        vec![b'\x1b', b'[', b'M', cb, cx, cy]
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
}

/// Simple percent-decoding for file paths (e.g., %20 -> space).
fn percent_decode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            // Try to parse next two chars as hex
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    output.push(byte as char);
                    continue;
                }
            }
            // Invalid escape, keep as-is
            output.push('%');
            output.push_str(&hex);
        } else {
            output.push(c);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resize_clears_selection() {
        let mut term = Terminal::new(Dimensions::new(80, 24));

        term.start_selection(SelectionPoint::new(0, 0), SelectionMode::Normal);
        assert!(term.selection().is_some());

        term.resize(Dimensions::new(100, 30));
        assert!(term.selection().is_none());
    }
}
