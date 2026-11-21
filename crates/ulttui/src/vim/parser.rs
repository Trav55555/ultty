//! Vim key sequence parser.
//!
//! Converts raw key events into vim commands and actions.

use super::ParseResult;
use crate::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator, VimState};
use ultty_protocol::{Key, Modifiers};

/// Parser for vim key sequences.
#[derive(Debug, Default, Clone)]
pub struct KeyParser {
    /// Pending character for f/F/t/T commands.
    pending_find: Option<FindType>,
    /// Pending 'g' prefix.
    pending_g: bool,
    /// Pending 'z' prefix.
    pending_z: bool,
    /// Pending text-object scope ('i' for inner, 'a' for around).
    pending_text_object: Option<TextObjectScope>,
}

#[derive(Debug, Clone, Copy)]
enum FindType {
    Find,     // f
    FindBack, // F
    Till,     // t
    TillBack, // T
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextObjectScope {
    Inner,
    Around,
}

impl KeyParser {
    /// Create a new key parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a key event given the current vim state.
    /// Returns None if the key should be passed through (not a vim key).
    pub fn parse_key(
        &mut self,
        state: &mut VimState,
        key: &Key,
        modifiers: Modifiers,
    ) -> Option<ParseResult> {
        // Handle pending find character
        if let Some(find_type) = self.pending_find.take() {
            if matches!(key, Key::Escape) {
                state.reset_to_normal();
                return Some(ParseResult::Action(VimAction::ExitToNormal));
            }
            if let Key::Char(c) = key {
                let motion = match find_type {
                    FindType::Find => VimMotion::FindChar(*c),
                    FindType::FindBack => VimMotion::FindCharBack(*c),
                    FindType::Till => VimMotion::TillChar(*c),
                    FindType::TillBack => VimMotion::TillCharBack(*c),
                };
                state.last_find_char = Some((
                    *c,
                    matches!(find_type, FindType::Find | FindType::Till),
                    matches!(find_type, FindType::Till | FindType::TillBack),
                ));
                return Some(self.complete_motion(state, motion));
            }
            state.take_operator();
            state.take_count();
            return None;
        }

        if let Some(scope) = self.pending_text_object.take() {
            return self.parse_text_object(state, scope, key, modifiers);
        }

        // Handle pending 'g' commands
        if self.pending_g {
            self.pending_g = false;
            return self.parse_g_command(state, key, modifiers);
        }

        // Handle pending 'z' commands
        if self.pending_z {
            self.pending_z = false;
            return self.parse_z_command(state, key);
        }

        match state.mode {
            VimMode::Normal => self.parse_normal_mode(state, key, modifiers),
            VimMode::Insert => self.parse_insert_mode(state, key, modifiers),
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => {
                self.parse_visual_mode(state, key, modifiers)
            }
            VimMode::Command | VimMode::Search => self.parse_command_mode(state, key, modifiers),
        }
    }

