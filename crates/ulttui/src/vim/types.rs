//! Vim motion types and commands.
//!
//! The data types for vim-style navigation and editing in widgets.
//! [`super::KeyParser`] turns key sequences into these types.

/// Vim editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum VimMode {
    /// Normal mode - navigation and commands.
    #[default]
    Normal = 0,
    /// Insert mode - text entry.
    Insert = 1,
    /// Visual mode - character-wise selection.
    Visual = 2,
    /// Visual line mode - line-wise selection.
    VisualLine = 3,
    /// Visual block mode - block selection.
    VisualBlock = 4,
    /// Command-line mode (`:` commands).
    Command = 5,
    /// Search mode (`/` or `?`).
    Search = 6,
}

impl VimMode {
    /// Returns true if this is an insert-like mode where typing inserts text.
    pub fn is_insert(&self) -> bool {
        matches!(self, VimMode::Insert | VimMode::Command | VimMode::Search)
    }

    /// Returns true if this is a visual selection mode.
    pub fn is_visual(&self) -> bool {
        matches!(
            self,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        )
    }
}

/// Vim motion - describes cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMotion {
    // Character motions
    /// Move left (h).
    Left,
    /// Move right (l).
    Right,
    /// Move up (k).
    Up,
    /// Move down (j).
    Down,

    // Word motions
    /// Move to start of next word (w).
    Word,
    /// Move to start of next WORD (W).
    WordBig,
    /// Move to end of word (e).
    WordEnd,
    /// Move to end of WORD (E).
    WordEndBig,
    /// Move to start of previous word (b).
    WordBack,
    /// Move to start of previous WORD (B).
    WordBackBig,

    // Line motions
    /// Move to first non-blank character (^).
    LineFirstNonBlank,
    /// Move to start of line (0).
    LineStart,
    /// Move to end of line ($).
    LineEnd,

    // Document motions
    /// Move to first line (gg).
    DocumentStart,
    /// Move to last line (G).
    DocumentEnd,
    /// Move to specific line number (42G or :42).
    Line(usize),

    // Screen motions
    /// Move half page down (Ctrl-d).
    HalfPageDown,
    /// Move half page up (Ctrl-u).
    HalfPageUp,
    /// Move full page down (Ctrl-f).
    PageDown,
    /// Move full page up (Ctrl-b).
    PageUp,
    /// Move to top of screen (H).
    ScreenTop,
    /// Move to middle of screen (M).
    ScreenMiddle,
    /// Move to bottom of screen (L).
    ScreenBottom,

    // Search motions
    /// Find character forward (f{char}).
    FindChar(char),
    /// Find character backward (F{char}).
    FindCharBack(char),
    /// Find character forward, stop before (t{char}).
    TillChar(char),
    /// Find character backward, stop after (T{char}).
    TillCharBack(char),
    /// Repeat last find (;).
    RepeatFind,
    /// Repeat last find in reverse (,).
    RepeatFindReverse,

    // Text object motions (used with operators)
    /// Inner word (iw).
    InnerWord,
    /// A word including surrounding space (aw).
    AWord,
    /// Inner WORD (iW).
    InnerWordBig,
    /// A WORD including surrounding space (aW).
    AWordBig,
    /// Inner sentence (is).
    InnerSentence,
    /// A sentence (as).
    ASentence,
    /// Inner paragraph (ip).
    InnerParagraph,
    /// A paragraph (ap).
    AParagraph,
    /// Inner block with delimiter (i{, i(, i[, i<, i", i', i`).
    InnerBlock(char),
    /// A block including delimiters (a{, a(, a[, a<, a", a', a`).
    ABlock(char),

    // Misc
    /// Match bracket (%).
    MatchBracket,
    /// Next search result (n).
    SearchNext,
    /// Previous search result (N).
    SearchPrev,
}

