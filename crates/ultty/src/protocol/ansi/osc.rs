//! OSC (Operating System Command), DCS, APC, and other string sequence handling.

use super::{Action, State};
use super::AnsiParser;

impl AnsiParser {
    /// OSC string state - accumulating OSC content.
    pub(super) fn osc_string<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ESC - might be start of ST (String Terminator = ESC \)
            0x1B => {
                self.state = State::OscStringEsc;
            }
            // BEL also terminates OSC
            0x07 => {
                self.dispatch_osc(callback);
                self.reset();
            }
            _ => {
                self.osc_buffer.push(byte);
            }
        }
    }

    /// OSC string ESC state - waiting for backslash to complete ST.
    pub(super) fn osc_string_esc<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.dispatch_osc(callback);
                self.reset();
            }
            // Any other byte - was not a valid ST, dispatch and reprocess
            _ => {
                self.dispatch_osc(callback);
                self.reset();
                // Reprocess this byte from ground state
                self.process_byte(byte, callback);
            }
        }
    }

    /// Dispatch OSC sequence based on accumulated buffer.
    fn dispatch_osc<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        let content = String::from_utf8_lossy(&self.osc_buffer);

        // OSC format: code ; data
        if let Some(semicolon_pos) = content.find(';') {
            let code = &content[..semicolon_pos];
            let data = &content[semicolon_pos + 1..];

            match code {
                // Set window title (OSC 0, 1, 2)
                "0" | "1" | "2" => {
                    callback(Action::SetTitle(data.to_string()));
                }
                // Set working directory (OSC 7)
                // Format: file://hostname/path
                "7" => {
                    callback(Action::SetWorkingDirectory(data.to_string()));
                }
                // Clipboard (OSC 52)
                // Format: clipboard;base64data
                "52" => {
                    if let Some(semi_pos) = data.find(';') {
                        let clipboard = data.chars().next().unwrap_or('c');
                        let b64data = &data[semi_pos + 1..];
                        // Decode base64
                        if let Ok(decoded) = base64_decode(b64data) {
                            if let Ok(text) = String::from_utf8(decoded) {
                                callback(Action::SetClipboard {
                                    clipboard,
                                    data: text,
                                });
                            }
                        }
                    }
                }
                // Hyperlinks (OSC 8)
                // Format: 8;params;uri (params typically empty or id=xxx)
                "8" => {
                    // data format: params;uri
                    if let Some(semi_pos) = data.find(';') {
                        let _params = &data[..semi_pos];
                        let uri = &data[semi_pos + 1..];
                        if uri.is_empty() {
                            // End hyperlink
                            callback(Action::EndHyperlink);
                        } else {
                            // Start hyperlink
                            callback(Action::StartHyperlink(uri.to_string()));
                        }
                    }
                }
                // Desktop notification - iTerm2 style (OSC 9)
                "9" => {
                    callback(Action::Notification {
                        title: String::new(),
                        body: data.to_string(),
                    });
                }
                // Shell integration (OSC 133)
                "133" => {
                    self.dispatch_shell_integration(data, callback);
                }
                // Desktop notification - rxvt/urxvt style (OSC 777)
                // Format: 777;notify;title;body
                "777" => {
                    let parts: Vec<&str> = data.splitn(3, ';').collect();
                    if parts.first() == Some(&"notify") {
                        let title = parts.get(1).unwrap_or(&"").to_string();
                        let body = parts.get(2).unwrap_or(&"").to_string();
                        callback(Action::Notification { title, body });
                    }
                }
                // Foreground color query/set (OSC 10)
                "10" => {
                    if data == "?" {
                        callback(Action::QueryForegroundColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Background color query/set (OSC 11)
                "11" => {
                    if data == "?" {
                        callback(Action::QueryBackgroundColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Cursor color query/set (OSC 12)
                "12" => {
                    if data == "?" {
                        callback(Action::QueryCursorColor);
                    }
                    // Setting colors not implemented - ignore
                }
                // Kitty character width protocol (OSC 66)
                // Format: 66;w=N;CHAR or 66;s=N;CHAR
                // We print the character and ignore the width hint (monospace terminal)
                "66" => {
                    // data format: w=N;CHAR or s=N;CHAR
                    // Find the second semicolon to get the character(s)
                    if let Some(semi_pos) = data.find(';') {
                        let chars = &data[semi_pos + 1..];
                        // Print each character
                        for c in chars.chars() {
                            callback(Action::Print(c));
                        }
                    }
                }
                _ => {
                    // Unknown OSC - log for debugging
                    tracing::debug!("Unknown OSC sequence: {}", content);
                }
            }
        } else if let Ok(code) = content.parse::<u16>() {
            // Some OSC sequences have no data (just the code)
            if code == 133 {
                // Bare OSC 133 - treat as prompt start
                callback(Action::PromptStart);
            }
        }
    }

    /// Parse shell integration OSC 133 sequences.
    fn dispatch_shell_integration<F>(&self, data: &str, callback: &mut F)
    where
        F: FnMut(Action),
    {
        // OSC 133 ; type [; args]
        let mut parts = data.splitn(2, ';');
        let cmd_type = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("");

        match cmd_type {
            "A" => callback(Action::PromptStart),
            "B" => callback(Action::CommandStart),
            "C" => callback(Action::CommandExecuted),
            "D" => {
                let exit_code = args.parse::<i32>().ok();
                callback(Action::CommandFinished(exit_code));
            }
            _ => {
                tracing::debug!("Unknown shell integration type: {}", cmd_type);
            }
        }
    }

    /// DCS string state.
    pub(super) fn dcs_string<F>(&mut self, byte: u8, _callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // ESC - might be start of ST (String Terminator = ESC \)
            0x1B => {
                self.state = State::DcsStringEsc;
            }
            // CAN/SUB abort the sequence
            0x18 | 0x1A => {
                self.reset();
            }
            _ => {
                // Accumulate DCS content
                self.dcs_buffer.push(byte);
            }
        }
    }

    /// DCS string ESC state - waiting for backslash to complete ST.
    pub(super) fn dcs_string_esc<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.dispatch_dcs(callback);
                self.reset();
            }
            // Any other byte - was not a valid ST, dispatch and reprocess
            _ => {
                self.dispatch_dcs(callback);
                self.reset();
                // Reprocess this byte from ground state
                self.process_byte(byte, callback);
            }
        }
    }

    /// APC string state - accumulating APC content (Kitty graphics, etc.).
    /// We currently just ignore APC content but need to consume it properly.
    pub(super) fn apc_string(&mut self, byte: u8) {
        if byte == 0x1B {
            // ESC - might be start of ST (String Terminator = ESC \)
            self.state = State::ApcStringEsc;
        }
        // Otherwise just consume the byte - we don't process APC content yet
    }

    /// APC string ESC state - waiting for backslash to complete ST.
    pub(super) fn apc_string_esc(&mut self, byte: u8) {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                self.reset();
            }
            // Any other byte - not a valid ST, continue in APC mode
            _ => {
                self.state = State::ApcString;
            }
        }
    }

    /// Generic handler for ignored string sequences (PM, SOS, etc.)
    pub(super) fn ignored_string(&mut self, byte: u8, esc_state: State) {
        if byte == 0x1B {
            self.state = esc_state;
        }
        // Otherwise just consume the byte
    }

    /// Generic handler for ESC in ignored string sequences.
    pub(super) fn ignored_string_esc(&mut self, byte: u8, string_state: State) {
        match byte {
            b'\\' => self.reset(),
            _ => self.state = string_state,
        }
    }

    /// Dispatch DCS sequence (primarily Sixel).
    fn dispatch_dcs<F>(&mut self, callback: &mut F)
    where
        F: FnMut(Action),
    {
        if self.dcs_buffer.is_empty() {
            return;
        }

        // Check for Sixel: first character should be 'q' after params
        // DCS format: ESC P params q <sixel data> ST
        // When we get here, params are in dcs_params, buffer starts with 'q' or params

        // Find the 'q' that starts sixel data
        let content = &self.dcs_buffer;
        if let Some(q_pos) = content.iter().position(|&b| b == b'q') {
            // Parse params before 'q' (semicolon-separated)
            let param_bytes = &content[..q_pos];
            let mut params = Vec::new();
            let mut current = 0u16;

            for &b in param_bytes {
                match b {
                    b'0'..=b'9' => {
                        current = current.saturating_mul(10).saturating_add((b - b'0') as u16);
                    }
                    b';' => {
                        params.push(current);
                        current = 0;
                    }
                    _ => {}
                }
            }
            params.push(current); // Push last param

            // Get sixel data after 'q'
            let sixel_data = content[q_pos + 1..].to_vec();

            if !sixel_data.is_empty() {
                callback(Action::SixelImage {
                    aspect_ratio: params.first().copied().unwrap_or(0),
                    background: params.get(1).copied().unwrap_or(0),
                    horizontal_grid: params.get(2).copied().unwrap_or(0),
                    data: sixel_data,
                });
            }
        }
    }
}