    fn parse_normal_mode(
        &mut self,
        state: &mut VimState,
        key: &Key,
        modifiers: Modifiers,
    ) -> Option<ParseResult> {
        // Handle counts (digits, but not 0 at start)
        if let Key::Char(c @ '1'..='9') = key {
            state.add_count_digit(*c as u8 - b'0');
            return Some(ParseResult::Pending);
        }
        if let Key::Char('0') = key {
            if state.pending_count.is_some() {
                state.add_count_digit(0);
                return Some(ParseResult::Pending);
            }
            // 0 without count = line start
            return Some(self.complete_motion(state, VimMotion::LineStart));
        }

        // Handle operators
        if !state.has_pending_operator() {
            match key {
                Key::Char('d') if !modifiers.ctrl() => {
                    state.set_operator(VimOperator::Delete);
                    return Some(ParseResult::Pending);
                }
                Key::Char('c') if !modifiers.ctrl() => {
                    state.set_operator(VimOperator::Change);
                    return Some(ParseResult::Pending);
                }
                Key::Char('y') if !modifiers.ctrl() => {
                    state.set_operator(VimOperator::Yank);
                    return Some(ParseResult::Pending);
                }
                Key::Char('>') => {
                    state.set_operator(VimOperator::IndentRight);
                    return Some(ParseResult::Pending);
                }
                Key::Char('<') => {
                    state.set_operator(VimOperator::IndentLeft);
                    return Some(ParseResult::Pending);
                }
                _ => {}
            }
        }

        // Handle doubled operators (dd, yy, cc, >>, <<)
        if state.has_pending_operator() {
            let op = state.pending_operator.unwrap();
            let doubled = matches!(
                (op, key),
                (VimOperator::Delete, Key::Char('d'))
                    | (VimOperator::Yank, Key::Char('y'))
                    | (VimOperator::Change, Key::Char('c'))
                    | (VimOperator::IndentRight, Key::Char('>'))
                    | (VimOperator::IndentLeft, Key::Char('<'))
            );
            if doubled {
                state.take_operator();
                let action = match op {
                    VimOperator::Delete => VimAction::DeleteLine,
                    VimOperator::Yank => VimAction::YankLine,
                    VimOperator::Change => VimAction::ChangeLine,
                    _ => return None,
                };
                return Some(ParseResult::Action(action));
            }
        }

        // Motions
        match key {
            // Basic motions
            Key::Char('h') | Key::Left => {
                return Some(self.complete_motion(state, VimMotion::Left))
            }
            Key::Char('l') | Key::Right => {
                return Some(self.complete_motion(state, VimMotion::Right))
            }
            Key::Char('j') | Key::Down => {
                return Some(self.complete_motion(state, VimMotion::Down))
            }
            Key::Char('k') | Key::Up => return Some(self.complete_motion(state, VimMotion::Up)),

            // Page motions with Ctrl (must come before plain letter matches)
            Key::Char('d') if modifiers.ctrl() => {
                return Some(self.complete_motion(state, VimMotion::HalfPageDown))
            }
            Key::Char('u') if modifiers.ctrl() => {
                return Some(self.complete_motion(state, VimMotion::HalfPageUp))
            }
            Key::Char('f') if modifiers.ctrl() => {
                return Some(self.complete_motion(state, VimMotion::PageDown))
            }
            Key::Char('b') if modifiers.ctrl() => {
                return Some(self.complete_motion(state, VimMotion::PageUp))
            }
            Key::PageDown => return Some(self.complete_motion(state, VimMotion::PageDown)),
            Key::PageUp => return Some(self.complete_motion(state, VimMotion::PageUp)),

            // Word motions
            Key::Char('w') => return Some(self.complete_motion(state, VimMotion::Word)),
            Key::Char('W') => return Some(self.complete_motion(state, VimMotion::WordBig)),
            Key::Char('e') => return Some(self.complete_motion(state, VimMotion::WordEnd)),
            Key::Char('E') => return Some(self.complete_motion(state, VimMotion::WordEndBig)),
            Key::Char('b') => return Some(self.complete_motion(state, VimMotion::WordBack)),
            Key::Char('B') => return Some(self.complete_motion(state, VimMotion::WordBackBig)),

            // Line motions
            Key::Char('^') => {
                return Some(self.complete_motion(state, VimMotion::LineFirstNonBlank))
            }
            Key::Char('$') | Key::End => {
                return Some(self.complete_motion(state, VimMotion::LineEnd))
            }
            Key::Home => return Some(self.complete_motion(state, VimMotion::LineStart)),

            // Document motions
            Key::Char('G') => {
                let motion = if let Some(n) = state.take_count() {
                    VimMotion::Line(n)
                } else {
                    VimMotion::DocumentEnd
                };
                return Some(self.complete_motion(state, motion));
            }

            // Screen motions
            Key::Char('H') => return Some(self.complete_motion(state, VimMotion::ScreenTop)),
            Key::Char('M') => return Some(self.complete_motion(state, VimMotion::ScreenMiddle)),
            Key::Char('L') => return Some(self.complete_motion(state, VimMotion::ScreenBottom)),

            // Find motions — these work both standalone and as motions after operators (e.g. dfe, ct;)
            Key::Char('f') if !modifiers.ctrl() => {
                self.pending_find = Some(FindType::Find);
                return Some(ParseResult::Pending);
            }
            Key::Char('F') => {
                self.pending_find = Some(FindType::FindBack);
                return Some(ParseResult::Pending);
            }
            Key::Char('t') => {
                self.pending_find = Some(FindType::Till);
                return Some(ParseResult::Pending);
            }
            Key::Char('T') => {
                self.pending_find = Some(FindType::TillBack);
                return Some(ParseResult::Pending);
            }
            Key::Char(';') => return Some(self.complete_motion(state, VimMotion::RepeatFind)),
            Key::Char(',') => {
                return Some(self.complete_motion(state, VimMotion::RepeatFindReverse))
            }

            // Search
            Key::Char('n') => return Some(self.complete_motion(state, VimMotion::SearchNext)),
            Key::Char('N') => return Some(self.complete_motion(state, VimMotion::SearchPrev)),

            // Bracket matching
            Key::Char('%') => return Some(self.complete_motion(state, VimMotion::MatchBracket)),

            // 'g' prefix commands
            Key::Char('g') => {
                self.pending_g = true;
                return Some(ParseResult::Pending);
            }

            // 'z' prefix commands
            Key::Char('z') => {
                self.pending_z = true;
                return Some(ParseResult::Pending);
            }

            // Text object prefixes (valid after an operator or in visual mode)
            Key::Char('i')
                if !modifiers.ctrl()
                    && (state.has_pending_operator() || state.mode.is_visual()) =>
            {
                self.pending_text_object = Some(TextObjectScope::Inner);
                return Some(ParseResult::Pending);
            }
            Key::Char('a')
                if !modifiers.ctrl()
                    && (state.has_pending_operator() || state.mode.is_visual()) =>
            {
                self.pending_text_object = Some(TextObjectScope::Around);
                return Some(ParseResult::Pending);
            }

            // Insert mode entries
            Key::Char('i') => return Some(ParseResult::Action(VimAction::InsertAtCursor)),
            Key::Char('a') => return Some(ParseResult::Action(VimAction::InsertAfterCursor)),
            Key::Char('I') => return Some(ParseResult::Action(VimAction::InsertAtLineStart)),
            Key::Char('A') => return Some(ParseResult::Action(VimAction::InsertAtLineEnd)),
            Key::Char('o') => return Some(ParseResult::Action(VimAction::OpenLineBelow)),
            Key::Char('O') => return Some(ParseResult::Action(VimAction::OpenLineAbove)),

            // Single character actions
            Key::Char('x') => return Some(ParseResult::Action(VimAction::DeleteChar)),
            Key::Char('X') => return Some(ParseResult::Action(VimAction::DeleteCharBefore)),
            Key::Char('J') => return Some(ParseResult::Action(VimAction::JoinLines)),
            Key::Char('u') if !modifiers.ctrl() => {
                return Some(ParseResult::Action(VimAction::Undo))
            }
            Key::Char('r') if modifiers.ctrl() => {
                return Some(ParseResult::Action(VimAction::Redo))
            }
            Key::Char('p') => return Some(ParseResult::Action(VimAction::PasteAfter)),
            Key::Char('P') => return Some(ParseResult::Action(VimAction::PasteBefore)),
            Key::Char('.') => return Some(ParseResult::Action(VimAction::RepeatLastChange)),

            // Visual mode
            Key::Char('v') if !modifiers.ctrl() => {
                return Some(ParseResult::Action(VimAction::EnterVisual))
            }
            Key::Char('V') => return Some(ParseResult::Action(VimAction::EnterVisualLine)),
            Key::Char('v') if modifiers.ctrl() => {
                return Some(ParseResult::Action(VimAction::EnterVisualBlock))
            }

            // Search/command
            Key::Char('/') => return Some(ParseResult::Action(VimAction::SearchForward)),
            Key::Char('?') => return Some(ParseResult::Action(VimAction::SearchBackward)),
            Key::Char(':') => return Some(ParseResult::Action(VimAction::CommandMode)),

            Key::Escape => {
                self.reset_pending();
                state.reset_to_normal();
                return Some(ParseResult::Action(VimAction::ExitToNormal));
            }

            _ => {}
        }

        if state.has_pending_operator() {
            state.take_operator();
            state.take_count();
        }

        None
    }