/// Vim operator - action to perform on a motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimOperator {
    /// No operator (motion only).
    #[default]
    None,
    /// Delete (d).
    Delete,
    /// Change (c) - delete and enter insert mode.
    Change,
    /// Yank/copy (y).
    Yank,
    /// Indent right (>).
    IndentRight,
    /// Indent left (<).
    IndentLeft,
    /// Auto-format (=).
    Format,
    /// Uppercase (gU).
    Uppercase,
    /// Lowercase (gu).
    Lowercase,
    /// Swap case (g~).
    SwapCase,
}

/// A complete vim command ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimCommand {
    /// Repeat count (e.g., 3 in `3dw`).
    pub count: Option<usize>,
    /// The operator to apply.
    pub operator: VimOperator,
    /// The motion or text object.
    pub motion: VimMotion,
}

impl VimCommand {
    /// Create a motion-only command.
    pub fn motion(motion: VimMotion) -> Self {
        Self {
            count: None,
            operator: VimOperator::None,
            motion,
        }
    }

    /// Create a motion with count.
    pub fn motion_with_count(count: usize, motion: VimMotion) -> Self {
        Self {
            count: Some(count),
            operator: VimOperator::None,
            motion,
        }
    }

    /// Create an operator + motion command.
    pub fn operator(operator: VimOperator, motion: VimMotion) -> Self {
        Self {
            count: None,
            operator,
            motion,
        }
    }

    /// Create a full command with count, operator, and motion.
    pub fn full(count: usize, operator: VimOperator, motion: VimMotion) -> Self {
        Self {
            count: Some(count),
            operator,
            motion,
        }
    }

    /// Get the effective count (defaults to 1).
    pub fn effective_count(&self) -> usize {
        self.count.unwrap_or(1)
    }
}

/// Vim action - immediate actions that don't use the operator+motion pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimAction {
    /// Enter insert mode at cursor (i).
    InsertAtCursor,
    /// Enter insert mode after cursor (a).
    InsertAfterCursor,
    /// Enter insert mode at line start (I).
    InsertAtLineStart,
    /// Enter insert mode at line end (A).
    InsertAtLineEnd,
    /// Open line below and enter insert mode (o).
    OpenLineBelow,
    /// Open line above and enter insert mode (O).
    OpenLineAbove,
    /// Replace single character (r{char}).
    ReplaceChar(char),
    /// Enter replace mode (R).
    ReplaceMode,
    /// Delete character under cursor (x).
    DeleteChar,
    /// Delete character before cursor (X).
    DeleteCharBefore,
    /// Delete line (dd).
    DeleteLine,
    /// Yank line (yy).
    YankLine,
    /// Change line (cc).
    ChangeLine,
    /// Join lines (J).
    JoinLines,
    /// Undo (u).
    Undo,
    /// Redo (Ctrl-r).
    Redo,
    /// Paste after cursor (p).
    PasteAfter,
    /// Paste before cursor (P).
    PasteBefore,
    /// Enter visual mode (v).
    EnterVisual,
    /// Enter visual line mode (V).
    EnterVisualLine,
    /// Enter visual block mode (Ctrl-v).
    EnterVisualBlock,
    /// Exit to normal mode (Esc).
    ExitToNormal,
    /// Start search forward (/).
    SearchForward,
    /// Start search backward (?).
    SearchBackward,
    /// Start command mode (:).
    CommandMode,
    /// Repeat last change (.).
    RepeatLastChange,
    /// Record macro (q{register}).
    RecordMacro(char),
    /// Play macro (@{register}).
    PlayMacro(char),
    /// Center screen on cursor (zz).
    CenterScreen,
    /// Scroll cursor to top (zt).
    ScrollCursorTop,
    /// Scroll cursor to bottom (zb).
    ScrollCursorBottom,
}

/// Register for yank/delete operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimRegister {
    /// Unnamed register (default).
    #[default]
    Unnamed,
    /// Named register (a-z).
    Named(char),
    /// Numbered register (0-9).
    Numbered(u8),
    /// Small delete register (-).
    SmallDelete,
    /// Last inserted text register (.).
    LastInserted,
    /// Current file name register (%).
    FileName,
    /// Alternate file name register (#).
    AltFileName,
    /// Command register (:).
    Command,
    /// Search register (/).
    Search,
    /// Expression register (=).
    Expression,
    /// System clipboard (*).
    Clipboard,
    /// Primary selection (+).
    Primary,
    /// Black hole register (_).
    BlackHole,
    /// Last yank register (0).
    LastYank,
}

