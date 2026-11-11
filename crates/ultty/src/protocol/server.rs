//! Protocol server - accepts TUI app connections on per-tab Unix sockets.
//!
//! Every tab listens on its own socket, and the tab's shell is told about
//! that socket alone (`ULTTY_SOCKET`). A session therefore belongs to the
//! tab whose socket accepted it, which is what lets the server route input
//! and clipboard access to the tab the user is looking at rather than to
//! whichever process connected most recently. Within that tab, keys go only
//! to sessions whose connecting process is in the focused pane's foreground
//! process group.
//!
//! Apps can also talk in-band on a tab's PTY (ADR 0018). The app layer feeds
//! that data in with [`ProtocolServer::in_band_input`] and writes what
//! [`ProtocolServer::take_in_band_output`] returns back to the PTY. A tab
//! has at most one in-band session. It starts only on a well-formed
//! `ClientHello`, so stray in-band strings (a `cat`ed file, say) cannot
//! open one. The session is not shown or sent input until the app sends
//! more after the hello was answered, so bytes that merely contain a hello
//! cannot take the tab over; it ends if that does not happen within 2 s
//! (`IN_BAND_CONFIRM_TIMEOUT`), when the app disconnects, falls silent for
//! 10 s (`IN_BAND_IDLE_TIMEOUT`), or the PTY's foreground process group
//! changes.
//!
//! Anything that can write to the tab can send in-band data, including a
//! remote host over ssh, so in-band sessions never read the clipboard.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use polling::{Event, Events, Poller};
use tracing::{error, info, warn};

use super::pairing::AnchorPairing;
use super::session::{LiveChange, NativeSession};
use crate::grid::Terminal;
use crate::platform::socket::{peer_uid, set_cloexec, tab_socket_file_name};
use crate::terminal_manager::{TabId, TerminalId};
use ultty_protocol::FRAME_MAGIC;

/// Session identifier assigned by the server on accept.
pub type SessionId = u64;

/// Type code of `ClientHello`, the only frame that may open an in-band session.
const CLIENT_HELLO_TYPE: u16 = 0x0001;

/// Largest `ClientHello` payload accepted when opening an in-band session.
const MAX_CLIENT_HELLO_PAYLOAD: usize = 64;

/// Permissions applied to each listening socket file: owner only.
const SOCKET_FILE_MODE: u32 = 0o600;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// One tab's listening socket.
struct TabListener {
    tab_id: TabId,
    listener: UnixListener,
    socket_path: PathBuf,
}

pub struct ProtocolServer {
    /// Directory that holds the per-tab socket files.
    socket_dir: PathBuf,
    poller: Poller,
    /// Listeners keyed by their poller key.
    listeners: HashMap<usize, TabListener>,
    /// Sessions keyed by their poller key.
    sessions: HashMap<usize, NativeSession>,
    /// Next poller key; listeners and sessions share one key space.
    next_key: usize,
    viewport_width: f32,
    viewport_height: f32,
    /// Tab whose sessions receive keyboard input and may read the clipboard.
    ///
    /// `None` withholds input from every session. The app layer keeps this
    /// in sync with the visible tab via `set_active_tab`.
    active_tab: Option<TabId>,
    /// In-band bytes per tab that may be the start of a `ClientHello`.
    in_band_openers: HashMap<TabId, Vec<u8>>,
    /// Inline surface anchors of each terminal that has had any (ADR 0020).
    pairings: HashMap<TerminalId, AnchorPairing>,
    /// Whether sessions keep text appended to live regions for a screen
    /// reader ([`ProtocolServer::set_live_announcements`]).
    live_announcements: bool,
}

/// Whether `bytes` are, or may become, a complete `ClientHello` frame.
enum HelloCheck {
    /// A whole `ClientHello` frame starts the buffer.
    Complete,
    /// A prefix of one; wait for more bytes.
    Partial,
    /// Not a `ClientHello`.
    No,
}

fn check_client_hello(bytes: &[u8]) -> HelloCheck {
    let header = [
        FRAME_MAGIC[0],
        FRAME_MAGIC[1],
        CLIENT_HELLO_TYPE.to_le_bytes()[0],
        CLIENT_HELLO_TYPE.to_le_bytes()[1],
    ];
    let known = bytes.len().min(header.len());
    if bytes[..known] != header[..known] {
        return HelloCheck::No;
    }
    if bytes.len() < 8 {
        return HelloCheck::Partial;
    }
    let payload_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if payload_len > MAX_CLIENT_HELLO_PAYLOAD {
        return HelloCheck::No;
    }
    if bytes.len() < 8 + payload_len {
        return HelloCheck::Partial;
    }
    use ultty_protocol::Decode;
    match ultty_protocol::AppMessage::decode(&mut &bytes[..8 + payload_len]) {
        Ok(ultty_protocol::AppMessage::ClientHello { .. }) => HelloCheck::Complete,
        _ => HelloCheck::No,
    }
}

impl ProtocolServer {
    /// Create a protocol server whose per-tab sockets live in `socket_dir`.
    ///
    /// No socket is bound until a tab is opened with [`open_tab`].
    ///
    /// [`open_tab`]: ProtocolServer::open_tab
    pub fn new(socket_dir: impl AsRef<Path>) -> io::Result<Self> {
        let socket_dir = socket_dir.as_ref().to_path_buf();
        fs::create_dir_all(&socket_dir)?;

        Ok(Self {
            socket_dir,
            poller: Poller::new()?,
            listeners: HashMap::new(),
            sessions: HashMap::new(),
            next_key: 0,
            viewport_width: 80.0,
            viewport_height: 24.0,
            active_tab: None,
            in_band_openers: HashMap::new(),
            pairings: HashMap::new(),
            live_announcements: false,
        })
    }

    /// Directory that holds the per-tab socket files.
    pub fn socket_dir(&self) -> &Path {
        &self.socket_dir
    }

