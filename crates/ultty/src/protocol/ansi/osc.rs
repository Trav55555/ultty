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
fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
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