/// Simple base64 decoder for OSC 52 clipboard data.
pub(super) fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn char_to_val(c: u8) -> Option<u8> {
        ALPHABET.iter().position(|&x| x == c).map(|p| p as u8)
    }

    let input: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'=' && b != b'\n' && b != b'\r')
        .collect();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);

    for chunk in input.chunks(4) {
        let vals: Vec<u8> = chunk.iter().filter_map(|&c| char_to_val(c)).collect();
        if vals.len() >= 2 {
            output.push((vals[0] << 2) | (vals[1] >> 4));
            if vals.len() >= 3 {
                output.push((vals[1] << 4) | (vals[2] >> 2));
                if vals.len() >= 4 {
                    output.push((vals[2] << 6) | vals[3]);
                }
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::super::{Action, AnsiParser};

    /// Feed raw bytes through the parser and collect emitted actions.
    fn parse(input: &[u8]) -> Vec<Action> {
        let mut parser = AnsiParser::new();
        let mut actions = Vec::new();
        parser.feed(input, |a| actions.push(a));
        actions
    }

    // ------------------------------------------------------------------
    // OSC title changes (OSC 0, 1, 2)
    // ------------------------------------------------------------------

    #[test]
    fn osc_set_title_code_0_bel() {
        assert_eq!(
            parse(b"\x1b]0;Hello World\x07"),
            vec![Action::SetTitle("Hello World".to_string())]
        );
    }

    #[test]
    fn osc_set_title_code_1_bel() {
        assert_eq!(
            parse(b"\x1b]1;Icon Title\x07"),
            vec![Action::SetTitle("Icon Title".to_string())]
        );
    }

    #[test]
    fn osc_set_title_code_2_bel() {
        assert_eq!(
            parse(b"\x1b]2;Window Title\x07"),
            vec![Action::SetTitle("Window Title".to_string())]
        );
    }

    #[test]
    fn osc_set_title_st_terminator() {
        // ST terminator = ESC backslash
        assert_eq!(
            parse(b"\x1b]0;Title via ST\x1b\\"),
            vec![Action::SetTitle("Title via ST".to_string())]
        );
    }

    #[test]
    fn osc_set_title_empty() {
        assert_eq!(
            parse(b"\x1b]0;\x07"),
            vec![Action::SetTitle(String::new())]
        );
    }

    #[test]
    fn osc_title_with_special_chars() {
        assert_eq!(
            parse(b"\x1b]0;user@host: ~/dir\x07"),
            vec![Action::SetTitle("user@host: ~/dir".to_string())]
        );
    }

    // ------------------------------------------------------------------
    // OSC clipboard (OSC 52)
    // ------------------------------------------------------------------

    #[test]
    fn osc_clipboard_basic() {
        // "Hello" in base64 is "SGVsbG8="
        assert_eq!(
            parse(b"\x1b]52;c;SGVsbG8=\x07"),
            vec![Action::SetClipboard {
                clipboard: 'c',
                data: "Hello".to_string(),
            }]
        );
    }

    #[test]
    fn osc_clipboard_primary_selection() {
        // "test" in base64 is "dGVzdA=="
        assert_eq!(
            parse(b"\x1b]52;p;dGVzdA==\x07"),
            vec![Action::SetClipboard {
                clipboard: 'p',
                data: "test".to_string(),
            }]
        );
    }

    #[test]
    fn osc_clipboard_with_st_terminator() {
        assert_eq!(
            parse(b"\x1b]52;c;SGVsbG8=\x1b\\"),
            vec![Action::SetClipboard {
                clipboard: 'c',
                data: "Hello".to_string(),
            }]
        );
    }

    #[test]
    fn osc_clipboard_invalid_base64_no_crash() {
        // Invalid base64 should not crash; it just won't produce an action
        let actions = parse(b"\x1b]52;c;!!!invalid!!!\x07");
        // The base64 decoder is lenient - it skips invalid chars
        // so this may or may not produce an action, but must not panic
        let _ = actions;
    }

    // ------------------------------------------------------------------
    // OSC color queries (OSC 10, 11, 12)
    // ------------------------------------------------------------------

    #[test]
    fn osc_query_foreground_color() {
        assert_eq!(
            parse(b"\x1b]10;?\x07"),
            vec![Action::QueryForegroundColor]
        );
    }

    #[test]
    fn osc_query_background_color() {
        assert_eq!(
            parse(b"\x1b]11;?\x07"),
            vec![Action::QueryBackgroundColor]
        );
    }

    #[test]
    fn osc_query_cursor_color() {
        assert_eq!(
            parse(b"\x1b]12;?\x07"),
            vec![Action::QueryCursorColor]
        );
    }

    #[test]
    fn osc_color_set_ignored() {
        // Setting colors (non-query) should not emit any action
        let actions = parse(b"\x1b]10;rgb:ff/ff/ff\x07");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // OSC hyperlinks (OSC 8)
    // ------------------------------------------------------------------

    #[test]
    fn osc_hyperlink_start() {
        assert_eq!(
            parse(b"\x1b]8;;https://example.com\x07"),
            vec![Action::StartHyperlink("https://example.com".to_string())]
        );
    }

    #[test]
    fn osc_hyperlink_start_with_id_param() {
        assert_eq!(
            parse(b"\x1b]8;id=link1;https://github.com\x07"),
            vec![Action::StartHyperlink("https://github.com".to_string())]
        );
    }

    #[test]
    fn osc_hyperlink_end() {
        assert_eq!(parse(b"\x1b]8;;\x07"), vec![Action::EndHyperlink]);
    }

    #[test]
    fn osc_hyperlink_with_st_terminator() {
        assert_eq!(
            parse(b"\x1b]8;;https://rust-lang.org\x1b\\"),
            vec![Action::StartHyperlink("https://rust-lang.org".to_string())]
        );
    }

    // ------------------------------------------------------------------
    // OSC with different terminators (BEL vs ST)
    // ------------------------------------------------------------------

    #[test]
    fn osc_bel_terminator() {
        assert_eq!(
            parse(b"\x1b]0;BEL terminated\x07"),
            vec![Action::SetTitle("BEL terminated".to_string())]
        );
    }

    #[test]
    fn osc_st_terminator() {
        assert_eq!(
            parse(b"\x1b]0;ST terminated\x1b\\"),
            vec![Action::SetTitle("ST terminated".to_string())]
        );
    }

    #[test]
    fn osc_st_followed_by_text() {
        // After ST, the parser should return to ground state and handle normal text
        assert_eq!(
            parse(b"\x1b]0;Title\x1b\\A"),
            vec![
                Action::SetTitle("Title".to_string()),
                Action::Print('A'),
            ]
        );
    }

    // ------------------------------------------------------------------
    // Malformed/truncated OSC sequences
    // ------------------------------------------------------------------

    #[test]
    fn osc_no_semicolon_unknown_code() {
        // OSC with no semicolon and unrecognized code - should not crash
        let actions = parse(b"\x1b]999\x07");
        assert!(actions.is_empty());
    }

    #[test]
    fn osc_empty_buffer() {
        // OSC immediately terminated - empty buffer
        let actions = parse(b"\x1b]\x07");
        assert!(actions.is_empty());
    }

    #[test]
    fn osc_bare_133() {
        // Bare OSC 133 without data = prompt start
        assert_eq!(parse(b"\x1b]133\x07"), vec![Action::PromptStart]);
    }

    #[test]
    fn osc_unknown_code_with_data() {
        // Unknown OSC code should be silently ignored
        let actions = parse(b"\x1b]999;some data\x07");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // OSC shell integration (OSC 133)
    // ------------------------------------------------------------------

    #[test]
    fn osc_shell_integration_prompt_start() {
        assert_eq!(parse(b"\x1b]133;A\x07"), vec![Action::PromptStart]);
    }

    #[test]
    fn osc_shell_integration_command_start() {
        assert_eq!(parse(b"\x1b]133;B\x07"), vec![Action::CommandStart]);
    }

    #[test]
    fn osc_shell_integration_command_executed() {
        assert_eq!(parse(b"\x1b]133;C\x07"), vec![Action::CommandExecuted]);
    }

    #[test]
    fn osc_shell_integration_command_finished_zero() {
        assert_eq!(
            parse(b"\x1b]133;D;0\x07"),
            vec![Action::CommandFinished(Some(0))]
        );
    }

    #[test]
    fn osc_shell_integration_command_finished_nonzero() {
        assert_eq!(
            parse(b"\x1b]133;D;127\x07"),
            vec![Action::CommandFinished(Some(127))]
        );
    }

    #[test]
    fn osc_shell_integration_command_finished_no_exit_code() {
        // "D" with no args - exit code should be None (empty string doesn't parse)
        assert_eq!(
            parse(b"\x1b]133;D\x07"),
            vec![Action::CommandFinished(None)]
        );
    }

    #[test]
    fn osc_shell_integration_unknown_type() {
        // Unknown shell integration type - should not emit any action
        let actions = parse(b"\x1b]133;Z\x07");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // OSC notifications (OSC 9, OSC 777)
    // ------------------------------------------------------------------

    #[test]
    fn osc_iterm2_notification() {
        assert_eq!(
            parse(b"\x1b]9;Build complete!\x07"),
            vec![Action::Notification {
                title: String::new(),
                body: "Build complete!".to_string(),
            }]
        );
    }

    #[test]
    fn osc_rxvt_notification_with_title_and_body() {
        assert_eq!(
            parse(b"\x1b]777;notify;Build;Done\x07"),
            vec![Action::Notification {
                title: "Build".to_string(),
                body: "Done".to_string(),
            }]
        );
    }

    #[test]
    fn osc_rxvt_notification_non_notify_ignored() {
        // OSC 777 with something other than "notify" - should be ignored
        let actions = parse(b"\x1b]777;other;stuff\x07");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // OSC working directory (OSC 7)
    // ------------------------------------------------------------------

    #[test]
    fn osc_working_directory() {
        assert_eq!(
            parse(b"\x1b]7;file://localhost/home/user\x07"),
            vec![Action::SetWorkingDirectory(
                "file://localhost/home/user".to_string()
            )]
        );
    }

    // ------------------------------------------------------------------
    // base64_decode unit tests
    // ------------------------------------------------------------------

    #[test]
    fn base64_decode_hello() {
        let decoded = super::base64_decode("SGVsbG8=").unwrap();
        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn base64_decode_empty() {
        let decoded = super::base64_decode("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn base64_decode_padding_stripped() {
        // base64 with padding
        let decoded = super::base64_decode("YQ==").unwrap();
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn base64_decode_no_padding() {
        // base64 without padding should also work
        let decoded = super::base64_decode("YQ").unwrap();
        assert_eq!(decoded, b"a");
    }

    #[test]
    fn base64_decode_ignores_newlines() {
        let decoded = super::base64_decode("SGVs\nbG8=").unwrap();
        assert_eq!(decoded, b"Hello");
    }

    // ------------------------------------------------------------------
    // DCS / Sixel
    // ------------------------------------------------------------------

    #[test]
    fn dcs_sixel_basic() {
        // DCS q <sixel_data> ST
        // Minimal sixel: ESC P q <data> ESC backslash
        let actions = parse(b"\x1bPq#0!4~\x1b\\");
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::SixelImage { data, .. } => {
                assert_eq!(data, b"#0!4~");
            }
            other => panic!("Expected SixelImage, got {:?}", other),
        }
    }

    #[test]
    fn dcs_sixel_with_params() {
        // DCS 0;1;0 q <data> ST
        let actions = parse(b"\x1bP0;1;0q~\x1b\\");
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::SixelImage {
                aspect_ratio,
                background,
                horizontal_grid,
                data,
            } => {
                assert_eq!(*aspect_ratio, 0);
                assert_eq!(*background, 1);
                assert_eq!(*horizontal_grid, 0);
                assert_eq!(data, b"~");
            }
            other => panic!("Expected SixelImage, got {:?}", other),
        }
    }

    #[test]
    fn dcs_empty_no_action() {
        // Empty DCS should produce no action
        let actions = parse(b"\x1bP\x1b\\");
        assert!(actions.is_empty());
    }

    // ------------------------------------------------------------------
    // APC sequences (consumed silently)
    // ------------------------------------------------------------------

    #[test]
    fn apc_consumed_silently() {
        let actions = parse(b"\x1b_Gi=31337,s=1,v=1,a=q\x1b\\");
        assert!(actions.is_empty());
    }

    #[test]
    fn apc_followed_by_text() {
        let actions = parse(b"\x1b_payload\x1b\\X");
        assert_eq!(actions, vec![Action::Print('X')]);
    }
}
