use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use polling::{Event, Events, Poller};
use tracing::{debug, error, info, warn};

use ultty_protocol::control::{
    control_socket_path, ControlData, ControlEnvelope, ControlRequest, ControlResponse,
    OutputFormat,
};

const LISTENER_KEY: usize = 0;
const CLIENT_KEY_BASE: usize = 100;

pub struct ControlServer {
    listener: UnixListener,
    socket_path: PathBuf,
    poller: Poller,
    clients: HashMap<usize, ClientState>,
    next_key: usize,
    pending_requests: Vec<(usize, ControlRequest)>,
}

struct ClientState {
    stream: UnixStream,
    output_format: OutputFormat,
    authenticated: bool,
}

struct ParsedControlRequest {
    request: ControlRequest,
    output_format: OutputFormat,
}

impl ControlServer {
    pub fn new() -> io::Result<Self> {
        Self::new_at(control_socket_path())
    }

    fn new_at(socket_path: PathBuf) -> io::Result<Self> {
        if socket_path.exists() {
            remove_stale_socket(&socket_path)?;
        }

        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let poller = Poller::new()?;

        // SAFETY: listener fd is valid and owned by this ControlServer for its lifetime.
        // The poller uses it only for readability notifications.
        unsafe {
            poller.add(&listener, Event::readable(LISTENER_KEY))?;
        }

        info!("Control server listening on {:?}", socket_path);

        Ok(Self {
            listener,
            socket_path,
            poller,
            clients: HashMap::new(),
            next_key: CLIENT_KEY_BASE,
            pending_requests: Vec::new(),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// File descriptors that should wake the UI loop when readable.
    pub fn readiness_fds(&self) -> Vec<RawFd> {
        let mut fds = Vec::with_capacity(self.clients.len() + 1);
        fds.push(self.listener.as_raw_fd());
        fds.extend(
            self.clients
                .values()
                .map(|client| client.stream.as_raw_fd()),
        );
        fds
    }

    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let mut events = Events::new();
        self.poller.wait(&mut events, timeout)?;

        for event in events.iter() {
            if event.key == LISTENER_KEY {
                self.accept_connections()?;
            } else {
                self.handle_client_event(event.key)?;
            }
        }

        Ok(())
    }

    fn accept_connections(&mut self) -> io::Result<()> {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    stream.set_nonblocking(true)?;
                    let key = self.next_key;
                    self.next_key += 1;

                    // SAFETY: stream fd is valid and stored in self.clients for its lifetime.
                    unsafe {
                        self.poller.add(&stream, Event::readable(key))?;
                    }

                    self.clients.insert(
                        key,
                        ClientState {
                            stream,
                            output_format: OutputFormat::Text,
                            authenticated: false,
                        },
                    );
                    debug!("Control client connected: {}", key);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        self.poller
            .modify(&self.listener, Event::readable(LISTENER_KEY))?;

        Ok(())
    }

    fn handle_client_event(&mut self, key: usize) -> io::Result<()> {
        let mut should_remove = false;
        let mut request_data: Option<Vec<u8>> = None;
        let mut need_modify = false;

        if let Some(client) = self.clients.get_mut(&key) {
            let mut buf = [0u8; 4096];
            match client.stream.read(&mut buf) {
                Ok(0) => {
                    debug!("Control client disconnected: {}", key);
                    should_remove = true;
                }
                Ok(n) => {
                    request_data = Some(buf[..n].to_vec());
                    need_modify = true;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    need_modify = true;
                }
                Err(e) => {
                    error!("Control client error: {}", e);
                    should_remove = true;
                }
            }
        }

        if let Some(data) = request_data {
            if let Some(parsed) = self.parse_request(&data) {
                if let Some(client) = self.clients.get_mut(&key) {
                    client.output_format = parsed.output_format;
                }
                self.pending_requests.push((key, parsed.request));
            }
        }

        if need_modify {
            if let Some(client) = self.clients.get(&key) {
                self.poller.modify(&client.stream, Event::readable(key))?;
            }
        }

        if should_remove {
            if let Some(client) = self.clients.remove(&key) {
                let _ = self.poller.delete(&client.stream);
            }
        }

        Ok(())
    }

    fn parse_request(&self, data: &[u8]) -> Option<ParsedControlRequest> {
        parse_request(data)
    }

    pub fn take_pending_requests(&mut self) -> Vec<(usize, ControlRequest)> {
        std::mem::take(&mut self.pending_requests)
    }

    pub fn send_response(&mut self, client_key: usize, response: &ControlResponse) {
        if let Some(mut client) = self.clients.remove(&client_key) {
            let text = format_response(response, client.output_format);
            if let Err(e) = client.stream.write_all(text.as_bytes()) {
                warn!("Failed to send response to client {}: {}", client_key, e);
            }
            let _ = self.poller.delete(&client.stream);
        }
    }

    pub fn set_client_format(&mut self, client_key: usize, format: OutputFormat) {
        if let Some(client) = self.clients.get_mut(&client_key) {
            client.output_format = format;
        }
    }

    pub fn set_client_authenticated(&mut self, client_key: usize, authenticated: bool) {
        if let Some(client) = self.clients.get_mut(&client_key) {
            client.authenticated = authenticated;
        }
    }

    pub fn is_client_authenticated(&self, client_key: usize) -> bool {
        self.clients
            .get(&client_key)
            .map(|c| c.authenticated)
            .unwrap_or(false)
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        info!("Control server shut down");
    }
}

fn remove_stale_socket(socket_path: &Path) -> io::Result<()> {
    match UnixStream::connect(socket_path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "socket already has an active listener: {}",
                socket_path.display()
            ),
        )),
        Err(_) => fs::remove_file(socket_path),
    }
}

