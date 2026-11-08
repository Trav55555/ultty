//! OSC (Operating System Command), DCS, APC, and other string sequence handling.

use super::AnsiParser;
use super::{
    Action, ApcKind, DynamicColor, Progress, State, StringTerminator, MAX_ANCHOR_APC,
    MAX_ANCHOR_ROWS, MAX_IN_BAND_APC,
};

/// Maximum size for OSC/DCS string buffers to prevent unbounded memory growth.
const MAX_STRING_BUFFER: usize = 65536;

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
                self.dispatch_osc(StringTerminator::Bel, callback);
                self.reset();
            }
            // CAN/SUB abort the sequence
            0x18 | 0x1A => {
                self.reset();
            }
            _ => {
                if self.osc_buffer.len() < MAX_STRING_BUFFER {
                    self.osc_buffer.push(byte);
                } else {
                    self.reset();
                }
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
                self.dispatch_osc(StringTerminator::St, callback);
                self.reset();
            }
            // Any other byte - was not a valid ST, dispatch and reprocess
            _ => {
                self.dispatch_osc(StringTerminator::St, callback);
                self.reset();
                // The ESC ended the string and starts the next sequence
                // (xterm), so `byte` is read after it, not printed.
                self.process_byte(0x1b, callback);
                self.process_byte(byte, callback);
            }
        }
    }

    /// Dispatch OSC sequence based on accumulated buffer.
    fn dispatch_osc<F>(&mut self, terminator: StringTerminator, callback: &mut F)
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
                        // Format: Pc;base64data where Pc is clipboard name (c, p, s, 0, etc.)
                        let clipboard = data[..semi_pos].chars().next().unwrap_or('c');
                        let b64data = &data[semi_pos + 1..];
                        if b64data == "?" {
                            // A read request. Ultty does not hand the
                            // clipboard to programs, so it is refused; it
                            // must never decode to an empty write.
                            tracing::debug!("refused OSC 52 clipboard read");
                        } else if let Ok(decoded) = base64_decode(b64data) {
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
                // ConEmu's progress report (OSC 9;4;state;percent) shares
                // OSC 9 with iTerm2's notifications (OSC 9;body).
                "9" if data == "4" || data.starts_with("4;") => {
                    if let Some(progress) = parse_progress(&data[1..]) {
                        callback(Action::Progress(progress));
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
                // Dynamic colour queries (OSC 10, 11, 12). Each further
                // `;`-separated item names the next colour, as in xterm, so
                // `OSC 10;?;?` asks for the foreground and the background.
                // Setting these colours is not supported, so other items are
                // ignored.
                "10" | "11" | "12" => {
                    let colors = [
                        DynamicColor::Foreground,
                        DynamicColor::Background,
                        DynamicColor::Cursor,
                    ];
                    let first = match code {
                        "10" => 0,
                        "11" => 1,
                        _ => 2,
                    };
                    for (item, &color) in data.split(';').zip(&colors[first..]) {
                        if item == "?" {
                            callback(Action::QueryDynamicColor { color, terminator });
                        }
                    }
                }
                // Palette entries (OSC 4): pairs of index and `?` or a colour.
                "4" => {
                    let items: Vec<&str> = data.split(';').collect();
                    for pair in items.as_chunks::<2>().0 {
                        let Ok(index) = pair[0].parse::<u8>() else {
                            continue;
                        };
                        if pair[1] == "?" {
                            callback(Action::QueryPaletteColor { index, terminator });
                        } else if let Some(rgb) = parse_color_spec(pair[1]) {
                            callback(Action::SetPaletteColor { index, rgb });
                        }
                    }
                }
                // Palette reset (OSC 104): the listed entries, or all.
                "104" => {
                    if data.is_empty() {
                        callback(Action::ResetPaletteColor(None));
                    }
                    for index in data.split(';').filter_map(|i| i.parse::<u8>().ok()) {
                        callback(Action::ResetPaletteColor(Some(index)));
                    }
                }
                // Dynamic colour resets (OSC 110, 111, 112). The dynamic
                // colours cannot be changed, so they are always at their
                // defaults and there is nothing to do.
                "110" | "111" | "112" => {}
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
            match code {
                // Bare OSC 133 - treat as prompt start
                133 => callback(Action::PromptStart),
                // Bare OSC 104 resets the whole palette
                104 => callback(Action::ResetPaletteColor(None)),
                _ => {}
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
                if self.dcs_buffer.len() < MAX_STRING_BUFFER {
                    self.dcs_buffer.push(byte);
                } else {
                    self.reset();
                }
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
                // The ESC ended the string and starts the next sequence
                // (xterm), so `byte` is read after it, not printed.
                self.process_byte(0x1b, callback);
                self.process_byte(byte, callback);
            }
        }
    }

    /// APC string state. Ultty in-band strings (`U`) and inline anchors
    /// (`u`) are collected; others, such as kitty graphics, are consumed and
    /// ignored.
    pub(super) fn apc_string(&mut self, byte: u8) {
        if byte == 0x1B {
            // ESC - might be start of ST (String Terminator = ESC \)
            self.state = State::ApcStringEsc;
            return;
        }
        match self.apc_kind {
            ApcKind::Unknown => {
                self.apc_kind = match byte {
                    b'U' => ApcKind::InBand,
                    b'u' => ApcKind::Anchor,
                    _ => ApcKind::Other,
                };
            }
            _ => self.apc_extend(&[byte]),
        }
    }

    /// Add payload bytes to a collected string, dropping it if it grows past
    /// its limit ([`MAX_IN_BAND_APC`] or [`MAX_ANCHOR_APC`]).
    pub(super) fn apc_extend(&mut self, bytes: &[u8]) {
        let limit = match self.apc_kind {
            ApcKind::InBand => MAX_IN_BAND_APC,
            ApcKind::Anchor => MAX_ANCHOR_APC,
            _ => return,
        };
        if self.apc_buffer.len() + bytes.len() > limit {
            self.apc_kind = ApcKind::Oversized;
            self.apc_buffer = Vec::new();
            return;
        }
        self.apc_buffer.extend_from_slice(bytes);
    }

    /// APC string ESC state - waiting for backslash to complete ST.
    pub(super) fn apc_string_esc<F>(&mut self, byte: u8, callback: &mut F)
    where
        F: FnMut(Action),
    {
        match byte {
            // Backslash completes ST (String Terminator)
            b'\\' => {
                match self.apc_kind {
                    ApcKind::InBand => {
                        callback(Action::InBand(std::mem::take(&mut self.apc_buffer)));
                    }
                    ApcKind::Anchor => {
                        if let Some(action) = parse_anchor(&self.apc_buffer) {
                            callback(action);
                        }
                        self.apc_buffer.clear();
                    }
                    _ => {}
                }
                self.apc_kind = ApcKind::Unknown;
                self.reset();
            }
            // Any other byte - not a valid ST, continue in APC mode. An
            // anchor has no ESC in it, so it is malformed.
            _ => {
                if self.apc_kind == ApcKind::Anchor {
                    self.apc_kind = ApcKind::Other;
                    self.apc_buffer.clear();
                }
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

        // DECRQSS (`$q`) and XTGETTCAP (`+q`) also end in `q`; take them
        // first so they are never decoded as sixel.
        let content = &self.dcs_buffer;
        if let Some(request) = content.strip_prefix(b"$q") {
            callback(Action::RequestStatusString(
                String::from_utf8_lossy(request).into_owned(),
            ));
            return;
        }
        if let Some(request) = content.strip_prefix(b"+q") {
            callback(Action::RequestTermcap(
                String::from_utf8_lossy(request).into_owned(),
            ));
            return;
        }

        // Find the 'q' that starts sixel data
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

/// Parse an X11 colour specification of the forms xterm accepts from
/// applications: `rgb:R/G/B` with 1-4 hex digits per channel (scaled to 8
/// bits), and `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB` (the high
/// bits of each channel). Colour names are not supported.
pub(super) fn parse_color_spec(spec: &str) -> Option<[u8; 3]> {
    fn hex(digits: &str) -> Option<u32> {
        if digits.is_empty() || digits.len() > 4 {
            return None;
        }
        u32::from_str_radix(digits, 16).ok()
    }

    if let Some(channels) = spec.strip_prefix("rgb:") {
        let mut rgb = [0u8; 3];
        let mut parts = channels.split('/');
        for channel in &mut rgb {
            let digits = parts.next()?;
            // Validate before using the length: a long channel would
            // overflow the shift below.
            let value = hex(digits)?;
            let max = (1u32 << (4 * digits.len())) - 1;
            // Round to the nearest 8-bit value: ff -> 255, f -> 255, 8 -> 136.
            *channel = ((value * 255 + max / 2) / max) as u8;
        }
        return parts.next().is_none().then_some(rgb);
    }

    let digits = spec.strip_prefix('#')?;
    if digits.is_empty() || !digits.len().is_multiple_of(3) || digits.len() > 12 {
        return None;
    }
    let width = digits.len() / 3;
    let mut rgb = [0u8; 3];
    for (i, channel) in rgb.iter_mut().enumerate() {
        let value = hex(digits.get(i * width..(i + 1) * width)?)?;
        // Left-align to 16 bits and keep the top 8.
        *channel = ((value << (16 - 4 * width)) >> 8) as u8;
    }
    Some(rgb)
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

/// Parse what follows `OSC 9;4`: `;state;percent`, both optional. An
/// unknown state is ignored; percentages above 100 count as 100.
fn parse_progress(rest: &str) -> Option<Progress> {
    let mut fields = rest.strip_prefix(';').unwrap_or(rest).split(';');
    let state = match fields.next() {
        None | Some("") => 0,
        Some(state) => state.parse::<u8>().ok()?,
    };
    let percent = fields
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .map(|p| p.min(100) as u8);
    Some(match state {
        0 => Progress::Clear,
        1 => Progress::Normal(percent.unwrap_or(0)),
        2 => Progress::Error(percent),
        3 => Progress::Indeterminate,
        4 => Progress::Paused(percent),
        _ => return None,
    })
}

/// Parse an inline anchor's text after the `u`: `session;surface;rows`,
/// three decimal numbers with nothing else, `rows` from 1 to
/// [`MAX_ANCHOR_ROWS`].
fn parse_anchor(text: &[u8]) -> Option<Action> {
    fn number<T: std::str::FromStr>(field: &[u8]) -> Option<T> {
        if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(field).ok()?.parse().ok()
    }
    let mut fields = text.split(|&b| b == b';');
    let session_id = number(fields.next()?)?;
    let surface_id = number(fields.next()?)?;
    let rows: u16 = number(fields.next()?)?;
    if fields.next().is_some() || !(1..=MAX_ANCHOR_ROWS).contains(&rows) {
        return None;
    }
    Some(Action::InlineAnchor {
        session_id,
        surface_id,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Action, AnsiParser, DynamicColor, StringTerminator};

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
        assert_eq!(parse(b"\x1b]0;\x07"), vec![Action::SetTitle(String::new())]);
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

    fn query(color: DynamicColor, terminator: StringTerminator) -> Action {
        Action::QueryDynamicColor { color, terminator }
    }

    #[test]
    fn osc_dynamic_color_queries_keep_their_terminator() {
        use DynamicColor::*;
        use StringTerminator::*;
        assert_eq!(parse(b"\x1b]10;?\x07"), vec![query(Foreground, Bel)]);
        assert_eq!(parse(b"\x1b]11;?\x1b\\"), vec![query(Background, St)]);
        assert_eq!(parse(b"\x1b]12;?\x07"), vec![query(Cursor, Bel)]);
        // Later items name the following colours.
        assert_eq!(
            parse(b"\x1b]10;?;?;?\x07"),
            vec![
                query(Foreground, Bel),
                query(Background, Bel),
                query(Cursor, Bel)
            ]
        );
        assert_eq!(parse(b"\x1b]12;?;?\x07"), vec![query(Cursor, Bel)]);
    }

    #[test]
    fn osc_4_sets_and_queries_several_entries() {
        assert_eq!(
            parse(b"\x1b]4;1;#ff8000;200;?;300;?;x;?\x1b\\"),
            vec![
                Action::SetPaletteColor {
                    index: 1,
                    rgb: [0xff, 0x80, 0x00]
                },
                Action::QueryPaletteColor {
                    index: 200,
                    terminator: StringTerminator::St
                },
            ]
        );
    }

    #[test]
    fn osc_104_resets_listed_entries_or_all() {
        assert_eq!(
            parse(b"\x1b]104\x07\x1b]104;\x07\x1b]104;3;9\x07"),
            vec![
                Action::ResetPaletteColor(None),
                Action::ResetPaletteColor(None),
                Action::ResetPaletteColor(Some(3)),
                Action::ResetPaletteColor(Some(9)),
            ]
        );
    }

    #[test]
    fn color_specs() {
        use super::parse_color_spec as spec;
        assert_eq!(spec("rgb:ff/00/80"), Some([0xff, 0x00, 0x80]));
        assert_eq!(spec("rgb:f/8/0"), Some([0xff, 0x88, 0x00]));
        assert_eq!(spec("rgb:ffff/8000/0000"), Some([0xff, 0x80, 0x00]));
        assert_eq!(spec("rgb:fff/800/000"), Some([0xff, 0x80, 0x00]));
        assert_eq!(spec("#f80"), Some([0xf0, 0x80, 0x00]));
        assert_eq!(spec("#ff8000"), Some([0xff, 0x80, 0x00]));
        assert_eq!(spec("#ffff80000000"), Some([0xff, 0x80, 0x00]));
        assert_eq!(spec("rgb:ff/00"), None);
        assert_eq!(spec("rgb:ff/00/00/00"), None);
        assert_eq!(spec("rgb:fffff/0/0"), None);
        // Long enough to overflow a shift by the channel length.
        assert_eq!(spec("rgb:fffffffff/0/0"), None);
        assert_eq!(spec("rgb:/0/0"), None);
        assert_eq!(spec("#ff80"), None);
        assert_eq!(spec("red"), None);
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
            vec![Action::SetTitle("Title".to_string()), Action::Print('A'),]
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
    fn osc_9_4_is_progress_not_a_notification() {
        use super::Progress::*;
        let cases: [(&[u8], Vec<Action>); 10] = [
            (b"\x1b]9;4;1;50\x07", vec![Action::Progress(Normal(50))]),
            (b"\x1b]9;4;1\x07", vec![Action::Progress(Normal(0))]),
            (b"\x1b]9;4;1;250\x1b\\", vec![Action::Progress(Normal(100))]),
            (
                b"\x1b]9;4;2;30\x07",
                vec![Action::Progress(Error(Some(30)))],
            ),
            (b"\x1b]9;4;2\x07", vec![Action::Progress(Error(None))]),
            (b"\x1b]9;4;3\x07", vec![Action::Progress(Indeterminate)]),
            (
                b"\x1b]9;4;4;80\x07",
                vec![Action::Progress(Paused(Some(80)))],
            ),
            (b"\x1b]9;4;0;0\x07", vec![Action::Progress(Clear)]),
            (b"\x1b]9;4\x07", vec![Action::Progress(Clear)]),
            // An unknown state is dropped, not shown as a notification.
            (b"\x1b]9;4;7;10\x07", vec![]),
        ];
        for (input, expected) in cases {
            assert_eq!(parse(input), expected, "{input:?}");
        }
        // A notification that merely starts with 4 is still one.
        assert_eq!(
            parse(b"\x1b]9;42 tests passed\x07"),
            vec![Action::Notification {
                title: String::new(),
                body: "42 tests passed".to_string(),
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

    #[test]
    fn in_band_apc_is_collected_without_the_marker() {
        let actions = parse(b"a\x1b_UAAEC-_\x1b\\b");
        assert_eq!(
            actions,
            vec![
                Action::Print('a'),
                Action::InBand(b"AAEC-_".to_vec()),
                Action::Print('b')
            ]
        );
    }

    #[test]
    fn other_apc_strings_are_not_in_band() {
        assert!(parse(b"\x1b_Ga=q\x1b\\").is_empty());
        assert!(parse(b"\x1b_\x1b\\").is_empty());
    }

    #[test]
    fn in_band_apc_survives_any_split_through_both_feed_paths() {
        let wire = b"x\x1b_Uabc123def\x1b\\y\x1b_Gskip\x1b\\\x1b_U\x1b\\";
        let expected = vec![
            Action::Print('x'),
            Action::InBand(b"abc123def".to_vec()),
            Action::Print('y'),
            Action::InBand(Vec::new()),
        ];
        for split in 1..wire.len() {
            let mut parser = AnsiParser::new();
            let mut got = Vec::new();
            for chunk in wire.chunks(split) {
                parser.feed(chunk, |a| got.push(a));
            }
            assert_eq!(got, expected, "feed, split {split}");

            let mut parser = AnsiParser::new();
            let mut got = Vec::new();
            for chunk in wire.chunks(split) {
                parser.feed_with_ascii_spans(chunk, |out| match out {
                    crate::ansi::AnsiOutput::Action(a) => got.push(a),
                    crate::ansi::AnsiOutput::PrintAscii(s) => {
                        got.extend(s.iter().map(|&b| Action::Print(b as char)))
                    }
                });
            }
            assert_eq!(got, expected, "spans, split {split}");
        }
    }

    #[test]
    fn oversized_in_band_apc_is_dropped_and_parsing_recovers() {
        let mut wire = b"\x1b_U".to_vec();
        wire.extend(std::iter::repeat_n(b'A', crate::ansi::MAX_IN_BAND_APC + 1));
        wire.extend_from_slice(b"\x1b\\ok\x1b_Uyes\x1b\\");
        let mut parser = AnsiParser::new();
        let mut got = Vec::new();
        parser.feed_with_ascii_spans(&wire, |out| {
            if let crate::ansi::AnsiOutput::Action(a) = out {
                got.push(a);
            }
        });
        assert_eq!(got, vec![Action::InBand(b"yes".to_vec())]);
    }

    #[test]
    fn largest_allowed_in_band_apc_is_kept() {
        let mut wire = b"\x1b_U".to_vec();
        wire.extend(std::iter::repeat_n(b'A', crate::ansi::MAX_IN_BAND_APC));
        wire.extend_from_slice(b"\x1b\\");
        let actions = parse(&wire);
        assert!(
            matches!(&actions[..], [Action::InBand(p)] if p.len() == crate::ansi::MAX_IN_BAND_APC)
        );
    }

    fn anchor(session_id: u64, surface_id: u32, rows: u16) -> Action {
        Action::InlineAnchor {
            session_id,
            surface_id,
            rows,
        }
    }

    #[test]
    fn inline_anchor_is_parsed_where_it_sits() {
        assert_eq!(
            parse(b"a\x1b_u18446744073709551615;4294967295;128\x1b\\b"),
            vec![
                Action::Print('a'),
                anchor(u64::MAX, u32::MAX, 128),
                Action::Print('b')
            ]
        );
        assert_eq!(parse(b"\x1b_u7;0;1\x1b\\"), vec![anchor(7, 0, 1)]);
        assert_eq!(parse(b"\x1b_u007;02;003\x1b\\"), vec![anchor(7, 2, 3)]);
    }

    #[test]
    fn malformed_inline_anchors_are_ignored() {
        for wire in [
            &b"\x1b_u\x1b\\"[..],
            b"\x1b_u1;2\x1b\\",
            b"\x1b_u1;2;3;4\x1b\\",
            b"\x1b_u1;2;0\x1b\\",
            b"\x1b_u1;2;129\x1b\\",
            b"\x1b_u1;2;65536\x1b\\",
            b"\x1b_u18446744073709551616;2;3\x1b\\",
            b"\x1b_u1;4294967296;3\x1b\\",
            b"\x1b_u-1;2;3\x1b\\",
            b"\x1b_u+1;2;3\x1b\\",
            b"\x1b_u1;;3\x1b\\",
            b"\x1b_u1;2;3 \x1b\\",
            b"\x1b_u 1;2;3\x1b\\",
            b"\x1b_u1;2;\xff\x1b\\",
            b"\x1b_u1;2\x1bx;3\x1b\\",
        ] {
            assert_eq!(parse(wire), vec![], "{:?}", String::from_utf8_lossy(wire));
        }
    }

    #[test]
    fn oversized_inline_anchor_is_dropped_and_parsing_recovers() {
        let mut wire = b"\x1b_u1;2;3".to_vec();
        wire.extend(std::iter::repeat_n(b'0', crate::ansi::MAX_ANCHOR_APC));
        wire.extend_from_slice(b"\x1b\\x\x1b_u1;2;3\x1b\\");
        assert_eq!(parse(&wire), vec![Action::Print('x'), anchor(1, 2, 3)]);

        // Exactly the limit is kept: leading zeros pad it out.
        let mut wire = b"\x1b_u".to_vec();
        let tail = b"1;2;3";
        wire.extend(std::iter::repeat_n(
            b'0',
            crate::ansi::MAX_ANCHOR_APC - tail.len(),
        ));
        wire.extend_from_slice(tail);
        wire.extend_from_slice(b"\x1b\\");
        assert_eq!(parse(&wire), vec![anchor(1, 2, 3)]);
    }

    #[test]
    fn inline_anchor_and_in_band_survive_any_split_through_both_feed_paths() {
        let wire = b"x\x1b_u12;34;5\x1b\\y\x1b_Uabc\x1b\\\x1b_u1;2\x1b\\z";
        let expected = vec![
            Action::Print('x'),
            anchor(12, 34, 5),
            Action::Print('y'),
            Action::InBand(b"abc".to_vec()),
            Action::Print('z'),
        ];
        for split in 1..wire.len() {
            let mut parser = AnsiParser::new();
            let mut got = Vec::new();
            for chunk in wire.chunks(split) {
                parser.feed(chunk, |a| got.push(a));
            }
            assert_eq!(got, expected, "feed, split {split}");

            let mut parser = AnsiParser::new();
            let mut got = Vec::new();
            for chunk in wire.chunks(split) {
                parser.feed_with_ascii_spans(chunk, |out| match out {
                    crate::ansi::AnsiOutput::Action(a) => got.push(a),
                    crate::ansi::AnsiOutput::PrintAscii(s) => {
                        got.extend(s.iter().map(|&b| Action::Print(b as char)))
                    }
                });
            }
            assert_eq!(got, expected, "spans, split {split}");
        }
    }

    #[test]
    fn osc_clipboard_read_request_is_refused() {
        // Must not decode "?" to an empty write that wipes the clipboard.
        assert_eq!(parse(b"\x1b]52;c;?\x07"), vec![]);
        assert_eq!(parse(b"\x1b]52;p;?\x1b\\"), vec![]);
    }
}