    fn reset_pending(&mut self) {
        self.pending_find = None;
        self.pending_g = false;
        self.pending_z = false;
        self.pending_text_object = None;
    }

    fn parse_text_object(
        &mut self,
        state: &mut VimState,
        scope: TextObjectScope,
        key: &Key,
        modifiers: Modifiers,
    ) -> Option<ParseResult> {
        if matches!(key, Key::Escape) {
            self.reset_pending();
            state.reset_to_normal();
            return Some(ParseResult::Action(VimAction::ExitToNormal));
        }

        let motion = match key {
            Key::Char(c) if !modifiers.ctrl() => text_object_motion(scope, *c),
            _ => None,
        };

        if let Some(motion) = motion {
            Some(self.complete_motion(state, motion))
        } else {
            state.take_operator();
            state.take_count();
            None
        }
    }

    fn parse_g_command(
        &mut self,
        state: &mut VimState,
        key: &Key,
        _modifiers: Modifiers,
    ) -> Option<ParseResult> {
        match key {
            Key::Char('g') => Some(self.complete_motion(state, VimMotion::DocumentStart)),
            Key::Char('e') => Some(self.complete_motion(state, VimMotion::WordEnd)),
            Key::Char('E') => Some(self.complete_motion(state, VimMotion::WordEndBig)),
            Key::Char('u') => {
                state.set_operator(VimOperator::Lowercase);
                Some(ParseResult::Pending)
            }
            Key::Char('U') => {
                state.set_operator(VimOperator::Uppercase);
                Some(ParseResult::Pending)
            }
            Key::Char('~') => {
                state.set_operator(VimOperator::SwapCase);
                Some(ParseResult::Pending)
            }
            _ => None,
        }
    }

