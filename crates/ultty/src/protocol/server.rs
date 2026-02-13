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
    /// Create a new protocol server listening on the given Unix socket path.
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

    /// Returns the path to the Unix socket this server is listening on.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Update the viewport dimensions and propagate to all connected sessions.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport_width = width;
        self.viewport_height = height;
        for session in self.sessions.values_mut() {
            session.set_viewport(width, height);
        }
    }

    /// Poll for new connections and session events, with an optional timeout.
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
                    self.poller.modify(session.stream_fd(), interest)?;
                    false
                }
                Ok(false) => {
                    info!("Session {} ended", session.session_id());
                    true
                }
                Err(e) => {
                    if is_disconnect_error(&e) {
                        info!("Session {} disconnected: {}", session.session_id(), e);
                    } else {
                        error!("Session {} error: {:?}", session.session_id(), e);
                    }
                    true
                }
            }
        } else {
            false
        };

        if should_remove {
            if let Some(session) = self.sessions.remove(&key) {
                let _ = self.poller.delete(session.stream_fd());
            }
        }

        Ok(())
    }

    /// Returns the number of currently connected native sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Iterate over all connected sessions (immutable).
    pub fn sessions(&self) -> impl Iterator<Item = &NativeSession> {
        self.sessions.values()
    }

    /// Iterate over all connected sessions (mutable).
    pub fn sessions_mut(&mut self) -> impl Iterator<Item = &mut NativeSession> {
        self.sessions.values_mut()
    }

    /// Send a key event to all connected native sessions that have subscribed to key events.
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
        self.flush();
    }

    pub fn has_key_subscribers(&self) -> bool {
        self.sessions.values().any(|s| s.has_key_subscription())
    }

    /// Send a key-up event to all connected native sessions that have subscribed to key events.
    pub fn broadcast_key_up(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) {
        for session in self.sessions.values_mut() {
            if let Err(e) = session.send_key_up(key.clone(), modifiers) {
                tracing::warn!(
                    "Failed to send key_up to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// Send a text input event to all connected native sessions that have subscribed to key events.
    pub fn broadcast_text_input(&mut self, text: String) {
        for session in self.sessions.values_mut() {
            if let Err(e) = session.send_text_input(text.clone()) {
                tracing::warn!(
                    "Failed to send text_input to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// Forward a mouse event to all native sessions.
    /// Hit-tests each session's scene tree to find the target region,
    /// checks subscriptions, and sends the event with region-local coords.
    pub fn broadcast_mouse(
        &mut self,
        event_type: ultty_protocol::InputType,
        cell_x: f32,
        cell_y: f32,
        modifiers: ultty_protocol::Modifiers,
    ) {
        let event_mask = match &event_type {
            ultty_protocol::InputType::MouseScroll { .. } => ultty_protocol::EVENT_SCROLL,
            _ => ultty_protocol::EVENT_MOUSE,
        };
        for session in self.sessions.values_mut() {
            let (region_id, local_x, local_y) = session.hit_test(cell_x, cell_y);
            if session.is_subscribed(region_id, event_mask)
                || session.is_subscribed(ultty_protocol::RegionId::ROOT, event_mask)
            {
                let input = ultty_protocol::InputEvent {
                    region_id,
                    event_type: event_type.clone(),
                    x: local_x,
                    y: local_y,
                    modifiers,
                };
                if let Err(e) = session.send_input(input) {
                    tracing::warn!(
                        "Failed to send mouse to session {}: {}",
                        session.session_id(),
                        e
                    );
                }
            }
        }
        self.flush();
    }

    /// Collect pending clipboard get requests from all sessions.
    /// Returns a list of session keys that requested clipboard content.
    pub fn pending_clipboard_gets(&mut self) -> Vec<usize> {
        let mut gets = Vec::new();
        for (&key, session) in &mut self.sessions {
            if session.take_clipboard_get() {
                gets.push(key);
            }
        }
        gets
    }

    /// Collect pending clipboard set requests from all sessions.
    /// Returns a list of (session_key, text) pairs.
    pub fn pending_clipboard_sets(&mut self) -> Vec<(usize, String)> {
        let mut sets = Vec::new();
        for (&key, session) in &mut self.sessions {
            if let Some(text) = session.take_clipboard_set() {
                sets.push((key, text));
            }
        }
        sets
    }

    /// Send clipboard content to a specific session (identified by key).
    pub fn send_clipboard_content(&mut self, session_key: usize, text: Option<String>) {
        if let Some(session) = self.sessions.get_mut(&session_key) {
            if let Err(e) = session.send_clipboard_content(text) {
                tracing::warn!(
                    "Failed to send clipboard content to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// Advance all animations (scroll and style transitions) across all sessions.
    /// Returns true if any session still has active animations.
    pub fn update_animations(&mut self) -> bool {
        let mut any_active = false;
        for session in self.sessions.values_mut() {
            if session.update_scroll_animations() {
                any_active = true;
            }
            if session.update_style_animations() {
                any_active = true;
            }
        }
        any_active
    }

    /// Check keepalive state for all sessions. Removes timed-out sessions.
    pub fn check_keepalives(&mut self) {
        let mut to_remove = Vec::new();
        for (&key, session) in &mut self.sessions {
            match session.check_keepalive() {
                Ok(true) => {}
                Ok(false) => {
                    info!("Session {} timed out", session.session_id());
                    to_remove.push(key);
                }
                Err(e) => {
                    error!("Session {} keepalive error: {}", session.session_id(), e);
                    to_remove.push(key);
                }
            }
        }
        for key in to_remove {
            if let Some(session) = self.sessions.remove(&key) {
                let _ = self.poller.delete(session.stream_fd());
            }
        }
    }

    /// Flush pending writes for all sessions.
    pub fn flush(&mut self) {
        for session in self.sessions.values_mut() {
            if session.has_pending_writes() {
                if let Err(e) = session.flush() {
                    tracing::warn!("Failed to flush session {}: {}", session.session_id(), e);
                }
            }
        }
    }

    /// Send a resize event to all connected native sessions.
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
        self.flush();
    }
}

/// Check if an IO error represents a normal client disconnection.
fn is_disconnect_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof
    )
}

impl Drop for ProtocolServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        info!("Protocol server shut down");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn temp_socket_path() -> PathBuf {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        PathBuf::from(format!("/tmp/ultty-test-{}-{}", pid, id))
    }

    /// Helper: ensure the socket file is removed even if the test panics.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    // ---------------------------------------------------------------
    // 1. Lifecycle: new() creates socket, drop removes it
    // ---------------------------------------------------------------
    #[test]
    fn new_creates_socket_file() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let server = ProtocolServer::new(&path).expect("failed to create server");
        assert!(path.exists(), "socket file should exist after new()");
        drop(server);
        assert!(!path.exists(), "socket file should be removed after drop");
    }

    // ---------------------------------------------------------------
    // 2. set_viewport updates dimensions
    // ---------------------------------------------------------------
    #[test]
    fn set_viewport_updates_dimensions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        server.set_viewport(120.0, 40.0);
        // The internal fields should now reflect the new viewport.
        // We can verify indirectly by checking that a newly accepted
        // session would inherit these values, but at minimum the call
        // must not panic.
        assert_eq!(server.viewport_width, 120.0);
        assert_eq!(server.viewport_height, 40.0);
    }

    // ---------------------------------------------------------------
    // 3. session_count starts at 0
    // ---------------------------------------------------------------
    #[test]
    fn session_count_starts_at_zero() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let server = ProtocolServer::new(&path).unwrap();
        assert_eq!(server.session_count(), 0);
    }

    // ---------------------------------------------------------------
    // 4. poll with no connections returns Ok
    // ---------------------------------------------------------------
    #[test]
    fn poll_no_connections_returns_ok() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        // Use a zero-duration timeout so poll returns immediately.
        let result = server.poll(Some(Duration::ZERO));
        assert!(result.is_ok(), "poll with no connections should succeed");
    }

    // ---------------------------------------------------------------
    // 5. broadcast_key with no sessions doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_key_no_sessions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        // Should not panic when there are no sessions.
        server.broadcast_key(
            ultty_protocol::Key::Char('a'),
            ultty_protocol::Modifiers::NONE,
        );
    }

    // ---------------------------------------------------------------
    // 6. broadcast_mouse with no sessions doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_mouse_no_sessions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        server.broadcast_mouse(
            ultty_protocol::InputType::MouseDown {
                button: ultty_protocol::MouseButton::Left,
            },
            10.0,
            5.0,
            ultty_protocol::Modifiers::NONE,
        );
    }

    // ---------------------------------------------------------------
    // 7. broadcast_resize with no sessions doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_resize_no_sessions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        server.broadcast_resize(120, 40);
    }

    // ---------------------------------------------------------------
    // 8. update_animations with no sessions returns false
    // ---------------------------------------------------------------
    #[test]
    fn update_animations_no_sessions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        assert!(!server.update_animations());
    }

    // ---------------------------------------------------------------
    // 9. socket_path returns correct path
    // ---------------------------------------------------------------
    #[test]
    fn socket_path_returns_correct_path() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let server = ProtocolServer::new(&path).unwrap();
        assert_eq!(server.socket_path(), path.as_path());
    }

    // ---------------------------------------------------------------
    // 10. Accepting a connection increments session_count
    // ---------------------------------------------------------------
    #[test]
    fn accept_connection_increments_session_count() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        assert_eq!(server.session_count(), 0);

        // Connect a client to the server socket.
        let _client = UnixStream::connect(&path).expect("failed to connect");

        // poll() should accept the pending connection.
        server
            .poll(Some(Duration::from_millis(100)))
            .expect("poll failed");

        assert_eq!(server.session_count(), 1);
    }

    // ---------------------------------------------------------------
    // 11. Multiple connections are tracked independently
    // ---------------------------------------------------------------
    #[test]
    fn multiple_connections_tracked() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();

        let _c1 = UnixStream::connect(&path).unwrap();
        let _c2 = UnixStream::connect(&path).unwrap();

        server
            .poll(Some(Duration::from_millis(100)))
            .expect("poll failed");

        assert_eq!(server.session_count(), 2);
    }

    // ---------------------------------------------------------------
    // 12. broadcast_key with a connected session doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_key_with_session() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        assert_eq!(server.session_count(), 1);
        server.broadcast_key(ultty_protocol::Key::Enter, ultty_protocol::Modifiers::CTRL);
    }

    // ---------------------------------------------------------------
    // 13. broadcast_mouse with a connected session doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_mouse_with_session() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        server.broadcast_mouse(
            ultty_protocol::InputType::MouseScroll {
                delta_x: 0.0,
                delta_y: -3.0,
            },
            20.0,
            10.0,
            ultty_protocol::Modifiers::NONE,
        );
    }

    // ---------------------------------------------------------------
    // 14. broadcast_resize with a connected session doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_resize_with_session() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        server.broadcast_resize(200, 60);
    }

    // ---------------------------------------------------------------
    // 15. new() removes stale socket file before binding
    // ---------------------------------------------------------------
    #[test]
    fn new_removes_stale_socket() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        // Create a first server (creates the socket file).
        let server1 = ProtocolServer::new(&path).unwrap();
        // Intentionally leak (don't drop) so socket file persists by
        // just forgetting the Drop.
        std::mem::forget(server1);

        // A second server on the same path should succeed because
        // new() removes the stale file.
        let _server2 = ProtocolServer::new(&path)
            .expect("should be able to create server on path with stale socket");
    }

    // ---------------------------------------------------------------
    // 16. set_viewport propagates to existing sessions
    // ---------------------------------------------------------------
    #[test]
    fn set_viewport_propagates_to_sessions() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        server.set_viewport(200.0, 50.0);

        // Verify each session received the viewport update.
        for session in server.sessions() {
            let (w, h) = session.viewport();
            assert_eq!(w, 200.0);
            assert_eq!(h, 50.0);
        }
    }

    // ---------------------------------------------------------------
    // 17. sessions() iterator works
    // ---------------------------------------------------------------
    #[test]
    fn sessions_iterator() {
        let path = temp_socket_path();
        let _cleanup = Cleanup(path.clone());

        let mut server = ProtocolServer::new(&path).unwrap();
        assert_eq!(server.sessions().count(), 0);

        let _c1 = UnixStream::connect(&path).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();

        assert_eq!(server.sessions().count(), 1);
    }
}