/// Parse one client request.
///
/// A request starting with `{` is JSON: either a [`ControlEnvelope`] or a
/// bare [`ControlRequest`]. Anything else is a whitespace-separated text
/// command such as `send-text hello`. A text command may carry a leading
/// `token <hex> ` prefix (for example `token abc123 send-text hello`), which
/// authenticates the connection and parses into
/// [`ControlRequest::WithToken`]; a malformed prefix rejects the request.
fn parse_request(data: &[u8]) -> Option<ParsedControlRequest> {
    let text = std::str::from_utf8(data).ok()?.trim();

    if text.starts_with('{') {
        if let Ok(envelope) = serde_json::from_str::<ControlEnvelope>(text) {
            return Some(ParsedControlRequest {
                request: envelope.request,
                output_format: envelope.output_format,
            });
        }

        return serde_json::from_str::<ControlRequest>(text)
            .ok()
            .map(|request| ParsedControlRequest {
                request,
                output_format: OutputFormat::Text,
            });
    }

    let request = parse_text_request(text)?;

    Some(ParsedControlRequest {
        request,
        output_format: OutputFormat::Text,
    })
}

/// Parse a text-mode command line, honouring an optional `token <hex>` prefix.
fn parse_text_request(text: &str) -> Option<ControlRequest> {
    let (first, after_first) = split_word(text);
    if first != "token" {
        return parse_text_command(text);
    }

    let (token, command) = split_word(after_first);
    let token_is_hex = !token.is_empty() && token.bytes().all(|b| b.is_ascii_hexdigit());
    if !token_is_hex || command.is_empty() {
        return None;
    }

    // `parse_text_command` has no `token` arm, so the prefix cannot nest.
    let request = parse_text_command(command)?;
    Some(ControlRequest::WithToken {
        token: token.to_string(),
        request: Box::new(request),
    })
}

/// Split off the first whitespace-delimited word; both halves are trimmed
/// of leading whitespace.
fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    (&text[..end], text[end..].trim_start())
}

