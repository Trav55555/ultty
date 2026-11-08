use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use polling::{Event, Events, Poller};
use tracing::{debug, error, info, warn};

use ultty_protocol::control::{
    ControlData, ControlEnvelope, ControlRequest, ControlResponse, OutputFormat,
};

use super::paths::{control_dir, ControlEndpoint};

const LISTENER_KEY: usize = 0;
const CLIENT_KEY_BASE: usize = 100;

/// Largest request the control socket accepts.
///
/// A request ends at its first newline or when the client half-closes its
/// write side, whichever comes first. One bigger than this gets an error
/// reply instead of growing the buffer.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

const READ_CHUNK: usize = 64 * 1024;

pub struct ControlServer {
    listener: UnixListener,
    socket_path: PathBuf,
    token_path: PathBuf,
    poller: Poller,
    clients: HashMap<usize, ClientState>,
    next_key: usize,
    pending_requests: Vec<(usize, ControlRequest)>,
}

struct ClientState {
    stream: UnixStream,
    output_format: OutputFormat,
    authenticated: bool,
    /// Request bytes received so far.
    request: Vec<u8>,
    /// The request outgrew [`MAX_REQUEST_BYTES`]; the rest of it is being
    /// read and dropped.
    oversized: bool,
    /// The client has sent its one request and is waiting for the answer.
    ///
    /// It is no longer polled for reading: ulttyctl half-closes its write
    /// side after the request, and that EOF must not count as a disconnect,
    /// or a deferred answer (`wait-for`) would find the client gone and close
    /// the socket unanswered.
    awaiting_response: bool,
    /// Reply bytes the socket has not taken yet. While non-empty the client
    /// is polled for writability; it is closed once the reply is out.
    reply: Vec<u8>,
}

impl ClientState {
    /// Write as much of the reply as the socket takes now.
    ///
    /// Returns `Ok(true)` once the whole reply is out.
    fn flush_reply(&mut self) -> io::Result<bool> {
        let mut written = 0;
        let result = loop {
            if written == self.reply.len() {
                break Ok(true);
            }
            match self.stream.write(&self.reply[written..]) {
                Ok(0) => break Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => break Err(e),
            }
        };
        self.reply.drain(..written);
        result
    }
}

/// What reading a client that is still sending its request produced.
enum ReadOutcome {
    /// More bytes may follow.
    Incomplete,
    /// The request is complete.
    Request(Vec<u8>),
    /// The request outgrew [`MAX_REQUEST_BYTES`].
    TooLarge,
    /// The client went away without sending anything.
    Gone,
}

struct ParsedControlRequest {
    request: ControlRequest,
    output_format: OutputFormat,
}

impl ControlServer {
    /// Listen on this process's own socket in the private control
    /// directory (see [`super::paths`]).
    pub fn new() -> io::Result<Self> {
        Self::new_at(ControlEndpoint::for_pid(
            &control_dir()?,
            std::process::id(),
        ))
    }

