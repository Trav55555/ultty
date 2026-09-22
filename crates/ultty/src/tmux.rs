//! Native tmux integration support.
//!
//! When Ultty itself runs inside tmux (nested mode), escape sequences that
//! must reach the outer terminal (for example OSC 52 clipboard writes) are
//! wrapped in DCS `tmux;` passthrough sequences, and inbound passthrough
//! sequences are unwrapped.

use std::env;
use std::process::Command;

/// How Ultty relates to tmux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxMode {
    /// Not running inside tmux.
    None,
    /// Running inside a tmux session (passthrough required).
    Nested,
}

/// Parser state for DCS passthrough sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PassthroughState {
    Normal,
    Escape,
    DcsStart,
    InPassthrough,
    PassthroughEscape,
}

/// Tmux passthrough support.
pub struct TmuxController {
    /// Current mode
    mode: TmuxMode,
    /// Passthrough parser state
    passthrough_state: PassthroughState,
    /// Buffer for passthrough content
    passthrough_buffer: Vec<u8>,
}

impl TmuxController {
    /// Create a new tmux controller.
    pub fn new() -> Self {
        Self {
            mode: Self::detect_mode(),
            passthrough_state: PassthroughState::Normal,
            passthrough_buffer: Vec::new(),
        }
    }

    /// Detect the current tmux mode based on environment.
    fn detect_mode() -> TmuxMode {
        if env::var("TMUX").is_ok() {
            TmuxMode::Nested
        } else {
            TmuxMode::None
        }
    }

    /// Get the current tmux mode.
    pub fn mode(&self) -> TmuxMode {
        self.mode
    }

    /// Check if we're running inside tmux.
    pub fn is_nested(&self) -> bool {
        self.mode == TmuxMode::Nested
    }

    /// Process data for passthrough sequences.
    ///
    /// When running inside tmux (nested mode), certain escape sequences need
    /// to be wrapped in DCS passthrough sequences to reach the outer terminal.
    ///
    /// Returns (processed_data, extracted_passthrough_content).
    pub fn process_passthrough(&mut self, data: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut output = Vec::with_capacity(data.len());
        let mut extracted = Vec::new();

        for &byte in data {
            match self.passthrough_state {
                PassthroughState::Normal => {
                    if byte == 0x1b {
                        self.passthrough_state = PassthroughState::Escape;
                    } else {
                        output.push(byte);
                    }
                }
                PassthroughState::Escape => {
                    if byte == b'P' {
                        self.passthrough_state = PassthroughState::DcsStart;
                    } else {
                        // Not a DCS, emit the ESC and this byte
                        output.push(0x1b);
                        output.push(byte);
                        self.passthrough_state = PassthroughState::Normal;
                    }
                }
                PassthroughState::DcsStart => {
                    // Check for "tmux;" prefix
                    self.passthrough_buffer.push(byte);
                    if self.passthrough_buffer.len() >= 5 {
                        if &self.passthrough_buffer[..5] == b"tmux;" {
                            self.passthrough_buffer.clear();
                            self.passthrough_state = PassthroughState::InPassthrough;
                        } else {
                            // Not a tmux passthrough, emit what we have
                            output.push(0x1b);
                            output.push(b'P');
                            output.extend_from_slice(&self.passthrough_buffer);
                            self.passthrough_buffer.clear();
                            self.passthrough_state = PassthroughState::Normal;
                        }
                    }
                }
                PassthroughState::InPassthrough => {
                    if byte == 0x1b {
                        self.passthrough_state = PassthroughState::PassthroughEscape;
                    } else {
                        self.passthrough_buffer.push(byte);
                    }
                }
                PassthroughState::PassthroughEscape => {
                    if byte == b'\\' {
                        // End of passthrough (ST = ESC \)
                        // The passthrough content has doubled ESCs, so unescape
                        let content = self.unescape_passthrough(&self.passthrough_buffer);
                        extracted.push(content);
                        self.passthrough_buffer.clear();
                        self.passthrough_state = PassthroughState::Normal;
                    } else if byte == 0x1b {
                        // Doubled ESC inside passthrough, emit one
                        self.passthrough_buffer.push(0x1b);
                    } else {
                        // ESC followed by something else
                        self.passthrough_buffer.push(0x1b);
                        self.passthrough_buffer.push(byte);
                        self.passthrough_state = PassthroughState::InPassthrough;
                    }
                }
            }
        }

        (output, extracted)
    }

