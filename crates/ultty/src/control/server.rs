use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use polling::{Event, Events, Poller};
use tracing::{debug, error, info, warn};

use ultty_protocol::control::{control_socket_path, ControlData, ControlRequest, ControlResponse};

const LISTENER_KEY: usize = 0;
const CLIENT_KEY_BASE: usize = 100;

pub struct ControlServer {
    listener: UnixListener,
    socket_path: PathBuf,
    poller: Poller,
    clients: HashMap<usize, UnixStream>,
    next_key: usize,
    pending_requests: Vec<(usize, ControlRequest)>,
}

impl ControlServer {
    pub fn new() -> io::Result<Self> {
        let socket_path = control_socket_path();

        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }

        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let poller = Poller::new()?;

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

                    unsafe {
                        self.poller.add(&stream, Event::readable(key))?;
                    }

                    self.clients.insert(key, stream);
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

        if let Some(stream) = self.clients.get_mut(&key) {
            let mut buf = [0u8; 4096];
            match stream.read(&mut buf) {
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
            if let Some(request) = self.parse_request(&data) {
                self.pending_requests.push((key, request));
            }
        }

        if need_modify {
            if let Some(stream) = self.clients.get(&key) {
                self.poller.modify(stream, Event::readable(key))?;
            }
        }

        if should_remove {
            if let Some(stream) = self.clients.remove(&key) {
                let _ = self.poller.delete(&stream);
            }
        }

        Ok(())
    }

    fn parse_request(&self, data: &[u8]) -> Option<ControlRequest> {
        let text = std::str::from_utf8(data).ok()?.trim();
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
                let index = parts.next()?.parse().ok()?;
                Some(ControlRequest::SwitchTab { index })
            }
            "close-tab" => {
                let index = parts.next().and_then(|s| s.parse().ok());
                Some(ControlRequest::CloseTab { index })
            }
            "focus" => Some(ControlRequest::FocusWindow),
            "fullscreen" => Some(ControlRequest::ToggleFullscreen),
            "opacity" => {
                let opacity = parts.next()?.parse().ok()?;
                Some(ControlRequest::SetOpacity { opacity })
            }
            "run" => {
                let command = parts.next()?.to_string();
                let args: Vec<String> = parts.map(String::from).collect();
                Some(ControlRequest::RunCommand { command, args })
            }
            _ => None,
        }
    }

    pub fn take_pending_requests(&mut self) -> Vec<(usize, ControlRequest)> {
        std::mem::take(&mut self.pending_requests)
    }

    pub fn send_response(&mut self, client_key: usize, response: &ControlResponse) {
        if let Some(stream) = self.clients.get_mut(&client_key) {
            let text = format_response(response);
            if let Err(e) = stream.write_all(text.as_bytes()) {
                warn!("Failed to send response to client {}: {}", client_key, e);
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        info!("Control server shut down");
    }
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

fn format_response(response: &ControlResponse) -> String {
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
        },
        ControlResponse::Error { code, message } => {
            format!("ERROR {}: {}\n", code, message)
        }
    }
}