    fn new_at(endpoint: ControlEndpoint) -> io::Result<Self> {
        let ControlEndpoint {
            socket: socket_path,
            token: token_path,
        } = endpoint;
        if socket_path.exists() {
            remove_stale_socket(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        // Owner-only, like the native protocol sockets; the directory is
        // private as well.
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
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
            token_path,
            poller,
            clients: HashMap::new(),
            next_key: CLIENT_KEY_BASE,
            pending_requests: Vec::new(),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Where this server's auth token lives. The app writes it at start;
    /// the server removes it on shutdown.
    pub fn token_path(&self) -> &Path {
        &self.token_path
    }

    /// File descriptors that should wake the UI loop when readable.
    pub fn readiness_fds(&self) -> Vec<RawFd> {
        let mut fds = Vec::with_capacity(self.clients.len() + 1);
        fds.push(self.listener.as_raw_fd());
        // A client awaiting its answer stays readable (its EOF) and would
        // wake the loop on every pass.
        fds.extend(
            self.clients
                .values()
                .filter(|client| !client.awaiting_response)
                .map(|client| client.stream.as_raw_fd()),
        );
        fds
    }

    /// File descriptors that should wake the UI loop when writable: clients
    /// whose reply did not fit in the socket buffer.
    pub fn writable_fds(&self) -> Vec<RawFd> {
        self.clients
            .values()
            .filter(|client| !client.reply.is_empty())
            .map(|client| client.stream.as_raw_fd())
            .collect()
    }

    /// Whether the client `key` has reply bytes still waiting to be written.
    fn is_sending_reply(&self, key: usize) -> bool {
        self.clients
            .get(&key)
            .is_some_and(|client| !client.reply.is_empty())
    }

    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let mut events = Events::new();
        self.poller.wait(&mut events, timeout)?;

        for event in events.iter() {
            if event.key == LISTENER_KEY {
                self.accept_connections()?;
            } else if self.is_sending_reply(event.key) {
                self.continue_reply(event.key)?;
            } else {
                // A client is either sending its request or sending its
                // reply, never both. Dispatch on that, not on the reported
                // readiness: a peer that writes and closes at once shows up
                // as readable *and* writable, and its request must be read.
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
                            request: Vec::new(),
                            oversized: false,
                            awaiting_response: false,
                            reply: Vec::new(),
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
        let Some(client) = self.clients.get_mut(&key) else {
            return Ok(());
        };
        if client.awaiting_response {
            return Ok(());
        }

        let data = match read_request(client) {
            Ok(ReadOutcome::Incomplete) => {
                self.poller.modify(&client.stream, Event::readable(key))?;
                return Ok(());
            }
            Ok(ReadOutcome::Gone) => {
                debug!("Control client disconnected: {}", key);
                self.remove_client(key);
                return Ok(());
            }
            Err(e) => {
                error!("Control client error: {}", e);
                self.remove_client(key);
                return Ok(());
            }
            Ok(ReadOutcome::TooLarge) => None,
            Ok(ReadOutcome::Request(data)) => Some(data),
        };

        client.awaiting_response = true;
        if let Err(e) = self.poller.delete(&client.stream) {
            warn!("Failed to stop polling control client {}: {}", key, e);
        }

        let Some(data) = data else {
            warn!("Control client {} sent an oversized request", key);
            let response = ControlResponse::Error {
                code: 413,
                message: format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
            };
            self.send_response(key, &response);
            return Ok(());
        };

        match parse_request(&data) {
            Some(parsed) => {
                client.output_format = parsed.output_format;
                self.pending_requests.push((key, parsed.request));
            }
            None => {
                // A client that spoke JSON reads a JSON reply.
                if data.trim_ascii_start().starts_with(b"{") {
                    client.output_format = OutputFormat::Json;
                }
                let response = ControlResponse::Error {
                    code: 400,
                    message: "malformed or unknown request".to_string(),
                };
                self.send_response(key, &response);
            }
        }
        Ok(())
    }

    /// Write more of a reply that filled the socket buffer earlier.
    fn continue_reply(&mut self, key: usize) -> io::Result<()> {
        let Some(client) = self.clients.get_mut(&key) else {
            return Ok(());
        };
        match client.flush_reply() {
            Ok(true) => self.remove_client(key),
            Ok(false) => self.poller.modify(&client.stream, Event::writable(key))?,
            Err(e) => {
                warn!("Failed to send response to client {}: {}", key, e);
                self.remove_client(key);
            }
        }
        Ok(())
    }

    fn remove_client(&mut self, key: usize) {
        if let Some(client) = self.clients.remove(&key) {
            // A client awaiting its answer with nothing left to write is
            // already out of the poller; that delete fails harmlessly.
            let _ = self.poller.delete(&client.stream);
        }
    }

    pub fn take_pending_requests(&mut self) -> Vec<(usize, ControlRequest)> {
        std::mem::take(&mut self.pending_requests)
    }

    /// Answer a client's request and close the connection once the whole
    /// answer is written. A reply bigger than the socket buffer is finished
    /// from [`ControlServer::poll`] as the client reads it.
    pub fn send_response(&mut self, client_key: usize, response: &ControlResponse) {
        let Some(client) = self.clients.get_mut(&client_key) else {
            return;
        };
        if !client.reply.is_empty() {
            warn!("Control client {} already has a reply", client_key);
            return;
        }
        client
            .reply
            .extend_from_slice(format_response(response, client.output_format).as_bytes());
        match client.flush_reply() {
            Ok(true) => self.remove_client(client_key),
            Ok(false) => {
                // SAFETY: the stream fd stays valid while the client is in
                // self.clients, and remove_client deletes it from the poller.
                let added = unsafe { self.poller.add(&client.stream, Event::writable(client_key)) };
                if let Err(e) = added {
                    warn!("Failed to poll control client {}: {}", client_key, e);
                    self.remove_client(client_key);
                }
            }
            Err(e) => {
                warn!("Failed to send response to client {}: {}", client_key, e);
                self.remove_client(client_key);
            }
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

/// Read what the client has sent so far and decide whether its request is
/// complete: it ends at the first newline, or at EOF when the client
/// half-closes.
///
/// An oversized request is read to its end and dropped, so the client is
/// done writing and reads the error reply instead of having its write fail.
fn read_request(client: &mut ClientState) -> io::Result<ReadOutcome> {
    let mut chunk = vec![0u8; READ_CHUNK];
    loop {
        let n = match client.stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(ReadOutcome::Incomplete),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Ok(if client.oversized {
                ReadOutcome::TooLarge
            } else if client.request.is_empty() {
                ReadOutcome::Gone
            } else {
                ReadOutcome::Request(std::mem::take(&mut client.request))
            });
        }
        let received = &chunk[..n];
        let newline = received.iter().position(|&b| b == b'\n');
        if !client.oversized {
            let end = newline.unwrap_or(n);
            client.request.extend_from_slice(&received[..end]);
            if client.request.len() > MAX_REQUEST_BYTES {
                client.oversized = true;
                client.request = Vec::new();
            }
        }
        if newline.is_some() {
            return Ok(if client.oversized {
                ReadOutcome::TooLarge
            } else {
                ReadOutcome::Request(std::mem::take(&mut client.request))
            });
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.token_path);
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
        "panes" => Some(ControlRequest::ListPanes),
        "text" => {
            let include_scrollback = parts.next() == Some("--scrollback");
            Some(ControlRequest::GetText {
                include_scrollback,
                scrollback_lines: None,
                target: None,
            })
        }
        "selection" => Some(ControlRequest::GetSelection { target: None }),
        "send-text" => {
            let text: String = parts.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                None
            } else {
                Some(ControlRequest::SendText { text, target: None })
            }
        }
        "send-key" => {
            let key_str = parts.next()?;
            let key = parse_key(key_str)?;
            let modifiers = parse_modifiers(parts.next().unwrap_or(""));
            Some(ControlRequest::SendKey {
                key,
                modifiers,
                target: None,
            })
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
            Some(ControlRequest::SendRaw { data, target: None })
        }
        "run" => {
            let command = parts.next()?.to_string();
            let args: Vec<String> = parts.map(String::from).collect();
            Some(ControlRequest::RunCommand {
                command,
                args,
                target: None,
            })
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
                    target: None,
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
                    target: None,
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

/// Render `response` as the control socket sends it in `format`.
pub fn format_response(response: &ControlResponse, format: OutputFormat) -> String {
    if format == OutputFormat::Json {
        return serde_json::to_string(response)
            .unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
            + "\n";
    }

    match response {
        ControlResponse::Ok => "OK\n".to_string(),
        ControlResponse::Text { text } => format!("{}\n", text),
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
            ControlData::Tabs { tabs } => {
                let mut out = String::new();
                for tab in tabs {
                    let marker = if tab.active { "*" } else { " " };
                    out.push_str(&format!("{}{}: {}\n", marker, tab.index + 1, tab.title));
                }
                out
            }
            ControlData::Panes { panes } => {
                let mut out = String::new();
                for pane in panes {
                    let marker = if pane.focused { "*" } else { " " };
                    let label = [pane.foreground.as_deref(), Some(pane.title.as_str())]
                        .into_iter()
                        .chain([pane.cwd.as_deref()])
                        .flatten()
                        .find(|label| !label.is_empty())
                        .unwrap_or("");
                    out.push_str(&format!(
                        "{}{} tab:{} {}x{} {}\n",
                        marker,
                        pane.id,
                        pane.tab_index + 1,
                        pane.cols,
                        pane.rows,
                        label
                    ));
                }
                out
            }
            ControlData::Selection { text } => match text {
                Some(text) => format!("{}\n", text),
                None => "(no selection)\n".to_string(),
            },
            ControlData::ScreenshotSaved { path } => format!("Screenshot saved: {}\n", path),
            ControlData::ScreenshotData { data } => {
                format!("Screenshot data: {} bytes\n", data.len())
            }
            ControlData::SearchResults { matches } => {
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
            ControlData::BatchResults { results } => {
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

    fn test_endpoint(name: &str) -> ControlEndpoint {
        let socket = test_socket_path(name);
        ControlEndpoint {
            token: socket.with_extension("token"),
            socket,
        }
    }

    fn test_server(name: &str) -> ControlServer {
        ControlServer::new_at(test_endpoint(name)).unwrap()
    }

    #[test]
    fn socket_is_owner_only() {
        let server = test_server("mode");
        let mode = fs::metadata(server.socket_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn two_instances_listen_side_by_side_and_clean_up_only_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let first = ControlServer::new_at(ControlEndpoint::for_pid(dir.path(), 1)).unwrap();
        let second = ControlServer::new_at(ControlEndpoint::for_pid(dir.path(), 2)).unwrap();
        fs::write(first.token_path(), "a").unwrap();
        fs::write(second.token_path(), "b").unwrap();

        let (second_socket, second_token) = (
            second.socket_path().to_path_buf(),
            second.token_path().to_path_buf(),
        );
        drop(second);
        assert!(!second_socket.exists() && !second_token.exists());

        assert!(UnixStream::connect(first.socket_path()).is_ok());
        assert!(first.token_path().exists());
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
        assert_eq!(
            parse_request_only(b"panes"),
            Some(ControlRequest::ListPanes)
        );
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
                text: "hello world".to_string(),
                target: None,
            })
        );
    }

    #[test]
    fn parse_send_raw() {
        assert_eq!(
            parse_request_only(b"send-raw 1b5b41"),
            Some(ControlRequest::SendRaw {
                data: vec![0x1b, 0x5b, 0x41],
                target: None,
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
                    text: "hello world".to_string(),
                    target: None,
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
                target: None,
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
                target: None,
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
    fn format_panes_text_marks_focus_and_prefers_the_foreground_process() {
        use ultty_protocol::control::PaneInfo;
        let pane = |id, focused, title: &str, foreground: Option<&str>| PaneInfo {
            id,
            tab_index: 0,
            active: focused,
            focused,
            title: title.to_string(),
            foreground: foreground.map(str::to_string),
            pid: None,
            cwd: None,
            cols: 40,
            rows: 24,
        };
        let response = ControlResponse::Data(ControlData::Panes {
            panes: vec![
                pane(3, true, "~/src", Some("vim")),
                pane(5, false, "build log", None),
                PaneInfo {
                    cwd: Some("/srv".to_string()),
                    ..pane(8, false, "", None)
                },
            ],
        });
        assert_eq!(
            format_response(&response, OutputFormat::Text),
            "*3 tab:1 40x24 vim\n 5 tab:1 40x24 build log\n 8 tab:1 40x24 /srv\n"
        );
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
    fn deferred_answer_survives_client_half_close() {
        let mut server = test_server("half-close");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();

        client.write_all(b"wait-for DONE").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let pending = poll_until_pending(&mut server);
        let (client_key, _) = pending[0].clone();

        // Polling past the client's EOF, as the UI loop does while a
        // wait-for is pending, must keep the client.
        for _ in 0..5 {
            server.poll(Some(Duration::from_millis(10))).unwrap();
        }
        assert!(server.clients.contains_key(&client_key));
        assert_eq!(server.readiness_fds(), vec![server.listener.as_raw_fd()]);

        server.send_response(
            client_key,
            &ControlResponse::Data(ControlData::WaitMatched { line: 0, col: 0 }),
        );

        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert_eq!(response, "MATCHED at 1:1\n");
        assert!(server.clients.is_empty());
    }

    #[test]
    fn a_client_that_closes_right_after_writing_is_still_heard() {
        // Fire-and-forget scripts (`nc -q0`, a send and close) hang up as
        // soon as the request is written. The hang-up makes the socket
        // report writable too; the request must still be read and run.
        let mut server = test_server("write-and-close");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();
        client.write_all(b"send-text hi\n").unwrap();
        drop(client);

        let pending = poll_until_pending(&mut server);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].1,
            ControlRequest::SendText {
                text: "hi".to_string(),
                target: None,
            }
        );
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

    /// Poll the server until `client` finishes reading in its own thread.
    fn read_reply_while_polling(server: &mut ControlServer, mut client: UnixStream) -> String {
        let reader = std::thread::spawn(move || {
            let mut reply = String::new();
            client.read_to_string(&mut reply).map(|_| reply)
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reader.is_finished() {
            assert!(Instant::now() < deadline, "timed out waiting for reply");
            server.poll(Some(Duration::from_millis(10))).unwrap();
        }
        reader.join().unwrap().unwrap()
    }

    fn json_request(request: ControlRequest) -> Vec<u8> {
        serde_json::to_vec(&ControlEnvelope {
            output_format: OutputFormat::Json,
            request,
        })
        .unwrap()
    }

    #[test]
    fn request_larger_than_one_read_is_answered() {
        let mut server = test_server("large-request");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();
        let text = "x".repeat(5000);

        client
            .write_all(&json_request(ControlRequest::SendText {
                text: text.clone(),
                target: None,
            }))
            .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let pending = poll_until_pending(&mut server);
        assert_eq!(
            pending[0].1,
            ControlRequest::SendText { text, target: None }
        );
        server.send_response(pending[0].0, &ControlResponse::Ok);
        assert_eq!(
            read_reply_while_polling(&mut server, client),
            format_response(&ControlResponse::Ok, OutputFormat::Json)
        );
    }

    #[test]
    fn request_split_across_writes_ends_at_newline() {
        let mut server = test_server("split-request");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();

        client.write_all(b"send-text hel").unwrap();
        for _ in 0..3 {
            server.poll(Some(Duration::from_millis(10))).unwrap();
        }
        assert!(server.take_pending_requests().is_empty());

        // No half-close: the newline ends the request.
        client.write_all(b"lo\n").unwrap();
        let pending = poll_until_pending(&mut server);
        assert_eq!(
            pending[0].1,
            ControlRequest::SendText {
                text: "hello".to_string(),
                target: None,
            }
        );
    }

    #[test]
    fn reply_larger_than_socket_buffer_arrives_intact() {
        let mut server = test_server("large-reply");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();
        client.write_all(b"text --scrollback").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let pending = poll_until_pending(&mut server);

        let text: String = (0..9000)
            .map(|i| format!("line {i:05} of scrollback\n"))
            .collect();
        assert!(text.len() > 200 * 1024);
        server.send_response(pending[0].0, &ControlResponse::Text { text: text.clone() });

        assert_eq!(
            read_reply_while_polling(&mut server, client),
            format!("{text}\n")
        );
        assert!(server.clients.is_empty());
    }

    #[test]
    fn malformed_json_request_gets_json_error() {
        let mut server = test_server("malformed");
        let client = UnixStream::connect(server.socket_path()).unwrap();
        // What `ulttyctl opacity NaN` sends: NaN serialises as null.
        (&client)
            .write_all(
                br#"{"output_format":"json","request":{"type":"set_opacity","opacity":null}}"#,
            )
            .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let reply = read_reply_while_polling(&mut server, client);
        let response: ControlResponse = serde_json::from_str(reply.trim()).unwrap();
        assert!(
            matches!(response, ControlResponse::Error { code: 400, .. }),
            "{response:?}"
        );
        assert!(server.take_pending_requests().is_empty());
    }

    #[test]
    fn unknown_text_request_gets_text_error() {
        let mut server = test_server("unknown-text");
        let mut client = UnixStream::connect(server.socket_path()).unwrap();
        client.write_all(b"frobnicate\n").unwrap();

        let reply = read_reply_while_polling(&mut server, client);
        assert!(reply.starts_with("ERROR 400:"), "{reply}");
    }

    #[test]
    fn oversized_request_gets_error_instead_of_growing() {
        let mut server = test_server("oversized");
        let client = UnixStream::connect(server.socket_path()).unwrap();
        let mut writer = client.try_clone().unwrap();
        let sender = std::thread::spawn(move || {
            // The server drains the whole request, so the write completes.
            writer
                .write_all(&vec![b'a'; MAX_REQUEST_BYTES + 2 * READ_CHUNK])
                .unwrap();
            writer.shutdown(std::net::Shutdown::Write).unwrap();
        });

        let reply = read_reply_while_polling(&mut server, client);
        sender.join().unwrap();
        assert!(reply.starts_with("ERROR 413:"), "{reply}");
        assert!(server.take_pending_requests().is_empty());
    }
}