    /// Unescape passthrough content (doubled ESCs become single).
    fn unescape_passthrough(&self, data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        let mut i = 0;

        while i < data.len() {
            if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == 0x1b {
                result.push(0x1b);
                i += 2;
            } else {
                result.push(data[i]);
                i += 1;
            }
        }

        result
    }

    /// Wrap data in tmux passthrough sequence for sending to outer terminal.
    ///
    /// Use this when running inside tmux and need to send sequences that
    /// should reach the terminal outside tmux (e.g., clipboard OSC sequences).
    pub fn wrap_passthrough(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len() * 2 + 10);

        // DCS tmux;
        result.extend_from_slice(b"\x1bPtmux;");

        // Double all ESC characters
        for &byte in data {
            if byte == 0x1b {
                result.push(0x1b);
                result.push(0x1b);
            } else {
                result.push(byte);
            }
        }

        // ST (String Terminator)
        result.extend_from_slice(b"\x1b\\");

        result
    }

    /// Check if tmux is available on the system.
    pub fn is_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get tmux version.
    pub fn version() -> Option<String> {
        Command::new("tmux")
            .arg("-V")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    }
}

impl Default for TmuxController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TmuxMode enum ──────────────────────────────────────────────

    #[test]
    fn tmux_mode_clone_and_eq() {
        let modes = [TmuxMode::None, TmuxMode::Nested];
        for mode in &modes {
            let cloned = *mode;
            assert_eq!(*mode, cloned);
        }
        assert_ne!(TmuxMode::None, TmuxMode::Nested);
    }

    #[test]
    fn tmux_mode_debug() {
        let s = format!("{:?}", TmuxMode::Nested);
        assert!(s.contains("Nested"));
    }

    #[test]
    fn controller_new_detects_a_mode() {
        let c = TmuxController::new();
        assert!(matches!(c.mode(), TmuxMode::None | TmuxMode::Nested));
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
        assert!(c.passthrough_buffer.is_empty());
    }

    #[test]
    fn controller_default_same_as_new() {
        let a = TmuxController::new();
        let b = TmuxController::default();
        assert_eq!(a.mode(), b.mode());
    }

    #[test]
    fn is_nested_follows_mode() {
        let mut c = TmuxController::new();
        c.mode = TmuxMode::Nested;
        assert!(c.is_nested());

        c.mode = TmuxMode::None;
        assert!(!c.is_nested());
    }

    #[test]
    fn wrap_passthrough_empty_data() {
        let wrapped = TmuxController::wrap_passthrough(b"");
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\\");
    }

    #[test]
    fn wrap_passthrough_no_esc() {
        let wrapped = TmuxController::wrap_passthrough(b"Hello");
        assert_eq!(wrapped, b"\x1bPtmux;Hello\x1b\\");
    }

    #[test]
    fn wrap_passthrough_doubles_esc() {
        let data = b"\x1b[31mRed\x1b[0m";
        let wrapped = TmuxController::wrap_passthrough(data);

        // Manually construct expected:
        //   ESC P t m u x ;  ESC ESC [ 3 1 m R e d ESC ESC [ 0 m  ESC \
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1bPtmux;");
        expected.push(0x1b);
        expected.push(0x1b);
        expected.extend_from_slice(b"[31mRed");
        expected.push(0x1b);
        expected.push(0x1b);
        expected.extend_from_slice(b"[0m");
        expected.extend_from_slice(b"\x1b\\");

        assert_eq!(wrapped, expected);
    }

    #[test]
    fn wrap_passthrough_all_esc_bytes() {
        // Data consisting only of ESC characters
        let data = b"\x1b\x1b\x1b";
        let wrapped = TmuxController::wrap_passthrough(data);
        // Each ESC doubled, plus header and trailer
        let expected = b"\x1bPtmux;\x1b\x1b\x1b\x1b\x1b\x1b\x1b\\";
        assert_eq!(wrapped, expected.as_ref());
    }

    #[test]
    fn unescape_passthrough_no_esc() {
        let c = TmuxController::new();
        let result = c.unescape_passthrough(b"Hello World");
        assert_eq!(result, b"Hello World");
    }

    #[test]
    fn unescape_passthrough_doubled_esc() {
        let c = TmuxController::new();
        let data = [0x1b, 0x1b, b'[', b'3', b'1', b'm'];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'[', b'3', b'1', b'm']);
    }

    #[test]
    fn unescape_passthrough_single_esc_kept() {
        let c = TmuxController::new();
        // A single ESC followed by non-ESC should be kept as-is
        let data = [0x1b, b'['];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'[']);
    }

    #[test]
    fn unescape_passthrough_empty() {
        let c = TmuxController::new();
        let result = c.unescape_passthrough(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn unescape_passthrough_trailing_single_esc() {
        let c = TmuxController::new();
        // Trailing single ESC
        let data = [b'A', 0x1b];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[b'A', 0x1b]);
    }

    #[test]
    fn unescape_passthrough_multiple_doubled() {
        let c = TmuxController::new();
        // Two doubled ESC pairs with content between them
        let data = [0x1b, 0x1b, b'X', 0x1b, 0x1b, b'Y'];
        let result = c.unescape_passthrough(&data);
        assert_eq!(result, &[0x1b, b'X', 0x1b, b'Y']);
    }

    #[test]
    fn process_passthrough_normal_data() {
        let mut c = TmuxController::new();
        let (out, ext) = c.process_passthrough(b"plain text 123");
        assert_eq!(out, b"plain text 123");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_esc_non_dcs() {
        let mut c = TmuxController::new();
        // ESC followed by something other than 'P' => both emitted
        let (out, ext) = c.process_passthrough(b"\x1b[31m");
        assert_eq!(out, b"\x1b[31m");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_dcs_not_tmux() {
        let mut c = TmuxController::new();
        // ESC P followed by non-"tmux;" prefix
        let (out, ext) = c.process_passthrough(b"\x1bPotherXYZ");
        // Should emit ESC, P, and then the buffer contents
        assert!(out.starts_with(&[0x1b, b'P']));
        assert!(ext.is_empty());
    }

    #[test]
    fn process_passthrough_full_sequence() {
        let mut c = TmuxController::new();
        // Build: ESC P t m u x ; <doubled-esc>[31m ESC \
        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        // Doubled ESC inside passthrough
        input.push(0x1b);
        input.push(0x1b);
        input.extend_from_slice(b"[31m");
        // ST = ESC backslash
        input.push(0x1b);
        input.push(b'\\');

        let (out, ext) = c.process_passthrough(&input);
        assert!(out.is_empty(), "No output bytes expected");
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], b"\x1b[31m");
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_data_before_and_after() {
        let mut c = TmuxController::new();
        let mut input = Vec::new();
        input.extend_from_slice(b"before");
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"inner");
        input.push(0x1b);
        input.push(b'\\');
        input.extend_from_slice(b"after");

        let (out, ext) = c.process_passthrough(&input);
        assert_eq!(&out, b"beforeafter");
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], b"inner");
    }

    #[test]
    fn process_passthrough_two_sequences() {
        let mut c = TmuxController::new();

        // First passthrough
        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"seq1");
        input.push(0x1b);
        input.push(b'\\');
        // Second passthrough
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"seq2");
        input.push(0x1b);
        input.push(b'\\');

        let (out, ext) = c.process_passthrough(&input);
        assert!(out.is_empty());
        assert_eq!(ext.len(), 2);
        assert_eq!(ext[0], b"seq1");
        assert_eq!(ext[1], b"seq2");
    }

    #[test]
    fn process_passthrough_incremental() {
        // Feed the passthrough byte-by-byte to test state persistence
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.extend_from_slice(b"hello");
        input.push(0x1b);
        input.push(b'\\');

        let mut all_out = Vec::new();
        let mut all_ext = Vec::new();

        for &byte in &input {
            let (out, ext) = c.process_passthrough(&[byte]);
            all_out.extend_from_slice(&out);
            all_ext.extend(ext);
        }

        assert!(all_out.is_empty());
        assert_eq!(all_ext.len(), 1);
        assert_eq!(all_ext[0], b"hello");
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn process_passthrough_esc_esc_in_content() {
        // Doubled ESC inside passthrough means a single ESC in the extracted content
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        // Doubled ESC (the passthrough_state machine handles this:
        // first ESC -> PassthroughEscape, second ESC -> push one ESC, back to InPassthrough)
        input.push(0x1b);
        input.push(0x1b);
        input.push(b'X');
        input.push(0x1b);
        input.push(b'\\');

        let (_out, ext) = c.process_passthrough(&input);
        assert_eq!(ext.len(), 1);
        // The state machine pushes one ESC for doubled ESC, then 'X'
        assert_eq!(ext[0], &[0x1b, b'X']);
    }

    #[test]
    fn process_passthrough_esc_other_in_content() {
        // ESC followed by non-ESC, non-backslash inside passthrough
        let mut c = TmuxController::new();

        let mut input = Vec::new();
        input.push(0x1b);
        input.push(b'P');
        input.extend_from_slice(b"tmux;");
        input.push(b'A');
        // ESC followed by 'Z' (not ESC or backslash)
        input.push(0x1b);
        input.push(b'Z');
        input.push(b'B');
        // End
        input.push(0x1b);
        input.push(b'\\');

        let (_out, ext) = c.process_passthrough(&input);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], &[b'A', 0x1b, b'Z', b'B']);
    }

    #[test]
    fn process_passthrough_partial_dcs_prefix() {
        // ESC P followed by only 3 bytes (not enough for "tmux;")
        // then more data that makes it not "tmux;"
        let mut c = TmuxController::new();
        let (out, ext) = c.process_passthrough(b"\x1bPabcde");
        // "abcde" != "tmux;", so should emit ESC P + buffer
        assert_eq!(out, b"\x1bPabcde");
        assert!(ext.is_empty());
        assert_eq!(c.passthrough_state, PassthroughState::Normal);
    }

    #[test]
    fn wrap_then_process_roundtrip() {
        let original = b"\x1b[31mHello\x1b[0m World";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    #[test]
    fn wrap_then_process_roundtrip_no_esc() {
        let original = b"simple text";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    #[test]
    fn wrap_then_process_roundtrip_only_esc() {
        let original = b"\x1b";
        let wrapped = TmuxController::wrap_passthrough(original);

        let mut c = TmuxController::new();
        let (_out, ext) = c.process_passthrough(&wrapped);
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0], original.as_ref());
    }

    #[test]
    fn detect_mode_returns_valid_variant() {
        let mode = TmuxController::detect_mode();
        assert!(matches!(mode, TmuxMode::None | TmuxMode::Nested));
    }

    #[test]
    fn is_available_does_not_panic() {
        let _ = TmuxController::is_available();
    }

    #[test]
    fn version_does_not_panic() {
        let _ = TmuxController::version();
    }

    #[test]
    fn passthrough_state_eq_and_clone() {
        let states = [
            PassthroughState::Normal,
            PassthroughState::Escape,
            PassthroughState::DcsStart,
            PassthroughState::InPassthrough,
            PassthroughState::PassthroughEscape,
        ];
        for s in &states {
            assert_eq!(s, &s.clone());
        }
        assert_ne!(PassthroughState::Normal, PassthroughState::Escape);
    }
}