/// Parse a text-mode command without any authentication prefix.
fn parse_text_command(text: &str) -> Option<ControlRequest> {
    let mut parts = text.split_whitespace();
    let cmd = parts.next()?;

    match cmd {
        "ping" => Some(ControlRequest::Ping),
        "version" => Some(ControlRequest::Version),
        "info" => Some(ControlRequest::GetInfo),
        "tabs" => Some(ControlRequest::ListTabs),
        "text" => {
            let include_scrollback = parts.next() == Some("--scrollback");
            Some(ControlRequest::GetText {
                include_scrollback,
                scrollback_lines: None,
            })
        }
        "selection" => Some(ControlRequest::GetSelection),
        "send-text" => {
            let text: String = parts.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                None
            } else {
                Some(ControlRequest::SendText { text })
            }
        }
        "send-key" => {
            let key_str = parts.next()?;
            let key = parse_key(key_str)?;
            let modifiers = parse_modifiers(parts.next().unwrap_or(""));
            Some(ControlRequest::SendKey { key, modifiers })
        }
        "new-tab" => {
            let command = parts.next().map(String::from);
            let args: Vec<String> = parts.map(String::from).collect();
            Some(ControlRequest::NewTab {
                title: None,
                command,
                args,
                working_dir: None,
            })
        }
        "switch-tab" => {
            let index = parse_display_tab_index(parts.next()?)?;
            Some(ControlRequest::SwitchTab { index })
        }
        "close-tab" => {
            let index = parts.next().and_then(parse_display_tab_index);
            Some(ControlRequest::CloseTab { index })
        }
        "focus" => Some(ControlRequest::FocusWindow),
        "fullscreen" => Some(ControlRequest::ToggleFullscreen),
        "opacity" => {
            let opacity = parts.next()?.parse().ok()?;
            Some(ControlRequest::SetOpacity { opacity })
        }
        "send-raw" => {
            let hex_str: String = parts.collect::<Vec<_>>().join("");
            let data = hex_decode(&hex_str)?;
            Some(ControlRequest::SendRaw { data })
        }
        "run" => {
            let command = parts.next()?.to_string();
            let args: Vec<String> = parts.map(String::from).collect();
            Some(ControlRequest::RunCommand { command, args })
        }
        "resize" => {
            let width: u32 = parts.next()?.parse().ok()?;
            let height: u32 = parts.next()?.parse().ok()?;
            Some(ControlRequest::Resize { width, height })
        }
        "screenshot" => {
            let path = parts.next().map(String::from);
            Some(ControlRequest::Screenshot { path })
        }
        "search" => {
            let pattern: String = parts.collect::<Vec<_>>().join(" ");
            if pattern.is_empty() {
                None
            } else {
                Some(ControlRequest::Search {
                    pattern,
                    regex: false,
                    case_sensitive: false,
                    include_scrollback: true,
                })
            }
        }
        "wait-for" => {
            let rest: Vec<&str> = parts.collect();
            let mut timeout_ms = 30000u64;
            let mut pattern_parts = Vec::new();

            for (i, part) in rest.iter().enumerate() {
                if *part == "--timeout" {
                    if let Some(t) = rest.get(i + 1) {
                        timeout_ms = t.parse().unwrap_or(30000);
                    }
                    break;
                }
                pattern_parts.push(*part);
            }

            let pattern = pattern_parts.join(" ");
            if pattern.is_empty() {
                None
            } else {
                Some(ControlRequest::WaitFor {
                    pattern,
                    regex: false,
                    timeout_ms,
                })
            }
        }
        "auth" => {
            let token = parts.next()?.to_string();
            Some(ControlRequest::Authenticate { token })
        }
        _ => None,
    }
}

#[cfg(test)]
fn parse_request_only(data: &[u8]) -> Option<ControlRequest> {
    parse_request(data).map(|parsed| parsed.request)
}