    fn parse_z_command(&mut self, _state: &mut VimState, key: &Key) -> Option<ParseResult> {
        match key {
            Key::Char('z') => Some(ParseResult::Action(VimAction::CenterScreen)),
            Key::Char('t') => Some(ParseResult::Action(VimAction::ScrollCursorTop)),
            Key::Char('b') => Some(ParseResult::Action(VimAction::ScrollCursorBottom)),
            _ => None,
        }
    }

    fn parse_insert_mode(
        &mut self,
        state: &mut VimState,
        key: &Key,
        _modifiers: Modifiers,
    ) -> Option<ParseResult> {
        match key {
            Key::Escape => {
                state.reset_to_normal();
                Some(ParseResult::Action(VimAction::ExitToNormal))
            }
            // Let all other keys pass through for text input
            _ => None,
        }
    }

    fn parse_visual_mode(
        &mut self,
        state: &mut VimState,
        key: &Key,
        modifiers: Modifiers,
    ) -> Option<ParseResult> {
        // Most motions work the same in visual mode
        match key {
            Key::Escape => {
                state.reset_to_normal();
                Some(ParseResult::Action(VimAction::ExitToNormal))
            }
            // Operators in visual mode apply to selection
            Key::Char('d') | Key::Char('x') => {
                let cmd = VimCommand::operator(VimOperator::Delete, VimMotion::Right);
                state.reset_to_normal();
                Some(ParseResult::Command(cmd))
            }
            Key::Char('c') => {
                let cmd = VimCommand::operator(VimOperator::Change, VimMotion::Right);
                state.reset_to_normal();
                Some(ParseResult::Command(cmd))
            }
            Key::Char('y') => {
                let cmd = VimCommand::operator(VimOperator::Yank, VimMotion::Right);
                state.reset_to_normal();
                Some(ParseResult::Command(cmd))
            }
            // Switch visual modes
            Key::Char('v') if !modifiers.ctrl() => {
                if state.mode == VimMode::Visual {
                    state.reset_to_normal();
                    Some(ParseResult::Action(VimAction::ExitToNormal))
                } else {
                    state.mode = VimMode::Visual;
                    Some(ParseResult::ModeChange(VimMode::Visual))
                }
            }
            Key::Char('V') => {
                if state.mode == VimMode::VisualLine {
                    state.reset_to_normal();
                    Some(ParseResult::Action(VimAction::ExitToNormal))
                } else {
                    state.mode = VimMode::VisualLine;
                    Some(ParseResult::ModeChange(VimMode::VisualLine))
                }
            }
            // Motions extend selection
            _ => self.parse_normal_mode(state, key, modifiers),
        }
    }