    /// Bind the listening socket for `tab_id` and return its path.
    ///
    /// The socket file is owner-only (0600), the listener is close-on-exec
    /// so the tab's shell cannot inherit it, and each accepted peer must
    /// run as the terminal's uid. Opening a tab twice is an error.
    pub fn open_tab(&mut self, tab_id: TabId) -> io::Result<PathBuf> {
        if self.listener_key(tab_id).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("tab {} already has a protocol socket", tab_id.raw()),
            ));
        }

        let socket_path = self.socket_dir.join(tab_socket_file_name(tab_id));
        if socket_path.exists() {
            remove_stale_socket(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(SOCKET_FILE_MODE))?;
        set_cloexec(&listener)?;
        listener.set_nonblocking(true)?;

        let key = self.allocate_key();
        // SAFETY: the listener fd stays valid until `close_tab` deletes it
        // from the poller and drops it.
        unsafe {
            self.poller.add(&listener, Event::readable(key))?;
        }

        info!(
            "Protocol server listening for tab {} on {:?}",
            tab_id.raw(),
            socket_path
        );
        self.listeners.insert(
            key,
            TabListener {
                tab_id,
                listener,
                socket_path: socket_path.clone(),
            },
        );
        Ok(socket_path)
    }

    /// Stop listening for `tab_id`: unlink its socket and disconnect its
    /// sessions. A tab that was never opened is ignored.
    pub fn close_tab(&mut self, tab_id: TabId) {
        self.in_band_openers.remove(&tab_id);
        if let Some(key) = self.listener_key(tab_id) {
            if let Some(tab) = self.listeners.remove(&key) {
                let _ = self.poller.delete(&tab.listener);
                let _ = fs::remove_file(&tab.socket_path);
                info!("Protocol server closed tab {} socket", tab_id.raw());
            }
        }

        let session_keys: Vec<usize> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.tab_id() == tab_id)
            .map(|(&key, _)| key)
            .collect();
        for key in session_keys {
            self.remove_session(key);
        }
    }

    /// Socket path of an open tab.
    pub fn tab_socket_path(&self, tab_id: TabId) -> Option<&Path> {
        self.listener_key(tab_id)
            .and_then(|key| self.listeners.get(&key))
            .map(|tab| tab.socket_path.as_path())
    }

    /// Tabs that currently have a listening socket.
    pub fn open_tabs(&self) -> impl Iterator<Item = TabId> + '_ {
        self.listeners.values().map(|tab| tab.tab_id)
    }

    /// Tab whose sessions receive keyboard input and may read the clipboard.
    pub fn active_tab(&self) -> Option<TabId> {
        self.active_tab
    }

    /// Route keyboard input and clipboard reads to the sessions of `tab_id`.
    ///
    /// `None` withholds input from every session. Sessions of a newly
    /// activated tab are marked render-dirty so the renderer rebuilds them
    /// instead of reusing buffers cached for the previous tab.
    pub fn set_active_tab(&mut self, tab_id: Option<TabId>) {
        if self.active_tab == tab_id {
            return;
        }
        self.active_tab = tab_id;
        for session in self.active_tab_sessions_mut() {
            session.mark_render_dirty();
        }
    }

    /// Sessions that belong to the active tab and are shown there: an
    /// in-band session only once confirmed (see
    /// [`NativeSession::is_confirmed`]).
    pub fn active_tab_sessions(&self) -> impl Iterator<Item = &NativeSession> {
        let active = self.active_tab;
        self.sessions
            .values()
            .filter(move |session| Some(session.tab_id()) == active && session.is_confirmed())
    }

    /// Sessions that belong to the active tab and are shown there (mutable).
    pub fn active_tab_sessions_mut(&mut self) -> impl Iterator<Item = &mut NativeSession> {
        let active = self.active_tab;
        self.sessions
            .values_mut()
            .filter(move |session| Some(session.tab_id()) == active && session.is_confirmed())
    }

    /// The shown sessions (see [`Self::active_tab_sessions`]) bound to
    /// `pane`: the ones drawn in its rectangle (ADR 0025).
    pub fn pane_sessions(&self, pane: TerminalId) -> impl Iterator<Item = &NativeSession> {
        self.active_tab_sessions()
            .filter(move |session| session.pane() == Some(pane))
    }

    /// Bind the sessions that have no pane yet (ADR 0025 §1). `pane_for`
    /// gets a session's tab and peer pid and names the pane whose PTY runs
    /// the app, or `None` to leave it unbound for now. Returns how many
    /// sessions were bound, so the caller can size them.
    pub fn bind_unbound_sessions(
        &mut self,
        mut pane_for: impl FnMut(TabId, Option<i32>) -> Option<TerminalId>,
    ) -> usize {
        let mut bound = 0;
        for session in self.sessions.values_mut() {
            if session.pane().is_some() {
                continue;
            }
            if let Some(pane) = pane_for(session.tab_id(), session.peer_pid()) {
                info!(
                    "Session {} bound to pane {}",
                    session.session_id(),
                    pane.raw()
                );
                session.bind_pane(pane);
                bound += 1;
            }
        }
        bound
    }

    /// Disconnect the sessions bound to a pane `alive` rejects: a closed
    /// pane's sessions go the way a closed tab's do.
    pub fn retain_pane_sessions(&mut self, mut alive: impl FnMut(TerminalId) -> bool) {
        let gone: Vec<usize> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.pane().is_some_and(|pane| !alive(pane)))
            .map(|(&key, _)| key)
            .collect();
        for key in gone {
            info!("Session of a closed pane disconnected");
            self.remove_session(key);
        }
    }

    /// Size every session to where it is drawn: a bound session to its
    /// pane's grid (`pane_size`), an unbound one to the window's `(cols,
    /// rows)`. A session whose size changed gets `Resize` and, with a
    /// scene, `LayoutComputed` (ADR 0025 §2). New sessions start at the
    /// window's size.
    pub fn resize_sessions(
        &mut self,
        (cols, rows): (u16, u16),
        mut pane_size: impl FnMut(TerminalId) -> Option<(u16, u16)>,
    ) {
        self.viewport_width = f32::from(cols);
        self.viewport_height = f32::from(rows);
        for session in self.sessions.values_mut() {
            let (cols, rows) = session
                .pane()
                .and_then(&mut pane_size)
                .unwrap_or((cols, rows));
            if let Err(e) = session.resize_to(cols, rows) {
                warn!(
                    "Failed to send resize to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    fn allocate_key(&mut self) -> usize {
        let key = self.next_key;
        self.next_key += 1;
        key
    }

    fn listener_key(&self, tab_id: TabId) -> Option<usize> {
        self.listeners
            .iter()
            .find(|(_, tab)| tab.tab_id == tab_id)
            .map(|(&key, _)| key)
    }

    fn remove_session(&mut self, key: usize) {
        if let Some(mut session) = self.sessions.remove(&key) {
            // Its surfaces freeze where they stand, with their last cells.
            session.freeze_surfaces();
            for pairing in self.pairings.values_mut() {
                pairing.session_ending(&mut session);
            }
            if let Some(fd) = session.stream_fd() {
                let _ = self.poller.delete(fd);
            }
        }
    }

    /// File descriptors that should wake the UI loop when readable.
    pub fn readiness_fds(&self) -> Vec<RawFd> {
        let mut fds = Vec::with_capacity(self.listeners.len() + self.sessions.len());
        fds.extend(self.listeners.values().map(|tab| tab.listener.as_raw_fd()));
        fds.extend(
            self.sessions
                .values()
                .filter_map(|session| session.stream_fd().map(|fd| fd.as_raw_fd())),
        );
        fds
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
            if self.listeners.contains_key(&event.key) {
                self.accept_connections(event.key)?;
            } else {
                self.handle_session_event(event.key)?;
            }
        }

        Ok(())
    }

    fn accept_connections(&mut self, listener_key: usize) -> io::Result<()> {
        let mut accepted = Vec::new();
        {
            let Some(tab) = self.listeners.get(&listener_key) else {
                return Ok(());
            };
            loop {
                match tab.listener.accept() {
                    Ok((stream, addr)) => {
                        if !peer_is_authorized(&stream) {
                            continue;
                        }
                        let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
                        info!(
                            "Accepted connection from {:?} on tab {}, session {}",
                            addr,
                            tab.tab_id.raw(),
                            session_id
                        );
                        let mut session = NativeSession::new(stream, session_id, tab.tab_id)?;
                        session.set_inline_surfaces(true);
                        session.set_live_announcements(self.live_announcements);
                        accepted.push(session);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                }
            }
            self.poller
                .modify(&tab.listener, Event::readable(listener_key))?;
        }

        for mut session in accepted {
            session.set_viewport(self.viewport_width, self.viewport_height);
            let key = self.allocate_key();
            self.sessions.insert(key, session);
            self.register_session(key)?;
        }

        Ok(())
    }

    fn register_session(&mut self, key: usize) -> io::Result<()> {
        if let Some(fd) = self.sessions.get(&key).and_then(NativeSession::stream_fd) {
            // SAFETY: session stream fd is valid and maintained for this session's lifetime
            unsafe {
                self.poller.add(&fd, Event::readable(key))?;
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
                    if let Some(fd) = session.stream_fd() {
                        self.poller.modify(fd, interest)?;
                    }
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
            self.remove_session(key);
        }

        Ok(())
    }

    /// Feed in-band data that arrived on `tab_id`'s PTY, whose foreground
    /// process group is now `pgrp`. `broken` means some of the stream was
    /// malformed or lost; it ends the tab's in-band session. `pids` lists
    /// the group's processes; it is called only when a session opens.
    ///
    /// Replies are queued; collect them with
    /// [`ProtocolServer::take_in_band_output`].
    pub fn in_band_input(
        &mut self,
        tab_id: TabId,
        pgrp: Option<i32>,
        pids: impl FnOnce() -> Vec<i32>,
        bytes: &[u8],
        broken: bool,
    ) {
        self.end_in_band_if_app_gone(tab_id, pgrp);
        let key = self.in_band_session_key(tab_id);
        if broken {
            self.in_band_openers.remove(&tab_id);
            if let Some(key) = key {
                warn!(
                    "In-band stream on tab {} was malformed; ending its session",
                    tab_id.raw()
                );
                self.remove_session(key);
            }
            return;
        }

        let key = match key {
            Some(key) => key,
            None => {
                let opener = self.in_band_openers.entry(tab_id).or_default();
                opener.extend_from_slice(bytes);
                match check_client_hello(opener) {
                    HelloCheck::Partial => return,
                    HelloCheck::No => {
                        self.in_band_openers.remove(&tab_id);
                        return;
                    }
                    HelloCheck::Complete => {}
                }
                let opener = self.in_band_openers.remove(&tab_id).unwrap_or_default();
                let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
                info!(
                    "In-band session {} started on tab {}",
                    session_id,
                    tab_id.raw()
                );
                let mut session = NativeSession::new_in_band(session_id, tab_id, pgrp, pids());
                session.set_inline_surfaces(true);
                session.set_live_announcements(self.live_announcements);
                session.set_viewport(self.viewport_width, self.viewport_height);
                let key = self.allocate_key();
                self.sessions.insert(key, session);
                self.feed_session(key, &opener);
                return;
            }
        };
        if !bytes.is_empty() {
            // Data in a later read than the hello: the app read the answer.
            if let Some(session) = self.sessions.get_mut(&key) {
                session.confirm_in_band();
            }
        }
        self.feed_session(key, bytes);
    }

    fn feed_session(&mut self, key: usize, bytes: &[u8]) {
        let Some(session) = self.sessions.get_mut(&key) else {
            return;
        };
        let alive = session.feed_in_band(bytes).and_then(|()| session.poll());
        match alive {
            Ok(true) => {}
            Ok(false) => {
                info!("In-band session {} ended", session.session_id());
                self.remove_session(key);
            }
            Err(e) => {
                warn!("In-band session {} failed: {}", session.session_id(), e);
                self.remove_session(key);
            }
        }
    }

    /// End `tab_id`'s in-band session if its app no longer runs: the PTY's
    /// foreground process group (now `pgrp`) changed, or a process that was
    /// in it at hello has exited. See [`NativeSession::in_band_app_alive`].
    pub fn end_in_band_if_app_gone(&mut self, tab_id: TabId, pgrp: Option<i32>) {
        if let Some(key) = self.in_band_session_key(tab_id) {
            if !self.sessions[&key].in_band_app_alive(pgrp) {
                info!(
                    "In-band session {} ended: its app is no longer running",
                    self.sessions[&key].session_id()
                );
                self.remove_session(key);
            }
        }
    }

    /// Frame bytes queued for `tab_id`'s in-band app since the last call.
    pub fn take_tab_in_band_output(&mut self, tab_id: TabId) -> Vec<u8> {
        self.in_band_session_key(tab_id)
            .and_then(|key| self.sessions.get_mut(&key))
            .map(NativeSession::take_in_band_output)
            .unwrap_or_default()
    }

    /// End `tab_id`'s in-band session, if it has one.
    pub fn end_in_band_session(&mut self, tab_id: TabId) {
        self.in_band_openers.remove(&tab_id);
        if let Some(key) = self.in_band_session_key(tab_id) {
            info!(
                "In-band session {} ended by the terminal",
                self.sessions[&key].session_id()
            );
            self.remove_session(key);
        }
    }

    /// Frame bytes queued for in-band apps, per tab, since the last call.
    pub fn take_in_band_output(&mut self) -> Vec<(TabId, Vec<u8>)> {
        self.sessions
            .values_mut()
            .filter(|session| session.is_in_band())
            .filter_map(|session| {
                let out = session.take_in_band_output();
                (!out.is_empty()).then(|| (session.tab_id(), out))
            })
            .collect()
    }

    /// Tabs that have an in-band session.
    pub fn in_band_tabs(&self) -> Vec<TabId> {
        self.sessions
            .values()
            .filter(|session| session.is_in_band())
            .map(NativeSession::tab_id)
            .collect()
    }

    fn in_band_session_key(&self, tab_id: TabId) -> Option<usize> {
        self.sessions
            .iter()
            .find(|(_, session)| session.is_in_band() && session.tab_id() == tab_id)
            .map(|(&key, _)| key)
    }

    /// Whether terminal `terminal_id` needs a pairing pass even without new
    /// output: it has anchors waiting for a surface or under one, or a
    /// session of its tab holds surfaces.
    pub fn needs_pairing(&self, tab_id: TabId, terminal_id: TerminalId) -> bool {
        self.pairings
            .get(&terminal_id)
            .is_some_and(|pairing| !pairing.is_idle())
            || self
                .sessions
                .values()
                .any(|session| session.tab_id() == tab_id && session.has_surfaces())
    }

    /// Pair the inline surfaces of `tab_id`'s sessions with the anchors in
    /// `terminal`'s output, as of `now`; see [`AnchorPairing::update`].
    /// `single_pane` says the terminal is the tab's only pane, and
    /// `on_screen` that it is drawn now. Returns whether a surface was
    /// written into the terminal's rows.
    pub fn pair_inline_surfaces(
        &mut self,
        tab_id: TabId,
        terminal_id: TerminalId,
        terminal: &mut Terminal,
        single_pane: bool,
        on_screen: bool,
        now: Instant,
    ) -> bool {
        let mut sessions: Vec<&mut NativeSession> = self
            .sessions
            .values_mut()
            .filter(|session| session.tab_id() == tab_id)
            .collect();
        let pairing = self.pairings.entry(terminal_id).or_default();
        let wrote = pairing.update(terminal, &mut sessions, single_pane, on_screen, now);
        if pairing.is_idle() {
            self.pairings.remove(&terminal_id);
        }
        wrote
    }

    /// Drop, with `Error` 400, every surface whose anchor has not come in
    /// time. Call it after the pairing passes of the same `now`.
    pub fn expire_unplaced_surfaces(&mut self, now: Instant) {
        for session in self.sessions.values_mut() {
            session.expire_unplaced_surfaces(now);
        }
    }

    /// Forget the anchors of terminals that are gone, evicting the
    /// surfaces placed on them.
    pub fn retain_pairings(&mut self, mut live: impl FnMut(TerminalId) -> bool) {
        let gone: Vec<TerminalId> = self
            .pairings
            .keys()
            .copied()
            .filter(|&terminal_id| !live(terminal_id))
            .collect();
        for terminal_id in gone {
            if let Some(mut pairing) = self.pairings.remove(&terminal_id) {
                let mut sessions: Vec<&mut NativeSession> = self.sessions.values_mut().collect();
                pairing.abandon(&mut sessions);
            }
        }
    }

    /// When the UI must next wake to release an anchor no surface claimed,
    /// or `None` when no anchor waits.
    pub fn next_pairing_deadline(&self) -> Option<Instant> {
        self.pairings
            .values()
            .filter_map(AnchorPairing::next_deadline)
            .min()
    }

    /// Whether every session, and every one that connects, keeps text
    /// appended to its live regions for [`ProtocolServer::take_live_appends`]:
    /// on while a screen reader listens (ADR 0021 §3).
    pub fn set_live_announcements(&mut self, on: bool) {
        if self.live_announcements != on {
            self.live_announcements = on;
            for session in self.sessions.values_mut() {
                session.set_live_announcements(on);
            }
        }
    }

    /// Hand `changed` what happened to the live regions of the active
    /// tab's shown sessions since the last call, as (session id, change).
    /// Other sessions' changes are dropped: they are not on screen, and
    /// must not be announced when their tab is shown later.
    pub fn take_live_appends(&mut self, mut changed: impl FnMut(u64, LiveChange)) {
        if !self.live_announcements {
            return;
        }
        let active = self.active_tab;
        for session in self.sessions.values_mut() {
            let shown = Some(session.tab_id()) == active && session.is_confirmed();
            for change in session.take_live_appends() {
                if shown {
                    changed(session.session_id(), change);
                }
            }
        }
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

    /// Sessions that may receive keyboard input typed into the focused pane,
    /// whose PTY has `foreground_pgrp` in the foreground: full-scene
    /// sessions of the active tab (see `set_active_tab`) whose app is that
    /// foreground program (see [`NativeSession::is_in_foreground`]). An app
    /// with only inline surfaces reads keys from its tty (ADR 0020).
    fn key_recipients_mut(
        &mut self,
        foreground_pgrp: Option<i32>,
    ) -> impl Iterator<Item = &mut NativeSession> {
        self.active_tab_sessions_mut().filter(move |session| {
            session.is_full_scene() && session.is_in_foreground(foreground_pgrp)
        })
    }

    /// Send a key event to the sessions that subscribed to keys and may
    /// receive them (see `key_recipients_mut`).
    ///
    /// Keys are never fanned out to every session: a session of a hidden
    /// tab, or of a program in the background, sees none of them.
    pub fn broadcast_key(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
        foreground_pgrp: Option<i32>,
    ) {
        for session in self.key_recipients_mut(foreground_pgrp) {
            if let Err(e) = session.send_key(key.clone(), modifiers) {
                warn!(
                    "Failed to send key to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// True if a session that may receive keys (see `broadcast_key`)
    /// subscribed to them.
    pub fn has_key_subscribers(&self, foreground_pgrp: Option<i32>) -> bool {
        self.active_tab_sessions()
            .filter(|session| session.is_full_scene() && session.is_in_foreground(foreground_pgrp))
            .any(NativeSession::has_key_subscription)
    }

    /// Send a key-up event to the sessions [`ProtocolServer::broadcast_key`]
    /// would send a key to.
    pub fn broadcast_key_up(
        &mut self,
        key: ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
        foreground_pgrp: Option<i32>,
    ) {
        for session in self.key_recipients_mut(foreground_pgrp) {
            if let Err(e) = session.send_key_up(key.clone(), modifiers) {
                warn!(
                    "Failed to send key_up to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// Send a text input event to the sessions
    /// [`ProtocolServer::broadcast_key`] would send a key to.
    pub fn broadcast_text_input(&mut self, text: String, foreground_pgrp: Option<i32>) {
        for session in self.key_recipients_mut(foreground_pgrp) {
            if let Err(e) = session.send_text_input(text.clone()) {
                warn!(
                    "Failed to send text_input to session {}: {}",
                    session.session_id(),
                    e
                );
            }
        }
        self.flush();
    }

    /// Forward a mouse event at `(cell_x, cell_y)`, in `pane`'s cells, to
    /// the shown sessions bound to `pane` (ADR 0025 §4). Hit-tests each
    /// session's scene tree to find the target region, checks
    /// subscriptions, and sends the event with region-local coords.
    pub fn broadcast_mouse(
        &mut self,
        pane: TerminalId,
        event_type: ultty_protocol::InputType,
        cell_x: f32,
        cell_y: f32,
        modifiers: ultty_protocol::Modifiers,
    ) {
        let event_mask = match &event_type {
            ultty_protocol::InputType::MouseScroll { .. } => ultty_protocol::EVENT_SCROLL,
            _ => ultty_protocol::EVENT_MOUSE,
        };
        for session in self
            .active_tab_sessions_mut()
            .filter(|session| session.pane() == Some(pane))
        {
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

    /// Collect pending clipboard get requests.
    ///
    /// Only sessions of the active tab may read the clipboard. Requests from
    /// other sessions are answered with an empty `ClipboardContent` so the
    /// client does not hang, and are not returned here.
    /// Returns a list of session keys that requested clipboard content.
    pub fn pending_clipboard_gets(&mut self) -> Vec<usize> {
        let mut gets = Vec::new();
        let mut denied = false;
        for (&key, session) in &mut self.sessions {
            if !session.take_clipboard_get() {
                continue;
            }
            if session.is_in_band() {
                warn!(
                    "In-band session {} requested the clipboard; in-band sessions may not read it",
                    session.session_id()
                );
                if let Err(e) = session.send_clipboard_content(None) {
                    warn!(
                        "Failed to send clipboard denial to session {}: {}",
                        session.session_id(),
                        e
                    );
                }
                denied = true;
            } else if Some(session.tab_id()) == self.active_tab {
                gets.push(key);
            } else {
                warn!(
                    "Session {} (tab {}) requested clipboard while its tab is not active; denied",
                    session.session_id(),
                    session.tab_id().raw()
                );
                if let Err(e) = session.send_clipboard_content(None) {
                    warn!(
                        "Failed to send clipboard denial to session {}: {}",
                        session.session_id(),
                        e
                    );
                }
                denied = true;
            }
        }
        if denied {
            self.flush();
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
        let mut queued_writes = false;
        for (&key, session) in &mut self.sessions {
            match session.check_keepalive() {
                Ok(true) => {
                    queued_writes |= session.has_pending_writes();
                }
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
            self.remove_session(key);
        }
        if queued_writes {
            self.flush();
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

    /// Size every session to the window's grid; see [`Self::resize_sessions`].
    pub fn broadcast_resize(&mut self, width: u16, height: u16) {
        self.resize_sessions((width, height), |_| None);
    }
}

/// Accept only peers running as the same user as the terminal.
fn peer_is_authorized(stream: &UnixStream) -> bool {
    match peer_uid(stream) {
        Ok(uid) if uid == crate::platform::socket::current_uid() => true,
        Ok(uid) => {
            warn!(
                "Rejected native protocol connection from uid {} (server uid {})",
                uid,
                crate::platform::socket::current_uid()
            );
            false
        }
        Err(e) => {
            warn!(
                "Rejected native protocol connection: cannot read peer credentials: {}",
                e
            );
            false
        }
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
        for tab in self.listeners.values() {
            let _ = fs::remove_file(&tab.socket_path);
        }
        info!("Protocol server shut down");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    const TAB_A: TabId = TabId::new(0);
    const TAB_B: TabId = TabId::new(1);

    /// Per-test socket directory, removed (with anything left in it) on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            Self(crate::testing::short_socket_test_dir("srv"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Server with one open, active tab; returns that tab's socket path.
    fn server_with_tab(dir: &TempDir) -> (ProtocolServer, PathBuf) {
        let mut server = ProtocolServer::new(&dir.0).expect("failed to create server");
        let path = server.open_tab(TAB_A).expect("open tab");
        server.set_active_tab(Some(TAB_A));
        (server, path)
    }

    fn own_process_group() -> i32 {
        // SAFETY: getpgrp has no preconditions.
        unsafe { libc::getpgrp() }
    }

    fn connect(server: &mut ProtocolServer, path: &Path) -> UnixStream {
        let client = UnixStream::connect(path).expect("connect");
        server.poll(Some(Duration::from_millis(100))).unwrap();
        client
    }

    // ---------------------------------------------------------------
    // 1. Lifecycle: open_tab creates the socket, close_tab/drop remove it
    // ---------------------------------------------------------------
    #[test]
    fn open_tab_creates_socket_file_and_drop_removes_it() {
        let dir = TempDir::new();
        let (server, path) = server_with_tab(&dir);
        assert!(path.exists(), "socket file should exist after open_tab()");
        assert_eq!(server.tab_socket_path(TAB_A), Some(path.as_path()));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("ultty-{}-tab0.sock", std::process::id())
        );
        drop(server);
        assert!(!path.exists(), "socket file should be removed after drop");
    }

    #[test]
    fn socket_file_is_owner_only_and_listener_is_cloexec() {
        let dir = TempDir::new();
        let (server, path) = server_with_tab(&dir);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket must be private to the owner");
        let listener = server.listeners.values().next().unwrap();
        assert!(crate::platform::socket::is_cloexec(&listener.listener).unwrap());
    }

    #[test]
    fn open_tab_twice_is_an_error() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        let err = server.open_tab(TAB_A).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(server.open_tabs().count(), 1);
    }

    #[test]
    fn close_tab_unlinks_socket_and_disconnects_its_sessions() {
        let dir = TempDir::new();
        let (mut server, path_a) = server_with_tab(&dir);
        let path_b = server.open_tab(TAB_B).unwrap();

        let _a = connect(&mut server, &path_a);
        let mut b = connect(&mut server, &path_b);
        assert_eq!(server.session_count(), 2);
        // Set the timeout while the peer is still open: macOS rejects
        // SO_RCVTIMEO on a Unix socket whose peer has already gone away.
        b.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();

        server.close_tab(TAB_B);
        assert!(!path_b.exists(), "closed tab's socket is unlinked");
        assert!(path_a.exists(), "other tab's socket is untouched");
        assert_eq!(server.tab_socket_path(TAB_B), None);
        assert_eq!(server.open_tabs().collect::<Vec<_>>(), vec![TAB_A]);
        assert_eq!(server.session_count(), 1);
        assert!(server.sessions().all(|s| s.tab_id() == TAB_A));

        // The dropped server end shows up as EOF on the client.
        let mut buf = [0u8; 16];
        assert_eq!(std::io::Read::read(&mut b, &mut buf).unwrap(), 0);

        // Closing an unknown or already closed tab is a no-op.
        server.close_tab(TAB_B);
        server.close_tab(TabId::new(99));
        assert_eq!(server.session_count(), 1);
    }

    #[test]
    fn sessions_record_the_tab_that_accepted_them() {
        let dir = TempDir::new();
        let (mut server, path_a) = server_with_tab(&dir);
        let path_b = server.open_tab(TAB_B).unwrap();

        let _a = connect(&mut server, &path_a);
        let _b = connect(&mut server, &path_b);

        let mut tabs: Vec<TabId> = server.sessions().map(|s| s.tab_id()).collect();
        tabs.sort_by_key(|t| t.raw());
        assert_eq!(tabs, vec![TAB_A, TAB_B]);
    }

    #[test]
    fn active_tab_selects_sessions_and_marks_them_dirty() {
        let dir = TempDir::new();
        let (mut server, path_a) = server_with_tab(&dir);
        let path_b = server.open_tab(TAB_B).unwrap();
        let _a = connect(&mut server, &path_a);
        let _b = connect(&mut server, &path_b);
        for session in server.sessions_mut() {
            session.clear_render_dirty();
        }

        assert_eq!(server.active_tab(), Some(TAB_A));
        assert!(server.active_tab_sessions().all(|s| s.tab_id() == TAB_A));
        assert_eq!(server.active_tab_sessions().count(), 1);

        server.set_active_tab(Some(TAB_B));
        assert_eq!(server.active_tab_sessions().count(), 1);
        for session in server.sessions() {
            assert_eq!(
                session.is_render_dirty(),
                session.tab_id() == TAB_B,
                "only the newly visible tab's sessions need a rebuild"
            );
        }

        server.set_active_tab(None);
        assert_eq!(server.active_tab_sessions().count(), 0);
        assert!(!server.has_key_subscribers(Some(own_process_group())));
    }

    #[test]
    fn readiness_fds_include_listeners_and_sessions() {
        let dir = TempDir::new();
        let (mut server, path_a) = server_with_tab(&dir);
        let listener_fd = server
            .listeners
            .values()
            .next()
            .unwrap()
            .listener
            .as_raw_fd();
        assert_eq!(server.readiness_fds(), vec![listener_fd]);

        let _path_b = server.open_tab(TAB_B).unwrap();
        assert_eq!(server.readiness_fds().len(), 2);

        let _client = connect(&mut server, &path_a);
        let fds = server.readiness_fds();
        assert!(fds.contains(&listener_fd));
        assert_eq!(fds.len(), 3);
    }

    // ---------------------------------------------------------------
    // 2. set_viewport updates dimensions
    // ---------------------------------------------------------------
    #[test]
    fn set_viewport_updates_dimensions() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.set_viewport(120.0, 40.0);
        assert_eq!(server.viewport_width, 120.0);
        assert_eq!(server.viewport_height, 40.0);
    }

    // ---------------------------------------------------------------
    // 3. session_count starts at 0
    // ---------------------------------------------------------------
    #[test]
    fn session_count_starts_at_zero() {
        let dir = TempDir::new();
        let (server, _path) = server_with_tab(&dir);
        assert_eq!(server.session_count(), 0);
    }

    // ---------------------------------------------------------------
    // 4. poll with no connections returns Ok
    // ---------------------------------------------------------------
    #[test]
    fn poll_no_connections_returns_ok() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        let result = server.poll(Some(Duration::ZERO));
        assert!(result.is_ok(), "poll with no connections should succeed");
    }

    // ---------------------------------------------------------------
    // 5. broadcast_key with no sessions doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_key_no_sessions() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.broadcast_key(
            ultty_protocol::Key::Char('a'),
            ultty_protocol::Modifiers::NONE,
            Some(own_process_group()),
        );
    }

    // ---------------------------------------------------------------
    // 6. broadcast_mouse with no sessions doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_mouse_no_sessions() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.broadcast_mouse(
            TerminalId::new(0),
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
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.broadcast_resize(120, 40);
    }

    #[test]
    fn sessions_bind_to_the_pane_named_for_their_tab_and_peer() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let _client = connect(&mut server, &path);
        let pane = TerminalId::new(3);

        let mut asked = Vec::new();
        let bound = server.bind_unbound_sessions(|tab, peer| {
            asked.push((tab, peer));
            None
        });
        assert_eq!(bound, 0);
        assert_eq!(asked, vec![(TAB_A, Some(std::process::id() as i32))]);
        assert!(
            server.sessions().all(|s| s.pane().is_none()),
            "left unbound"
        );
        assert_eq!(server.pane_sessions(pane).count(), 0);

        assert_eq!(server.bind_unbound_sessions(|_, _| Some(pane)), 1);
        assert_eq!(server.pane_sessions(pane).count(), 1);
        // Bound once: the resolver is not asked again.
        assert_eq!(server.bind_unbound_sessions(|_, _| panic!("asked")), 0);
    }

    #[test]
    fn bound_sessions_are_sized_to_their_pane_and_dropped_with_it() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let mut client = connect(&mut server, &path);
        let pane = TerminalId::new(3);
        server.resize_sessions((100, 30), |_| None);
        assert_eq!(
            server.sessions().next().unwrap().viewport(),
            (100.0, 30.0),
            "an unbound session has the window's size"
        );

        server.bind_unbound_sessions(|_, _| Some(pane));
        server.resize_sessions((100, 30), |p| (p == pane).then_some((50, 30)));
        assert_eq!(server.sessions().next().unwrap().viewport(), (50.0, 30.0));
        // The app hears about its pane's size.
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = std::io::Read::read(&mut client, &mut buf).expect("a Resize frame");
        assert!(n > 0);

        server.retain_pane_sessions(|p| p != pane);
        assert_eq!(server.session_count(), 0, "a closed pane's sessions go");
    }

    // ---------------------------------------------------------------
    // 8. update_animations with no sessions returns false
    // ---------------------------------------------------------------
    #[test]
    fn update_animations_no_sessions() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        assert!(!server.update_animations());
    }

    // ---------------------------------------------------------------
    // 9. socket_dir returns the configured directory
    // ---------------------------------------------------------------
    #[test]
    fn socket_dir_returns_configured_directory() {
        let dir = TempDir::new();
        let (server, path) = server_with_tab(&dir);
        assert_eq!(server.socket_dir(), dir.0.as_path());
        assert_eq!(path.parent(), Some(dir.0.as_path()));
    }

    // ---------------------------------------------------------------
    // 10. Accepting a connection increments session_count
    // ---------------------------------------------------------------
    #[test]
    fn accept_connection_increments_session_count() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        assert_eq!(server.session_count(), 0);
        let _client = connect(&mut server, &path);
        assert_eq!(server.session_count(), 1);
    }

    // ---------------------------------------------------------------
    // 11. Multiple connections are tracked independently
    // ---------------------------------------------------------------
    #[test]
    fn multiple_connections_tracked() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);

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
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let _client = connect(&mut server, &path);

        assert_eq!(server.session_count(), 1);
        server.broadcast_key(
            ultty_protocol::Key::Enter,
            ultty_protocol::Modifiers::CTRL,
            Some(own_process_group()),
        );
    }

    // ---------------------------------------------------------------
    // 13. broadcast_mouse with a connected session doesn't panic
    // ---------------------------------------------------------------
    #[test]
    fn broadcast_mouse_with_session() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let _client = connect(&mut server, &path);

        server.broadcast_mouse(
            TerminalId::new(0),
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
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let _client = connect(&mut server, &path);

        server.broadcast_resize(200, 60);
    }

    // ---------------------------------------------------------------
    // 15. open_tab removes a stale socket file before binding
    // ---------------------------------------------------------------
    #[test]
    fn open_tab_removes_stale_socket() {
        let dir = TempDir::new();
        fs::create_dir_all(&dir.0).unwrap();
        let stale = dir.0.join(tab_socket_file_name(TAB_A));
        fs::write(&stale, b"stale").expect("create stale socket placeholder");

        let mut server = ProtocolServer::new(&dir.0).unwrap();
        let path = server
            .open_tab(TAB_A)
            .expect("should be able to bind on a path with a stale socket");
        assert_eq!(path, stale);
    }

    #[test]
    fn open_tab_does_not_remove_active_socket() {
        let dir = TempDir::new();
        let (_server, _path) = server_with_tab(&dir);

        // A second server (as another ultty instance with the same pid
        // would be impossible, but a stale-path check must still hold).
        let mut other = ProtocolServer::new(&dir.0).unwrap();
        let err = match other.open_tab(TAB_A) {
            Ok(_) => panic!("active socket should not be unlinked"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    // ---------------------------------------------------------------
    // 16. set_viewport propagates to existing sessions
    // ---------------------------------------------------------------
    #[test]
    fn set_viewport_propagates_to_sessions() {
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let _client = connect(&mut server, &path);

        server.set_viewport(200.0, 50.0);

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
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        assert_eq!(server.sessions().count(), 0);

        let _c1 = connect(&mut server, &path);

        assert_eq!(server.sessions().count(), 1);
    }

    // ---------------------------------------------------------------
    // In-band sessions
    // ---------------------------------------------------------------

    fn hello_bytes() -> Vec<u8> {
        use ultty_protocol::Encode;
        AppMessage::ClientHello {
            version: ultty_protocol::PROTOCOL_VERSION,
            requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
        }
        .encode_to_bytes()
        .to_vec()
    }

    fn encode(msg: &AppMessage) -> Vec<u8> {
        use ultty_protocol::Encode;
        msg.encode_to_bytes().to_vec()
    }

    fn decode_all(mut bytes: &[u8]) -> Vec<ultty_protocol::TerminalMessage> {
        use ultty_protocol::Decode;
        let mut out = Vec::new();
        while !bytes.is_empty() {
            out.push(ultty_protocol::TerminalMessage::decode(&mut bytes).expect("decode"));
        }
        out
    }

    fn in_band_output(
        server: &mut ProtocolServer,
        tab: TabId,
    ) -> Vec<ultty_protocol::TerminalMessage> {
        server
            .take_in_band_output()
            .into_iter()
            .filter(|(t, _)| *t == tab)
            .flat_map(|(_, bytes)| decode_all(&bytes))
            .collect()
    }

    use ultty_protocol::{AppMessage, TerminalMessage};

    #[test]
    fn in_band_hello_split_across_calls_opens_a_session() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        let hello = hello_bytes();
        server.in_band_input(TAB_A, Some(42), Vec::new, &hello[..5], false);
        assert_eq!(server.session_count(), 0);
        server.in_band_input(TAB_A, Some(42), Vec::new, &hello[5..], false);
        assert_eq!(server.session_count(), 1);
        assert_eq!(server.in_band_tabs(), vec![TAB_A]);

        let replies = in_band_output(&mut server, TAB_A);
        assert!(matches!(
            replies.first(),
            Some(TerminalMessage::ServerHello { .. })
        ));
        assert!(matches!(
            replies.get(1),
            Some(TerminalMessage::SessionReady { .. })
        ));
        assert!(server.sessions().next().unwrap().stream_fd().is_none());
    }

    #[test]
    fn in_band_data_that_is_not_a_hello_opens_nothing() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        let begin = encode(&AppMessage::BeginFrame {
            frame_id: ultty_protocol::FrameId(1),
        });
        server.in_band_input(TAB_A, Some(42), Vec::new, &begin, false);
        server.in_band_input(TAB_A, Some(42), Vec::new, b"garbage", false);
        assert_eq!(server.session_count(), 0);
        // A real hello afterwards still works.
        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        assert_eq!(server.session_count(), 1);
    }

    #[test]
    fn in_band_session_ends_on_disconnect_broken_stream_or_new_foreground() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);

        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        server.in_band_input(
            TAB_A,
            Some(42),
            Vec::new,
            &encode(&AppMessage::Disconnect),
            false,
        );
        assert_eq!(server.session_count(), 0, "disconnect");

        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        server.in_band_input(TAB_A, Some(42), Vec::new, &[], true);
        assert_eq!(server.session_count(), 0, "broken stream");

        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        server.end_in_band_if_app_gone(TAB_A, Some(42));
        assert_eq!(server.session_count(), 1, "same foreground keeps it");
        server.end_in_band_if_app_gone(TAB_A, Some(7));
        assert_eq!(server.session_count(), 0, "new foreground");
    }

    #[test]
    fn in_band_sessions_are_one_per_tab_and_stay_on_their_tab() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.open_tab(TAB_B).unwrap();
        server.in_band_input(TAB_A, Some(1), Vec::new, &hello_bytes(), false);
        server.in_band_input(TAB_B, Some(2), Vec::new, &hello_bytes(), false);
        assert_eq!(server.session_count(), 2);
        let mut tabs = server.in_band_tabs();
        tabs.sort_by_key(|t| t.raw());
        assert_eq!(tabs, vec![TAB_A, TAB_B]);
        server.close_tab(TAB_B);
        assert_eq!(server.in_band_tabs(), vec![TAB_A]);
    }

    #[test]
    fn in_band_sessions_may_not_read_the_clipboard() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        let _ = in_band_output(&mut server, TAB_A);

        server.in_band_input(
            TAB_A,
            Some(42),
            Vec::new,
            &encode(&AppMessage::ClipboardGet),
            false,
        );
        assert!(server.pending_clipboard_gets().is_empty());
        let replies = in_band_output(&mut server, TAB_A);
        assert!(
            replies
                .iter()
                .any(|m| matches!(m, TerminalMessage::ClipboardContent { text: None })),
            "denial sent: {replies:?}"
        );
    }

    /// Read every message the server has sent `client` so far.
    fn socket_replies(client: &mut UnixStream) -> Vec<TerminalMessage> {
        use std::io::Read;
        client.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match client.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("read: {e}"),
            }
        }
        decode_all(&bytes)
    }

    fn key_downs(messages: &[TerminalMessage]) -> usize {
        messages
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    TerminalMessage::Input(ultty_protocol::InputEvent {
                        event_type: ultty_protocol::InputType::KeyDown { .. },
                        ..
                    })
                )
            })
            .count()
    }

    #[test]
    fn keys_reach_only_a_socket_session_in_the_foreground() {
        use std::io::Write;
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let mut client = connect(&mut server, &path);
        for msg in [
            AppMessage::ClientHello {
                version: ultty_protocol::PROTOCOL_VERSION,
                requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
            },
            root_child(),
            AppMessage::Subscribe {
                id: ultty_protocol::RegionId::ROOT,
                events: ultty_protocol::EVENT_KEY,
            },
        ] {
            client.write_all(&encode(&msg)).unwrap();
        }
        server.poll(Some(Duration::from_millis(100))).unwrap();
        let _handshake = socket_replies(&mut client);

        // The test process connected, so its group is the peer's group.
        let foreground = Some(own_process_group());
        let background = Some(own_process_group() + 1);
        let key = ultty_protocol::Key::Char('p');
        let mods = ultty_protocol::Modifiers::NONE;

        assert!(!server.has_key_subscribers(background));
        server.broadcast_key(key.clone(), mods, background);
        server.broadcast_text_input("hunter2".to_string(), background);
        server.broadcast_key(key.clone(), mods, None);
        assert!(
            socket_replies(&mut client).is_empty(),
            "a background session must receive no input"
        );

        assert!(server.has_key_subscribers(foreground));
        server.broadcast_key(key, mods, foreground);
        assert_eq!(key_downs(&socket_replies(&mut client)), 1);
    }

    /// A region under ROOT, which makes a session full-scene.
    fn root_child() -> AppMessage {
        AppMessage::CreateRegion {
            id: ultty_protocol::RegionId::new(1),
            parent: ultty_protocol::RegionId::ROOT,
            insert_index: -1,
            constraints: ultty_protocol::Constraints::default(),
            style: ultty_protocol::Style::default(),
        }
    }

    #[test]
    fn mouse_reaches_only_the_sessions_of_the_pane_under_the_pointer() {
        use std::io::Write;
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let mut clients: Vec<UnixStream> = (0..2).map(|_| connect(&mut server, &path)).collect();
        for client in &mut clients {
            for msg in [
                AppMessage::ClientHello {
                    version: ultty_protocol::PROTOCOL_VERSION,
                    requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
                },
                root_child(),
                AppMessage::Subscribe {
                    id: ultty_protocol::RegionId::ROOT,
                    events: ultty_protocol::EVENT_MOUSE,
                },
            ] {
                client.write_all(&encode(&msg)).unwrap();
            }
        }
        server.poll(Some(Duration::from_millis(100))).unwrap();
        for client in &mut clients {
            let _handshake = socket_replies(client);
        }
        // The first client's session (the lower id: accepted first) is in
        // the left pane, the second's in the right.
        let (left, right) = (TerminalId::new(1), TerminalId::new(2));
        let mut ids: Vec<u64> = server.sessions().map(|s| s.session_id()).collect();
        ids.sort_unstable();
        for session in server.sessions_mut() {
            session.bind_pane(if session.session_id() == ids[0] {
                left
            } else {
                right
            });
        }

        server.broadcast_mouse(
            right,
            ultty_protocol::InputType::MouseDown {
                button: ultty_protocol::MouseButton::Left,
            },
            3.0,
            2.0,
            ultty_protocol::Modifiers::NONE,
        );
        assert!(
            socket_replies(&mut clients[0]).is_empty(),
            "the left pane's session sees nothing"
        );
        let inputs: Vec<(f32, f32)> = socket_replies(&mut clients[1])
            .into_iter()
            .filter_map(|m| match m {
                TerminalMessage::Input(input) => Some((input.x, input.y)),
                _ => None,
            })
            .collect();
        assert_eq!(
            inputs,
            vec![(3.0, 2.0)],
            "pane-local cells, region-local on ROOT"
        );
    }

    /// An app with only inline surfaces reads keys from its tty, even if it
    /// subscribed ROOT to them; once it draws a full scene it gets them.
    #[test]
    fn keys_reach_a_session_only_while_it_is_full_scene() {
        use std::io::Write;
        let dir = TempDir::new();
        let (mut server, path) = server_with_tab(&dir);
        let mut client = connect(&mut server, &path);
        for msg in [
            AppMessage::ClientHello {
                version: ultty_protocol::PROTOCOL_VERSION,
                requested_capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
            },
            AppMessage::Subscribe {
                id: ultty_protocol::RegionId::ROOT,
                events: ultty_protocol::EVENT_KEY,
            },
        ] {
            client.write_all(&encode(&msg)).unwrap();
        }
        server.poll(Some(Duration::from_millis(100))).unwrap();
        let _handshake = socket_replies(&mut client);
        let foreground = Some(own_process_group());
        let key = ultty_protocol::Key::Char('k');
        let mods = ultty_protocol::Modifiers::NONE;

        assert!(!server.has_key_subscribers(foreground));
        server.broadcast_key(key.clone(), mods, foreground);
        server.broadcast_text_input("typed".to_string(), foreground);
        assert!(socket_replies(&mut client).is_empty());

        client.write_all(&encode(&root_child())).unwrap();
        server.poll(Some(Duration::from_millis(100))).unwrap();
        let _ = socket_replies(&mut client);
        assert!(server.has_key_subscribers(foreground));
        server.broadcast_key(key, mods, foreground);
        assert_eq!(key_downs(&socket_replies(&mut client)), 1);
    }

    /// What a `cat`ed file holding a hello looks like: the hello and more
    /// frames in one burst, and nothing after.
    fn stray_hello_burst() -> Vec<u8> {
        let mut burst = hello_bytes();
        for msg in [
            AppMessage::CreateRegion {
                id: ultty_protocol::RegionId::new(1),
                parent: ultty_protocol::RegionId::ROOT,
                insert_index: -1,
                constraints: ultty_protocol::Constraints::default(),
                style: ultty_protocol::Style::default(),
            },
            AppMessage::Subscribe {
                id: ultty_protocol::RegionId::ROOT,
                events: ultty_protocol::EVENT_KEY,
            },
            // Draws an error reply.
            AppMessage::SetContent {
                id: ultty_protocol::RegionId::new(99),
                content: ultty_protocol::Content::None,
            },
        ] {
            burst.extend(encode(&msg));
        }
        burst
    }

    #[test]
    fn unconfirmed_in_band_session_is_not_shown_and_gets_no_keys() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.in_band_input(TAB_A, Some(42), Vec::new, &stray_hello_burst(), false);
        assert_eq!(server.session_count(), 1);

        // Only the hello answer goes back; the probe needs it.
        let replies = in_band_output(&mut server, TAB_A);
        assert!(
            matches!(
                replies.as_slice(),
                [
                    TerminalMessage::ServerHello { .. },
                    TerminalMessage::SessionReady { .. }
                ]
            ),
            "{replies:?}"
        );

        assert_eq!(server.active_tab_sessions().count(), 0, "not drawn");
        assert!(!server.has_key_subscribers(Some(42)));
        server.broadcast_key(
            ultty_protocol::Key::Char('x'),
            ultty_protocol::Modifiers::NONE,
            Some(42),
        );
        server.broadcast_resize(100, 30);
        assert!(
            in_band_output(&mut server, TAB_A).is_empty(),
            "nothing is typed into the shell for it"
        );
    }

    #[test]
    fn in_band_session_confirmed_by_later_data_is_shown_and_gets_held_replies() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.in_band_input(TAB_A, Some(42), Vec::new, &stray_hello_burst(), false);
        let _hello_answer = in_band_output(&mut server, TAB_A);

        server.in_band_input(TAB_A, Some(42), Vec::new, &encode(&AppMessage::Pong), false);

        assert_eq!(server.active_tab_sessions().count(), 1);
        assert!(server.has_key_subscribers(Some(42)));
        let held = in_band_output(&mut server, TAB_A);
        assert!(
            held.iter()
                .any(|m| matches!(m, TerminalMessage::Error { .. })),
            "replies held while unconfirmed are sent: {held:?}"
        );
    }

    #[test]
    fn in_band_sessions_receive_keys_whatever_the_foreground() {
        let dir = TempDir::new();
        let (mut server, _path) = server_with_tab(&dir);
        server.in_band_input(TAB_A, Some(42), Vec::new, &hello_bytes(), false);
        let mut later = encode(&root_child());
        later.extend(encode(&AppMessage::Subscribe {
            id: ultty_protocol::RegionId::ROOT,
            events: ultty_protocol::EVENT_KEY,
        }));
        server.in_band_input(TAB_A, Some(42), Vec::new, &later, false);
        let _ = in_band_output(&mut server, TAB_A);

        assert!(server.has_key_subscribers(Some(42)));
        server.broadcast_key(
            ultty_protocol::Key::Enter,
            ultty_protocol::Modifiers::NONE,
            Some(42),
        );
        assert_eq!(key_downs(&in_band_output(&mut server, TAB_A)), 1);
    }
}