fn parse_key(s: &str) -> Option<ultty_protocol::Key> {
    match s.to_lowercase().as_str() {
        "enter" | "return" => Some(ultty_protocol::Key::Enter),
        "tab" => Some(ultty_protocol::Key::Tab),
        "escape" | "esc" => Some(ultty_protocol::Key::Escape),
        "backspace" => Some(ultty_protocol::Key::Backspace),
        "delete" | "del" => Some(ultty_protocol::Key::Delete),
        "insert" | "ins" => Some(ultty_protocol::Key::Insert),
        "home" => Some(ultty_protocol::Key::Home),
        "end" => Some(ultty_protocol::Key::End),
        "pageup" | "pgup" => Some(ultty_protocol::Key::PageUp),
        "pagedown" | "pgdn" => Some(ultty_protocol::Key::PageDown),
        "up" => Some(ultty_protocol::Key::Up),
        "down" => Some(ultty_protocol::Key::Down),
        "left" => Some(ultty_protocol::Key::Left),
        "right" => Some(ultty_protocol::Key::Right),
        "space" => Some(ultty_protocol::Key::Char(' ')),
        s if s.starts_with('f') && s.len() <= 3 => {
            let n: u8 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                Some(ultty_protocol::Key::Function(n))
            } else {
                None
            }
        }
        s if s.len() == 1 => Some(ultty_protocol::Key::Char(s.chars().next()?)),
        _ => None,
    }
}

fn parse_display_tab_index(s: &str) -> Option<usize> {
    s.parse::<usize>().ok()?.checked_sub(1)
}

