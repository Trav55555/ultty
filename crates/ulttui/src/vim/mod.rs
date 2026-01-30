//! Vim motion support for widgets.
//!
//! This module provides:
//! - `VimHandler` trait for widgets that support vim motions
//! - `KeyParser` for converting key sequences to vim commands
//! - `VimConfig` for customizing vim behavior

mod parser;

pub use parser::KeyParser;
pub use ultty_protocol::vim::{
    VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimRegister, VimState,
};

use ultty_protocol::{Key, Modifiers};

/// Result of handling a vim key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimResult {
    /// Key was consumed, command executed.
    Handled,
    /// Key was consumed, waiting for more input (e.g., after 'd' waiting for motion).
    Pending,
    /// Key was not handled by vim layer (pass to normal handling).
    Ignored,
    /// Mode changed.
    ModeChanged(VimMode),
    /// Action to execute.
    Action(VimAction),
    /// Command to execute.
    Command(VimCommand),
}

/// Trait for widgets that support vim motions.
pub trait VimHandler {
    /// Returns true if this widget supports vim motions.
    fn supports_vim(&self) -> bool {
        true
    }

    /// Get the current vim state.
    fn vim_state(&self) -> &VimState;

    /// Get mutable vim state.
    fn vim_state_mut(&mut self) -> &mut VimState;

    /// Handle a vim action (immediate actions like 'i', 'a', 'x', etc.).
    fn handle_vim_action(&mut self, action: &VimAction) -> VimResult;

    /// Handle a vim command (operator + motion like 'dw', '3j', etc.).
    fn handle_vim_command(&mut self, command: &VimCommand) -> VimResult;

    /// Handle raw key input, parsing it through vim state machine.
    /// Returns the result of handling.
    fn handle_vim_key(&mut self, key: &Key, modifiers: Modifiers) -> VimResult {
        let mut parser = KeyParser::new();
        match parser.parse_key(self.vim_state_mut(), key, modifiers) {
            Some(ParseResult::Action(action)) => {
                let result = self.handle_vim_action(&action);
                // Update mode if action changes it
                if let VimAction::ExitToNormal = action {
                    self.vim_state_mut().reset_to_normal();
                }
                result
            }
            Some(ParseResult::Command(cmd)) => self.handle_vim_command(&cmd),
            Some(ParseResult::ModeChange(mode)) => {
                self.vim_state_mut().mode = mode;
                VimResult::ModeChanged(mode)
            }
            Some(ParseResult::Pending) => VimResult::Pending,
            None => VimResult::Ignored,
        }
    }
}

/// Result from the key parser.
#[derive(Debug, Clone)]
pub enum ParseResult {
    /// An immediate action.
    Action(VimAction),
    /// A complete command.
    Command(VimCommand),
    /// Mode change.
    ModeChange(VimMode),
    /// Waiting for more input.
    Pending,
}

/// Configuration for vim behavior.
#[derive(Debug, Clone)]
pub struct VimConfig {
    /// Enable vim motions.
    pub enabled: bool,
    /// Start in insert mode (useful for text inputs).
    pub start_in_insert: bool,
    /// Allow escape to exit insert mode.
    pub escape_exits_insert: bool,
    /// Use jk or kj to exit insert mode.
    pub jk_escape: bool,
    /// Timeout for key sequences in milliseconds.
    pub key_timeout_ms: u64,
    /// Show mode indicator.
    pub show_mode: bool,
    /// Relative line numbers.
    pub relative_numbers: bool,
}

impl Default for VimConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            start_in_insert: false,
            escape_exits_insert: true,
            jk_escape: false,
            key_timeout_ms: 1000,
            show_mode: true,
            relative_numbers: false,
        }
    }
}

impl VimConfig {
    /// Create config for a text input (starts in insert mode).
    pub fn for_text_input() -> Self {
        Self {
            start_in_insert: true,
            ..Default::default()
        }
    }

    /// Create config for a scrollable view (no insert mode).
    pub fn for_scroll_view() -> Self {
        Self {
            start_in_insert: false,
            ..Default::default()
        }
    }

    /// Create config for a list (no insert mode, simplified motions).
    pub fn for_list() -> Self {
        Self {
            start_in_insert: false,
            ..Default::default()
        }
    }
}

/// Extension trait for enabling vim on widgets.
pub trait VimExt: Sized {
    /// Enable vim motions on this widget.
    fn vim(self) -> Self;

    /// Enable vim motions with custom config.
    fn vim_with_config(self, config: VimConfig) -> Self;
}
