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

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimState};
    use ultty_protocol::{Key, Modifiers};

    // --- VimResult ---

    #[test]
    fn vim_result_handled() {
        let r = VimResult::Handled;
        assert_eq!(r, VimResult::Handled);
    }

    #[test]
    fn vim_result_pending() {
        let r = VimResult::Pending;
        assert_eq!(r, VimResult::Pending);
    }

    #[test]
    fn vim_result_ignored() {
        let r = VimResult::Ignored;
        assert_eq!(r, VimResult::Ignored);
    }

    #[test]
    fn vim_result_mode_changed() {
        let r = VimResult::ModeChanged(VimMode::Insert);
        assert_eq!(r, VimResult::ModeChanged(VimMode::Insert));
    }

    #[test]
    fn vim_result_mode_changed_visual() {
        let r = VimResult::ModeChanged(VimMode::Visual);
        assert_eq!(r, VimResult::ModeChanged(VimMode::Visual));
    }

    #[test]
    fn vim_result_is_debuggable() {
        let s = format!("{:?}", VimResult::Handled);
        assert!(s.contains("Handled"));
    }

    #[test]
    fn vim_result_is_cloneable() {
        let r = VimResult::ModeChanged(VimMode::Normal);
        let r2 = r.clone();
        assert_eq!(r, r2);
    }

    // --- VimConfig ---

    #[test]
    fn vim_config_default() {
        let config = VimConfig::default();
        assert!(config.enabled);
        assert!(!config.start_in_insert);
        assert!(config.escape_exits_insert);
        assert!(!config.jk_escape);
        assert_eq!(config.key_timeout_ms, 1000);
        assert!(config.show_mode);
        assert!(!config.relative_numbers);
    }

    #[test]
    fn vim_config_for_text_input() {
        let config = VimConfig::for_text_input();
        assert!(config.enabled);
        assert!(config.start_in_insert);
        assert!(config.escape_exits_insert);
    }

    #[test]
    fn vim_config_for_scroll_view() {
        let config = VimConfig::for_scroll_view();
        assert!(config.enabled);
        assert!(!config.start_in_insert);
    }

    #[test]
    fn vim_config_for_list() {
        let config = VimConfig::for_list();
        assert!(config.enabled);
        assert!(!config.start_in_insert);
    }

    #[test]
    fn vim_config_is_cloneable() {
        let config = VimConfig::default();
        let config2 = config.clone();
        assert_eq!(config.enabled, config2.enabled);
        assert_eq!(config.key_timeout_ms, config2.key_timeout_ms);
    }

    #[test]
    fn vim_config_is_debuggable() {
        let config = VimConfig::default();
        let s = format!("{:?}", config);
        assert!(s.contains("VimConfig"));
        assert!(s.contains("enabled"));
    }

    // --- ParseResult ---

    #[test]
    fn parse_result_action() {
        let r = ParseResult::Action(VimAction::InsertAtCursor);
        assert!(matches!(r, ParseResult::Action(VimAction::InsertAtCursor)));
    }

    #[test]
    fn parse_result_command() {
        let cmd = VimCommand::motion(VimMotion::Down);
        let r = ParseResult::Command(cmd);
        assert!(matches!(r, ParseResult::Command(_)));
    }

    #[test]
    fn parse_result_mode_change() {
        let r = ParseResult::ModeChange(VimMode::Insert);
        assert!(matches!(r, ParseResult::ModeChange(VimMode::Insert)));
    }

    #[test]
    fn parse_result_pending() {
        let r = ParseResult::Pending;
        assert!(matches!(r, ParseResult::Pending));
    }

    #[test]
    fn parse_result_is_debuggable() {
        let s = format!("{:?}", ParseResult::Pending);
        assert!(s.contains("Pending"));
    }

    // --- KeyParser integration with modes ---

    #[test]
    fn parser_normal_mode_basic_motions() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        assert_eq!(state.mode, VimMode::Normal);

        let result = parser.parse_key(&mut state, &Key::Char('j'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Down,
                operator: VimOperator::None,
                count: None,
            }))
        ));
    }

    #[test]
    fn parser_normal_to_insert_with_i() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('i'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::InsertAtCursor))
        ));
    }

    #[test]
    fn parser_normal_to_insert_with_a() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('a'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::InsertAfterCursor))
        ));
    }

    #[test]
    fn parser_normal_to_visual_with_v() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('v'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::EnterVisual))
        ));
    }

    #[test]
    fn parser_normal_to_visual_line_with_shift_v() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('V'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::EnterVisualLine))
        ));
    }

    #[test]
    fn parser_insert_escape_returns_to_normal() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Insert;

        let result = parser.parse_key(&mut state, &Key::Escape, Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_insert_mode_passes_through_chars() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Insert;

        let result = parser.parse_key(&mut state, &Key::Char('a'), Modifiers::NONE);
        assert!(result.is_none()); // chars pass through in insert mode
    }

    #[test]
    fn parser_visual_escape_returns_to_normal() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        let result = parser.parse_key(&mut state, &Key::Escape, Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_visual_d_deletes_selection() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        let result = parser.parse_key(&mut state, &Key::Char('d'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Command(_))));
        // After d in visual, mode resets to normal
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_visual_y_yanks_selection() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        let result = parser.parse_key(&mut state, &Key::Char('y'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Command(_))));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_visual_c_changes_selection() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        let result = parser.parse_key(&mut state, &Key::Char('c'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Command(_))));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_visual_v_toggles_back_to_normal() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        let result = parser.parse_key(&mut state, &Key::Char('v'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_visual_line_to_visual_char() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::VisualLine;

        let result = parser.parse_key(&mut state, &Key::Char('v'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::ModeChange(VimMode::Visual))
        ));
        assert_eq!(state.mode, VimMode::Visual);
    }

    #[test]
    fn parser_command_mode_escape_exits() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Command;

        let result = parser.parse_key(&mut state, &Key::Escape, Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_command_mode_collects_chars() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Command;

        let result = parser.parse_key(&mut state, &Key::Char('w'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Pending)));
        assert_eq!(state.command_buffer, "w");

        let result = parser.parse_key(&mut state, &Key::Char('q'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Pending)));
        assert_eq!(state.command_buffer, "wq");
    }

    #[test]
    fn parser_command_mode_backspace_removes_char() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Command;
        state.command_buffer = "wq".to_string();

        let result = parser.parse_key(&mut state, &Key::Backspace, Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Pending)));
        assert_eq!(state.command_buffer, "w");
    }

    #[test]
    fn parser_command_mode_backspace_on_empty_exits() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Command;
        state.command_buffer = "x".to_string();

        // First backspace removes the char
        let _ = parser.parse_key(&mut state, &Key::Backspace, Modifiers::NONE);
        // Buffer is now empty, so it exits command mode
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_command_mode_enter_exits() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Command;
        state.command_buffer = "w".to_string();

        let result = parser.parse_key(&mut state, &Key::Enter, Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_count_then_motion() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let r1 = parser.parse_key(&mut state, &Key::Char('5'), Modifiers::NONE);
        assert!(matches!(r1, Some(ParseResult::Pending)));

        let r2 = parser.parse_key(&mut state, &Key::Char('j'), Modifiers::NONE);
        assert!(matches!(
            r2,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Down,
                count: Some(5),
                ..
            }))
        ));
    }

    #[test]
    fn parser_multi_digit_count() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('1'), Modifiers::NONE);
        let _ = parser.parse_key(&mut state, &Key::Char('2'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('k'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Up,
                count: Some(12),
                ..
            }))
        ));
    }

    #[test]
    fn parser_zero_without_count_is_line_start() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('0'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::LineStart,
                ..
            }))
        ));
    }

    #[test]
    fn parser_zero_after_count_extends_count() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('1'), Modifiers::NONE);
        let _ = parser.parse_key(&mut state, &Key::Char('0'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('j'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Down,
                count: Some(10),
                ..
            }))
        ));
    }

    #[test]
    fn parser_dd_deletes_line() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let r1 = parser.parse_key(&mut state, &Key::Char('d'), Modifiers::NONE);
        assert!(matches!(r1, Some(ParseResult::Pending)));

        let r2 = parser.parse_key(&mut state, &Key::Char('d'), Modifiers::NONE);
        assert!(matches!(
            r2,
            Some(ParseResult::Action(VimAction::DeleteLine))
        ));
    }

    #[test]
    fn parser_yy_yanks_line() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('y'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('y'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::YankLine))
        ));
    }

    #[test]
    fn parser_cc_changes_line() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('c'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('c'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ChangeLine))
        ));
    }

    #[test]
    fn parser_gg_goes_to_document_start() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let r1 = parser.parse_key(&mut state, &Key::Char('g'), Modifiers::NONE);
        assert!(matches!(r1, Some(ParseResult::Pending)));

        let r2 = parser.parse_key(&mut state, &Key::Char('g'), Modifiers::NONE);
        assert!(matches!(
            r2,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::DocumentStart,
                ..
            }))
        ));
    }

    #[test]
    fn parser_shift_g_goes_to_document_end() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('G'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::DocumentEnd,
                ..
            }))
        ));
    }

    #[test]
    fn parser_count_g_goes_to_line() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('5'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('G'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Line(5),
                ..
            }))
        ));
    }

    #[test]
    fn parser_word_motions() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('w'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Word,
                ..
            }))
        ));
    }

    #[test]
    fn parser_x_deletes_char() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('x'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::DeleteChar))
        ));
    }

    #[test]
    fn parser_u_undo() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('u'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::Undo))
        ));
    }

    #[test]
    fn parser_p_paste_after() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('p'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::PasteAfter))
        ));
    }

    #[test]
    fn parser_shift_p_paste_before() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('P'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::PasteBefore))
        ));
    }

    #[test]
    fn parser_dot_repeat() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('.'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::RepeatLastChange))
        ));
    }

    #[test]
    fn parser_search_forward() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('/'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::SearchForward))
        ));
    }

    #[test]
    fn parser_search_backward() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('?'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::SearchBackward))
        ));
    }

    #[test]
    fn parser_colon_command_mode() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char(':'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::CommandMode))
        ));
    }

    #[test]
    fn parser_z_commands() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let r1 = parser.parse_key(&mut state, &Key::Char('z'), Modifiers::NONE);
        assert!(matches!(r1, Some(ParseResult::Pending)));

        let r2 = parser.parse_key(&mut state, &Key::Char('z'), Modifiers::NONE);
        assert!(matches!(
            r2,
            Some(ParseResult::Action(VimAction::CenterScreen))
        ));
    }

    #[test]
    fn parser_z_t_scroll_top() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('z'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('t'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ScrollCursorTop))
        ));
    }

    #[test]
    fn parser_z_b_scroll_bottom() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('z'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('b'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ScrollCursorBottom))
        ));
    }

    #[test]
    fn parser_ctrl_d_half_page_down() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('d'), Modifiers::CTRL);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::HalfPageDown,
                ..
            }))
        ));
    }

    #[test]
    fn parser_ctrl_u_half_page_up() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('u'), Modifiers::CTRL);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::HalfPageUp,
                ..
            }))
        ));
    }

    #[test]
    fn parser_find_char_f() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let r1 = parser.parse_key(&mut state, &Key::Char('f'), Modifiers::NONE);
        assert!(matches!(r1, Some(ParseResult::Pending)));

        let r2 = parser.parse_key(&mut state, &Key::Char('x'), Modifiers::NONE);
        assert!(matches!(
            r2,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::FindChar('x'),
                ..
            }))
        ));
    }

    #[test]
    fn parser_find_char_f_stores_last_find() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('f'), Modifiers::NONE);
        let _ = parser.parse_key(&mut state, &Key::Char('a'), Modifiers::NONE);
        assert!(state.last_find_char.is_some());
    }

    #[test]
    fn parser_screen_motions() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('H'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::ScreenTop,
                ..
            }))
        ));
    }

    #[test]
    fn parser_line_end_dollar() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('$'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::LineEnd,
                ..
            }))
        ));
    }

    #[test]
    fn parser_line_first_nonblank_caret() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Char('^'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::LineFirstNonBlank,
                ..
            }))
        ));
    }

    #[test]
    fn parser_normal_escape_resets() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let result = parser.parse_key(&mut state, &Key::Escape, Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
    }

    #[test]
    fn parser_visual_motions_extend_selection() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        // Motions in visual mode should work the same as normal mode
        let result = parser.parse_key(&mut state, &Key::Char('w'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Word,
                ..
            }))
        ));
    }

    #[test]
    fn parser_visual_line_shift_v_toggles_off() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::VisualLine;

        let result = parser.parse_key(&mut state, &Key::Char('V'), Modifiers::NONE);
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn parser_g_u_sets_lowercase_operator() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('g'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('u'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Pending)));
        assert!(state.has_pending_operator());
    }

    #[test]
    fn parser_g_shift_u_sets_uppercase_operator() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        let _ = parser.parse_key(&mut state, &Key::Char('g'), Modifiers::NONE);
        let result = parser.parse_key(&mut state, &Key::Char('U'), Modifiers::NONE);
        assert!(matches!(result, Some(ParseResult::Pending)));
        assert!(state.has_pending_operator());
    }
}