/// Vim state for a widget or component.
#[derive(Debug, Clone, Default)]
pub struct VimState {
    /// Current mode.
    pub mode: VimMode,
    /// Pending count (e.g., typing "3" before a motion).
    pub pending_count: Option<usize>,
    /// Pending operator (e.g., "d" waiting for motion).
    pub pending_operator: Option<VimOperator>,
    /// Current register for next operation.
    pub register: VimRegister,
    /// Last search pattern.
    pub last_search: Option<String>,
    /// Search direction (true = forward).
    pub search_forward: bool,
    /// Last find character (for ; and ,).
    pub last_find_char: Option<(char, bool, bool)>, // (char, forward, till)
    /// Command line buffer (for : and / modes).
    pub command_buffer: String,
    /// Recording macro to this register (None if not recording).
    pub recording_macro: Option<char>,
}

impl VimState {
    /// Create a new vim state in normal mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset to normal mode, clearing pending state.
    pub fn reset_to_normal(&mut self) {
        self.mode = VimMode::Normal;
        self.pending_count = None;
        self.pending_operator = None;
        self.command_buffer.clear();
    }

    /// Add a digit to the pending count.
    pub fn add_count_digit(&mut self, digit: u8) {
        let digit = digit as usize;
        self.pending_count = Some(self.pending_count.unwrap_or(0) * 10 + digit);
    }

    /// Get and consume the pending count.
    pub fn take_count(&mut self) -> Option<usize> {
        self.pending_count.take()
    }

    /// Set the pending operator.
    pub fn set_operator(&mut self, op: VimOperator) {
        self.pending_operator = Some(op);
    }

    /// Get and consume the pending operator.
    pub fn take_operator(&mut self) -> VimOperator {
        self.pending_operator.take().unwrap_or(VimOperator::None)
    }

    /// Check if we have a pending operator.
    pub fn has_pending_operator(&self) -> bool {
        self.pending_operator.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vim_mode_is_insert() {
        assert!(VimMode::Insert.is_insert());
        assert!(VimMode::Command.is_insert());
        assert!(VimMode::Search.is_insert());
        assert!(!VimMode::Normal.is_insert());
        assert!(!VimMode::Visual.is_insert());
    }

    #[test]
    fn test_vim_mode_is_visual() {
        assert!(VimMode::Visual.is_visual());
        assert!(VimMode::VisualLine.is_visual());
        assert!(VimMode::VisualBlock.is_visual());
        assert!(!VimMode::Normal.is_visual());
        assert!(!VimMode::Insert.is_visual());
    }

    #[test]
    fn test_vim_command_effective_count() {
        let cmd = VimCommand::motion(VimMotion::Word);
        assert_eq!(cmd.effective_count(), 1);

        let cmd = VimCommand::motion_with_count(5, VimMotion::Word);
        assert_eq!(cmd.effective_count(), 5);
    }

    #[test]
    fn test_vim_state_count() {
        let mut state = VimState::new();
        state.add_count_digit(1);
        state.add_count_digit(2);
        state.add_count_digit(3);
        assert_eq!(state.pending_count, Some(123));
        assert_eq!(state.take_count(), Some(123));
        assert_eq!(state.pending_count, None);
    }

    #[test]
    fn test_vim_state_reset() {
        let mut state = VimState::new();
        state.mode = VimMode::Visual;
        state.pending_count = Some(5);
        state.pending_operator = Some(VimOperator::Delete);
        state.command_buffer = "test".to_string();

        state.reset_to_normal();

        assert_eq!(state.mode, VimMode::Normal);
        assert_eq!(state.pending_count, None);
        assert_eq!(state.pending_operator, None);
        assert!(state.command_buffer.is_empty());
    }
}