    fn parse_command_mode(
        &mut self,
        state: &mut VimState,
        key: &Key,
        _modifiers: Modifiers,
    ) -> Option<ParseResult> {
        match key {
            Key::Escape => {
                state.reset_to_normal();
                Some(ParseResult::Action(VimAction::ExitToNormal))
            }
            Key::Enter => {
                // Execute command
                state.reset_to_normal();
                Some(ParseResult::Action(VimAction::ExitToNormal))
            }
            Key::Char(c) => {
                state.command_buffer.push(*c);
                Some(ParseResult::Pending)
            }
            Key::Backspace => {
                state.command_buffer.pop();
                if state.command_buffer.is_empty() {
                    state.reset_to_normal();
                    Some(ParseResult::Action(VimAction::ExitToNormal))
                } else {
                    Some(ParseResult::Pending)
                }
            }
            _ => None,
        }
    }

    /// Complete a motion, combining with any pending operator.
    fn complete_motion(&mut self, state: &mut VimState, motion: VimMotion) -> ParseResult {
        let count = state.take_count();
        let operator = state.take_operator();

        let cmd = VimCommand {
            count,
            operator,
            motion,
        };

        ParseResult::Command(cmd)
    }
}

fn text_object_motion(scope: TextObjectScope, ch: char) -> Option<VimMotion> {
    match (scope, ch) {
        (TextObjectScope::Inner, 'w') => Some(VimMotion::InnerWord),
        (TextObjectScope::Around, 'w') => Some(VimMotion::AWord),
        (TextObjectScope::Inner, 'W') => Some(VimMotion::InnerWordBig),
        (TextObjectScope::Around, 'W') => Some(VimMotion::AWordBig),
        (TextObjectScope::Inner, 's') => Some(VimMotion::InnerSentence),
        (TextObjectScope::Around, 's') => Some(VimMotion::ASentence),
        (TextObjectScope::Inner, 'p') => Some(VimMotion::InnerParagraph),
        (TextObjectScope::Around, 'p') => Some(VimMotion::AParagraph),
        (TextObjectScope::Inner, '"' | '\'' | '`') => Some(VimMotion::InnerBlock(ch)),
        (TextObjectScope::Around, '"' | '\'' | '`') => Some(VimMotion::ABlock(ch)),
        (TextObjectScope::Inner, '(' | ')' | 'b') => Some(VimMotion::InnerBlock('(')),
        (TextObjectScope::Around, '(' | ')' | 'b') => Some(VimMotion::ABlock('(')),
        (TextObjectScope::Inner, '[' | ']') => Some(VimMotion::InnerBlock('[')),
        (TextObjectScope::Around, '[' | ']') => Some(VimMotion::ABlock('[')),
        (TextObjectScope::Inner, '{' | '}' | 'B') => Some(VimMotion::InnerBlock('{')),
        (TextObjectScope::Around, '{' | '}' | 'B') => Some(VimMotion::ABlock('{')),
        (TextObjectScope::Inner, '<' | '>') => Some(VimMotion::InnerBlock('<')),
        (TextObjectScope::Around, '<' | '>') => Some(VimMotion::ABlock('<')),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_keys(keys: &str) -> Option<ParseResult> {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        let mut result = None;

        for c in keys.chars() {
            let key = Key::Char(c);
            result = parser.parse_key(&mut state, &key, Modifiers::NONE);
        }

        result
    }

    #[test]
    fn test_simple_motion() {
        let result = parse_keys("j");
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
    fn test_motion_with_count() {
        let result = parse_keys("3j");
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Down,
                operator: VimOperator::None,
                count: Some(3),
            }))
        ));
    }

    #[test]
    fn test_operator_motion() {
        let result = parse_keys("dw");
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Word,
                operator: VimOperator::Delete,
                count: None,
            }))
        ));
    }

    #[test]
    fn test_count_operator_motion() {
        let result = parse_keys("3dw");
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::Word,
                operator: VimOperator::Delete,
                count: Some(3),
            }))
        ));
    }

    #[test]
    fn test_gg() {
        let result = parse_keys("gg");
        assert!(matches!(
            result,
            Some(ParseResult::Command(VimCommand {
                motion: VimMotion::DocumentStart,
                ..
            }))
        ));
    }

    #[test]
    fn test_insert_mode() {
        let result = parse_keys("i");
        assert!(matches!(
            result,
            Some(ParseResult::Action(VimAction::InsertAtCursor))
        ));
    }

    #[test]
    fn test_text_object_all_inner_and_around_objects() {
        let cases = [
            ("diw", VimOperator::Delete, VimMotion::InnerWord),
            ("daw", VimOperator::Delete, VimMotion::AWord),
            ("diW", VimOperator::Delete, VimMotion::InnerWordBig),
            ("daW", VimOperator::Delete, VimMotion::AWordBig),
            ("dis", VimOperator::Delete, VimMotion::InnerSentence),
            ("das", VimOperator::Delete, VimMotion::ASentence),
            ("dip", VimOperator::Delete, VimMotion::InnerParagraph),
            ("dap", VimOperator::Delete, VimMotion::AParagraph),
            ("ci\"", VimOperator::Change, VimMotion::InnerBlock('"')),
            ("ca\"", VimOperator::Change, VimMotion::ABlock('"')),
            ("yi'", VimOperator::Yank, VimMotion::InnerBlock('\'')),
            ("ya'", VimOperator::Yank, VimMotion::ABlock('\'')),
            ("di`", VimOperator::Delete, VimMotion::InnerBlock('`')),
            ("da`", VimOperator::Delete, VimMotion::ABlock('`')),
            ("di(", VimOperator::Delete, VimMotion::InnerBlock('(')),
            ("di)", VimOperator::Delete, VimMotion::InnerBlock('(')),
            ("dib", VimOperator::Delete, VimMotion::InnerBlock('(')),
            ("da(", VimOperator::Delete, VimMotion::ABlock('(')),
            ("da)", VimOperator::Delete, VimMotion::ABlock('(')),
            ("dab", VimOperator::Delete, VimMotion::ABlock('(')),
            ("yi[", VimOperator::Yank, VimMotion::InnerBlock('[')),
            ("yi]", VimOperator::Yank, VimMotion::InnerBlock('[')),
            ("ya[", VimOperator::Yank, VimMotion::ABlock('[')),
            ("ya]", VimOperator::Yank, VimMotion::ABlock('[')),
            ("ca{", VimOperator::Change, VimMotion::ABlock('{')),
            ("ca}", VimOperator::Change, VimMotion::ABlock('{')),
            ("caB", VimOperator::Change, VimMotion::ABlock('{')),
            ("ci{", VimOperator::Change, VimMotion::InnerBlock('{')),
            ("ci}", VimOperator::Change, VimMotion::InnerBlock('{')),
            ("ciB", VimOperator::Change, VimMotion::InnerBlock('{')),
            ("di<", VimOperator::Delete, VimMotion::InnerBlock('<')),
            ("da>", VimOperator::Delete, VimMotion::ABlock('<')),
            ("gUiw", VimOperator::Uppercase, VimMotion::InnerWord),
            ("guiw", VimOperator::Lowercase, VimMotion::InnerWord),
            ("g~aw", VimOperator::SwapCase, VimMotion::AWord),
        ];

        for (seq, expected_op, expected_motion) in cases {
            let mut parser = KeyParser::new();
            let mut state = VimState::new();
            let chars: Vec<char> = seq.chars().collect();
            for &ch in &chars[..chars.len() - 1] {
                let res = parser.parse_key(&mut state, &Key::Char(ch), Modifiers::NONE);
                assert!(
                    matches!(res, Some(ParseResult::Pending)),
                    "prefix of {seq:?} at {ch:?} should be pending, got {res:?}"
                );
            }
            let final_res = parser.parse_key(
                &mut state,
                &Key::Char(*chars.last().unwrap()),
                Modifiers::NONE,
            );
            assert!(
                matches!(
                    final_res,
                    Some(ParseResult::Command(VimCommand {
                        count: None,
                        operator,
                        motion,
                    })) if operator == expected_op && motion == expected_motion
                ),
                "sequence {seq:?} got {final_res:?}"
            );
            assert!(!state.has_pending_operator());
            assert_eq!(state.pending_count, None);
        }
    }

    #[test]
    fn test_text_object_with_counts() {
        for seq in ["2diw", "d2iw"] {
            let result = parse_keys(seq);
            assert!(
                matches!(
                    result,
                    Some(ParseResult::Command(VimCommand {
                        count: Some(2),
                        operator: VimOperator::Delete,
                        motion: VimMotion::InnerWord,
                    }))
                ),
                "sequence {seq:?} got {result:?}"
            );
        }
    }

    #[test]
    fn test_text_object_invalid_char_resets_pending_state() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('2'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('d'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('i'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        assert!(parser
            .parse_key(&mut state, &Key::Char('z'), Modifiers::NONE)
            .is_none());
        assert!(!state.has_pending_operator());
        assert_eq!(state.pending_count, None);

        let next = parser.parse_key(&mut state, &Key::Char('w'), Modifiers::NONE);
        assert!(matches!(
            next,
            Some(ParseResult::Command(VimCommand {
                count: None,
                operator: VimOperator::None,
                motion: VimMotion::Word,
            }))
        ));
    }

    #[test]
    fn test_text_object_escape_aborts_cleanly() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();

        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('c'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('a'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        assert!(matches!(
            parser.parse_key(&mut state, &Key::Escape, Modifiers::NONE),
            Some(ParseResult::Action(VimAction::ExitToNormal))
        ));
        assert!(!state.has_pending_operator());
        assert_eq!(state.mode, VimMode::Normal);

        let next = parser.parse_key(&mut state, &Key::Char('i'), Modifiers::NONE);
        assert!(matches!(
            next,
            Some(ParseResult::Action(VimAction::InsertAtCursor))
        ));
    }

    #[test]
    fn test_visual_mode_text_objects() {
        let mut parser = KeyParser::new();
        let mut state = VimState::new();
        state.mode = VimMode::Visual;

        assert!(matches!(
            parser.parse_key(&mut state, &Key::Char('i'), Modifiers::NONE),
            Some(ParseResult::Pending)
        ));
        let res = parser.parse_key(&mut state, &Key::Char('w'), Modifiers::NONE);
        assert!(matches!(
            res,
            Some(ParseResult::Command(VimCommand {
                count: None,
                operator: VimOperator::None,
                motion: VimMotion::InnerWord,
            }))
        ));
    }
}