fn parse_modifiers(s: &str) -> ultty_protocol::Modifiers {
    let mut mods = ultty_protocol::Modifiers::NONE;
    let lower = s.to_lowercase();
    if lower.contains("shift") {
        mods = mods.union(ultty_protocol::Modifiers::SHIFT);
    }
    if lower.contains("ctrl") || lower.contains("control") {
        mods = mods.union(ultty_protocol::Modifiers::CTRL);
    }
    if lower.contains("alt") {
        mods = mods.union(ultty_protocol::Modifiers::ALT);
    }
    if lower.contains("meta") || lower.contains("super") || lower.contains("cmd") {
        mods = mods.union(ultty_protocol::Modifiers::META);
    }
    mods
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() & 1 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn format_response(response: &ControlResponse, format: OutputFormat) -> String {
    if format == OutputFormat::Json {
        return serde_json::to_string(response)
            .unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
            + "\n";
    }

    match response {
        ControlResponse::Ok => "OK\n".to_string(),
        ControlResponse::Text(text) => format!("{}\n", text),
        ControlResponse::Data(data) => match data {
            ControlData::Pong => "PONG\n".to_string(),
            ControlData::Version(v) => {
                format!("ultty {} (protocol v{})\n", v.version, v.protocol_version)
            }
            ControlData::Info(info) => {
                format!(
                    "{}x{} tab:{}/{} cursor:({},{}) focused:{}\n",
                    info.cols,
                    info.rows,
                    info.current_tab + 1,
                    info.tab_count,
                    info.cursor_col,
                    info.cursor_row,
                    info.focused
                )
            }
            ControlData::Tabs(tabs) => {
                let mut out = String::new();
                for tab in tabs {
                    let marker = if tab.active { "*" } else { " " };
                    out.push_str(&format!("{}{}: {}\n", marker, tab.index + 1, tab.title));
                }
                out
            }
            ControlData::Selection(sel) => match sel {
                Some(text) => format!("{}\n", text),
                None => "(no selection)\n".to_string(),
            },
            ControlData::ScreenshotSaved(path) => format!("Screenshot saved: {}\n", path),
            ControlData::ScreenshotData(data) => format!("Screenshot data: {} bytes\n", data.len()),
            ControlData::SearchResults(matches) => {
                if matches.is_empty() {
                    "(no matches)\n".to_string()
                } else {
                    let mut out = format!("{} match(es):\n", matches.len());
                    for m in matches {
                        out.push_str(&format!("  {}:{}: {}\n", m.line + 1, m.col + 1, m.text));
                    }
                    out
                }
            }
            ControlData::BatchResults(results) => {
                let mut out = format!("{} result(s):\n", results.len());
                for (i, r) in results.iter().enumerate() {
                    out.push_str(&format!(
                        "[{}] {}",
                        i + 1,
                        format_response(r, OutputFormat::Text)
                    ));
                }
                out
            }
            ControlData::Authenticated => "AUTHENTICATED\n".to_string(),
            ControlData::WaitMatched { line, col } => {
                format!("MATCHED at {}:{}\n", line + 1, col + 1)
            }
            ControlData::WaitTimeout => "TIMEOUT\n".to_string(),
        },
        ControlResponse::Error { code, message } => {
            format!("ERROR {}: {}\n", code, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    fn test_socket_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ultty-control-server-{}-{}-{}.sock",
            name,
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ))
    }

    fn test_server(name: &str) -> ControlServer {
        ControlServer::new_at(test_socket_path(name)).unwrap()
    }

    #[test]
    fn readiness_fds_include_listener_and_clients() {
        let mut server = test_server("readiness");
        let listener_fd = server.listener.as_raw_fd();
        assert_eq!(server.readiness_fds(), vec![listener_fd]);

        let mut client = UnixStream::connect(server.socket_path()).unwrap();
        writeln!(client, "ping").unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        let fds = server.readiness_fds();
        assert!(fds.contains(&listener_fd));
        assert_eq!(fds.len(), 2);
    }

    fn poll_until_pending(server: &mut ControlServer) -> Vec<(usize, ControlRequest)> {
        let deadline = Instant::now() + Duration::from_secs(1);

        loop {
            server.poll(Some(Duration::from_millis(10))).unwrap();
            let pending = server.take_pending_requests();
            if !pending.is_empty() {
                return pending;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for pending control request"
            );
        }
    }

    #[test]
    fn hex_decode_valid() {
        assert_eq!(hex_decode("1b5b41"), Some(vec![0x1b, 0x5b, 0x41]));
        assert_eq!(hex_decode("00ff"), Some(vec![0x00, 0xff]));
        assert_eq!(hex_decode(""), Some(vec![]));
    }

    #[test]
    fn hex_decode_invalid() {
        assert_eq!(hex_decode("1b5"), None); // odd length
        assert_eq!(hex_decode("zz"), None); // invalid hex
    }

    #[test]
    fn parse_ping() {
        assert_eq!(parse_request_only(b"ping"), Some(ControlRequest::Ping));
    }

    #[test]
    fn parse_version() {
        assert_eq!(
            parse_request_only(b"version"),
            Some(ControlRequest::Version)
        );
    }

    #[test]
    fn parse_send_text() {
        assert_eq!(
            parse_request_only(b"send-text hello world"),
            Some(ControlRequest::SendText {
                text: "hello world".to_string()
            })
        );
    }

    #[test]
    fn parse_send_raw() {
        assert_eq!(
            parse_request_only(b"send-raw 1b5b41"),
            Some(ControlRequest::SendRaw {
                data: vec![0x1b, 0x5b, 0x41]
            })
        );
    }

    #[test]
    fn parse_resize() {
        assert_eq!(
            parse_request_only(b"resize 1920 1080"),
            Some(ControlRequest::Resize {
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(parse_request_only(b"nonexistent-command"), None);
    }

    #[test]
    fn parse_token_prefix_wraps_request_in_with_token() {
        let parsed = parse_request(b"token abc123 send-text hello world").unwrap();

        assert_eq!(
            parsed.request,
            ControlRequest::WithToken {
                token: "abc123".to_string(),
                request: Box::new(ControlRequest::SendText {
                    text: "hello world".to_string()
                }),
            }
        );
        assert_eq!(parsed.output_format, OutputFormat::Text);
    }

    #[test]
    fn parse_token_prefix_tolerates_extra_whitespace() {
        assert_eq!(
            parse_request_only(b"  token   0F  ping \n"),
            Some(ControlRequest::WithToken {
                token: "0F".to_string(),
                request: Box::new(ControlRequest::Ping),
            })
        );
    }

    #[test]
    fn parse_token_prefix_rejects_malformed_prefixes() {
        // No token, no command.
        assert_eq!(parse_request_only(b"token"), None);
        // Token without a command.
        assert_eq!(parse_request_only(b"token abc123"), None);
        // Token is not hex.
        assert_eq!(parse_request_only(b"token xyz ping"), None);
        // Command after the prefix is unknown.
        assert_eq!(parse_request_only(b"token abc123 nonexistent"), None);
        // Prefix cannot nest.
        assert_eq!(parse_request_only(b"token abc123 token abc123 ping"), None);
        // "token" must be its own word.
        assert_eq!(parse_request_only(b"tokenabc123 ping"), None);
    }

    #[test]
    fn parse_wait_for_with_timeout() {
        assert_eq!(
            parse_request_only(b"wait-for $ --timeout 5000"),
            Some(ControlRequest::WaitFor {
                pattern: "$".to_string(),
                regex: false,
                timeout_ms: 5000,
            })
        );
    }

    #[test]
    fn parse_search() {
        assert_eq!(
            parse_request_only(b"search error log"),
            Some(ControlRequest::Search {
                pattern: "error log".to_string(),
                regex: false,
                case_sensitive: false,
                include_scrollback: true,
            })
        );
    }

    #[test]
    fn parse_tab_commands_use_display_indices() {
        assert_eq!(
            parse_request_only(b"switch-tab 2"),
            Some(ControlRequest::SwitchTab { index: 1 })
        );
        assert_eq!(
            parse_request_only(b"close-tab 2"),
            Some(ControlRequest::CloseTab { index: Some(1) })
        );
        assert_eq!(
            parse_request_only(b"close-tab"),
            Some(ControlRequest::CloseTab { index: None })
        );
        assert_eq!(parse_request_only(b"switch-tab 0"), None);
    }

    #[test]
    fn parse_send_key() {
        let req = parse_request_only(b"send-key enter");
        assert!(req.is_some());
        if let Some(ControlRequest::SendKey { key, .. }) = req {
            assert_eq!(key, ultty_protocol::Key::Enter);
        }
    }

    #[test]
    fn parse_json_envelope_sets_output_format() {
        let json = serde_json::to_vec(&ControlEnvelope {
            output_format: OutputFormat::Json,
            request: ControlRequest::Ping,
        })
        .unwrap();

        let parsed = parse_request(&json).unwrap();

        assert_eq!(parsed.request, ControlRequest::Ping);
        assert_eq!(parsed.output_format, OutputFormat::Json);
    }

    #[test]
    fn format_pong_text() {
        let response = ControlResponse::Data(ControlData::Pong);
        assert_eq!(format_response(&response, OutputFormat::Text), "PONG\n");
    }

    #[test]
    fn format_error_text() {
        let response = ControlResponse::Error {
            code: 404,
            message: "not found".to_string(),
        };
        assert_eq!(
            format_response(&response, OutputFormat::Text),
            "ERROR 404: not found\n"
        );
    }

    #[test]
    fn format_json_output() {
        let response = ControlResponse::Data(ControlData::Pong);
        let json = format_response(&response, OutputFormat::Json);
        assert!(json.contains("Pong") || json.contains("pong"));
    }

    #[test]
    fn send_response_removes_one_shot_client() {
        let mut server = test_server("remove-client");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();

        client.write_all(b"ping").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let pending = poll_until_pending(&mut server);
        assert_eq!(pending.len(), 1);
        let (client_key, request) = pending[0].clone();
        assert_eq!(request, ControlRequest::Ping);
        assert!(server.clients.contains_key(&client_key));

        server.send_response(client_key, &ControlResponse::Data(ControlData::Pong));

        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();

        assert_eq!(response, "PONG\n");
        assert!(!server.clients.contains_key(&client_key));
    }

    #[test]
    fn repeated_one_shot_clients_do_not_accumulate() {
        let mut server = test_server("repeated-clients");

        for _ in 0..3 {
            let mut client = UnixStream::connect(server.socket_path()).unwrap();
            client.write_all(b"ping").unwrap();
            client.shutdown(std::net::Shutdown::Write).unwrap();

            let pending = poll_until_pending(&mut server);
            assert_eq!(pending.len(), 1);
            let (client_key, request) = pending[0].clone();
            assert_eq!(request, ControlRequest::Ping);

            server.send_response(client_key, &ControlResponse::Data(ControlData::Pong));

            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert_eq!(response, "PONG\n");
            assert!(server.clients.is_empty());
        }
    }
}
