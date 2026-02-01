//! Protocol server - accepts TUI app connections on a Unix socket.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use polling::{Event, Events, Poller};
use tracing::{error, info};

use super::session::NativeSession;

const LISTENER_KEY: usize = 0;
const SESSION_KEY_BASE: usize = 1000;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub struct ProtocolServer {
    listener: UnixListener,
    socket_path: PathBuf,
    poller: Poller,
    sessions: HashMap<usize, NativeSession>,
    next_key: usize,
    viewport_width: f32,
    viewport_height: f32,
}

impl ProtocolServer {
    pub fn new(socket_path: impl AsRef<Path>) -> io::Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();

        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }

        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let poller = Poller::new()?;

        // SAFETY: listener fd is valid and we maintain it for the lifetime of the server
        unsafe {
            poller.add(&listener, Event::readable(LISTENER_KEY))?;
        }

        info!("Protocol server listening on {:?}", socket_path);

        Ok(Self {
            listener,
            socket_path,
            poller,
            sessions: HashMap::new(),
            next_key: SESSION_KEY_BASE,
            viewport_width: 80.0,
            viewport_height: 24.0,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport_width = width;
        self.viewport_height = height;
        for session in self.sessions.values_mut() {
            session.set_viewport(width, height);
        }
    }

    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let mut events = Events::new();

        self.poller.wait(&mut events, timeout)?;

        for event in events.iter() {
            if event.key == LISTENER_KEY {
                self.accept_connections()?;
            } else {
                self.handle_session_event(event.key)?;
            }
        }

        Ok(())
    }

    fn accept_connections(&mut self) -> io::Result<()> {
        loop {
            match self.listener.accept() {
                Ok((stream, addr)) => {
                    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
                    info!(
                        "Accepted connection from {:?}, session {}",
                        addr, session_id
                    );

                    let mut session = NativeSession::new(stream, session_id)?;
                    session.set_viewport(self.viewport_width, self.viewport_height);

                    let key = self.next_key;
                    self.next_key += 1;

                    self.sessions.insert(key, session);
                    self.register_session(key)?;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        self.poller
            .modify(&self.listener, Event::readable(LISTENER_KEY))?;

        Ok(())
    }

    fn register_session(&mut self, key: usize) -> io::Result<()> {
        if let Some(session) = self.sessions.get(&key) {
            // SAFETY: session stream fd is valid and maintained for this session's lifetime
            unsafe {
                self.poller
                    .add(&session.stream_fd(), Event::readable(key))?;
            }
        }
        Ok(())
    }

    fn handle_session_event(&mut self, key: usize) -> io::Result<()> {
        let should_remove = if let Some(session) = self.sessions.get_mut(&key) {
            match session.poll() {
                Ok(true) => {
                    let interest = if session.has_pending_writes() {
                        Event::all(key)
                    } else {
                        Event::readable(key)
                    };
                    self.poller.modify(&session.stream_fd(), interest)?;
                    false
                }
                Ok(false) => {
                    info!("Session {} ended", session.session_id());
                    true
                }
                Err(e) => {
                    error!("Session {} error: {}", session.session_id(), e);
                    true
                }
            }
        } else {
            false
        };

        if should_remove {
            if let Some(session) = self.sessions.remove(&key) {
                let _ = self.poller.delete(&session.stream_fd());
            }
        }

        Ok(())
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn sessions(&self) -> impl Iterator<Item = &NativeSession> {
        self.sessions.values()
    }

    pub fn sessions_mut(&mut self) -> impl Iterator<Item = &mut NativeSession> {
        self.sessions.values_mut()
    }

    pub fn broadcast_key(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) {
        for session in self.sessions.values_mut() {
            if let Err(e) = session.send_key(key.clone(), modifiers) {
                tracing::warn!(
                    "Failed to send key to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
    }

    pub fn broadcast_resize(&mut self, width: u16, height: u16) {
        for session in self.sessions.values_mut() {
            session.set_viewport(width as f32, height as f32);
            if let Err(e) = session.send_resize(width, height) {
                tracing::warn!(
                    "Failed to send resize to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
    }
}

impl Drop for ProtocolServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        info!("Protocol server shut down");
    }
}
