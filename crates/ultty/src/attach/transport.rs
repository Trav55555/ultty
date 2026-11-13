//! Local-only attachment transport and blocking worker client.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout};
use std::time::{Duration, Instant};

use ultty_protocol::control::OwnerRef;

use super::auth::ClientId;
use super::wire::{self, Capability, Command, Reply, Request, Response, Token};
use crate::platform::socket::{current_uid, ensure_private_dir, peer_uid, private_socket_dir};

/// Whole local request/reply deadline.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum concurrent attachments.
pub const MAX_CLIENTS: usize = 32;
/// Retained outbound frame bytes per client; slow clients disconnect.
pub const MAX_OUTGOING_BYTES: usize = 32 * 1024 * 1024;
/// Global queued response budget across all viewers.
pub const MAX_GLOBAL_OUTGOING_BYTES: usize = 64 * 1024 * 1024;
const IO_BUDGET: usize = 64 * 1024;

/// Decoded operations and disconnects observed during one nonblocking pass.
#[derive(Default)]
pub struct Events {
    /// Requests in this pass, bounded to eight per client.
    pub requests: Vec<(ClientId, Request)>,
    /// Lost clients; revoke their lease before dispatching queued requests.
    pub disconnected: Vec<ClientId>,
}
struct Peer {
    stream: UnixStream,
    input: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    written: usize,
    queued: usize,
    created: Instant,
    partial_since: Option<Instant>,
    first_received: bool,
    last_progress: Instant,
}
impl Peer {
    fn read_requests(&mut self, now: Instant) -> io::Result<Vec<Request>> {
        if (!self.first_received && now.duration_since(self.created) >= DEFAULT_TIMEOUT)
            || self
                .partial_since
                .is_some_and(|start| now.duration_since(start) >= DEFAULT_TIMEOUT)
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "attachment frame deadline elapsed",
            ));
        }
        let mut chunk = [0u8; IO_BUDGET];
        let room = (wire::MAX_REQUEST_BYTES + 8).saturating_sub(self.input.len());
        if room != 0 {
            match self.stream.read(&mut chunk[..room.min(IO_BUDGET)]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(count) => {
                    if self.input.is_empty() {
                        self.partial_since = Some(now);
                    }
                    self.input.extend_from_slice(&chunk[..count]);
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        let mut requests = Vec::new();
        while requests.len() < 8 && self.input.len() >= 8 {
            let mut header = [0; 8];
            header.copy_from_slice(&self.input[..8]);
            let length = wire::body_length(&header, wire::MAX_REQUEST_BYTES)?;
            if self.input.len() < length + 8 {
                break;
            }
            let request = serde_json::from_slice(&self.input[8..length + 8])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            requests.push(request);
            self.input.drain(..length + 8);
            self.first_received = true;
            self.partial_since = (!self.input.is_empty()).then_some(now);
        }
        Ok(requests)
    }
    fn flush(&mut self, now: Instant) -> io::Result<()> {
        if !self.output.is_empty() && now.duration_since(self.last_progress) >= DEFAULT_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "viewer stopped consuming responses",
            ));
        }
        let mut budget = IO_BUDGET;
        while budget != 0 {
            let Some(frame) = self.output.front() else {
                break;
            };
            let end = frame.len().min(self.written + budget);
            match self.stream.write(&frame[self.written..end]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    self.last_progress = now;
                    self.written += count;
                    budget -= count;
                    if self.written == frame.len() {
                        self.queued -= frame.len();
                        self.output.pop_front();
                        self.written = 0;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Sole named local listener. A held OS lock makes stale-path cleanup a single
/// writer operation; viewer loss never owns or closes runtime children.
pub struct Server {
    listener: UnixListener,
    endpoint: PathBuf,
    admin_path: PathBuf,
    _lock: File,
    wrote_admin: bool,
    peers: HashMap<ClientId, Peer>,
    next_client: Option<ClientId>,
    disconnected: Vec<ClientId>,
}
impl Server {
    /// Bind one validated private named session, rejecting an existing owner.
    pub fn bind(session: &str) -> io::Result<Self> {
        Self::bind_at(&session_directory()?, session)
    }
    /// Bind in an explicit private fixture directory.
    pub fn bind_at(directory: &Path, session: &str) -> io::Result<Self> {
        ensure_private_dir(directory)?;
        let endpoint = endpoint_in(directory, session)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(endpoint.with_extension("lock"))?;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != current_uid()
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe session lock",
            ));
        }
        // SAFETY: lock owns this live fd throughout the server lifetime. flock
        // neither transfers nor closes it; its kernel lease ends on Drop.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "session owner already running",
            ));
        }
        match fs::symlink_metadata(&endpoint) {
            Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == current_uid() => {
                if UnixStream::connect(&endpoint).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "live session endpoint exists",
                    ));
                }
                fs::remove_file(&endpoint)?;
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unsafe session endpoint",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&endpoint)?;
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            admin_path: endpoint.with_extension("admin"),
            listener,
            endpoint,
            _lock: lock,
            wrote_admin: false,
            peers: HashMap::new(),
            next_client: Some(1),
            disconnected: Vec::new(),
        })
    }
    /// Bound endpoint for discovery and fixtures.
    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }
    /// Explicit local owner token path.
    pub fn admin_path(&self) -> &Path {
        &self.admin_path
    }
    /// Publish the local owner credential before announcing host readiness.
    pub fn write_admin_token(&mut self, token: &Token) -> io::Result<()> {
        let temporary = self
            .admin_path
            .with_extension(format!("admin.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(token.expose_secret().as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &self.admin_path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.wrote_admin = true;
        Ok(())
    }
    /// Whether a queued request's connection is still present.
    pub fn is_connected(&self, client: ClientId) -> bool {
        self.peers.contains_key(&client)
    }
    /// Disconnect independently; the caller must also drop its authority lease.
    pub fn disconnect(&mut self, client: ClientId) {
        if self.peers.remove(&client).is_some() {
            self.disconnected.push(client);
        }
    }
    /// Nonblocking bounded accept/read/write pass; never waits for a viewer.
    pub fn poll(&mut self, now: Instant) -> io::Result<Events> {
        for _ in 0..MAX_CLIENTS {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if self.peers.len() >= MAX_CLIENTS || peer_uid(&stream)? != current_uid() {
                continue;
            }
            let Some(client) = self.next_client else {
                continue;
            };
            self.next_client = client.checked_add(1);
            stream.set_nonblocking(true)?;
            self.peers.insert(
                client,
                Peer {
                    stream,
                    input: Vec::new(),
                    output: VecDeque::new(),
                    written: 0,
                    queued: 0,
                    created: now,
                    partial_since: None,
                    first_received: false,
                    last_progress: now,
                },
            );
        }
        let mut events = Events::default();
        let clients: Vec<_> = self.peers.keys().copied().collect();
        for client in clients {
            let result = self.peers.get_mut(&client).map(|peer| {
                let requests = peer.read_requests(now)?;
                peer.flush(now)?;
                Ok::<_, io::Error>(requests)
            });
            match result {
                Some(Ok(requests)) => events
                    .requests
                    .extend(requests.into_iter().map(|request| (client, request))),
                Some(Err(_)) => self.disconnect(client),
                None => {}
            }
        }
        events.disconnected = std::mem::take(&mut self.disconnected);
        Ok(events)
    }
    /// Queue a bounded response. False means the peer was dropped; release its
    /// authority before considering more already-decoded requests from it.
    pub fn send(&mut self, client: ClientId, response: &Response) -> io::Result<bool> {
        let frame = wire::encode(response, wire::MAX_RESPONSE_BYTES)?;
        let total: usize = self.peers.values().map(|peer| peer.queued).sum();
        if frame.len() > MAX_GLOBAL_OUTGOING_BYTES.saturating_sub(total) {
            self.disconnect(client);
            return Ok(false);
        }
        let Some(peer) = self.peers.get_mut(&client) else {
            return Ok(false);
        };
        if frame.len() > MAX_OUTGOING_BYTES.saturating_sub(peer.queued) {
            self.disconnect(client);
            return Ok(false);
        }
        if peer.output.is_empty() {
            peer.last_progress = Instant::now();
        }
        peer.queued += frame.len();
        peer.output.push_back(frame);
        if peer.flush(Instant::now()).is_err() {
            self.disconnect(client);
            return Ok(false);
        }
        Ok(true)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.endpoint);
        if self.wrote_admin {
            let _ = fs::remove_file(&self.admin_path);
        }
    }
}

/// Resolve a private endpoint from a validated session name.
pub fn session_endpoint(session: &str) -> io::Result<PathBuf> {
    endpoint_in(&session_directory()?, session)
}
/// Explicit local owner credential path; SSH never reads it.
pub fn admin_token_path(session: &str) -> io::Result<PathBuf> {
    Ok(session_endpoint(session)?.with_extension("admin"))
}
/// Read an explicitly selected local admin credential, never an SSH fallback.
pub fn read_admin_token(session: &str) -> io::Result<Token> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(admin_token_path(session)?)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe admin credential file",
        ));
    }
    let mut value = String::new();
    file.take(129).read_to_string(&mut value)?;
    if value.len() > 128 || value.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid admin credential file",
        ));
    }
    Ok(Token::new(value.trim()))
}
fn session_directory() -> io::Result<PathBuf> {
    let directory = private_socket_dir(&std::env::temp_dir())?;
    ensure_private_dir(&directory)?;
    Ok(directory)
}
fn endpoint_in(directory: &Path, session: &str) -> io::Result<PathBuf> {
    if !wire::valid_session_name(session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session name",
        ));
    }
    Ok(directory.join(format!("ultty-session-{session}.sock")))
}

enum Connection {
    Local(UnixStream),
    Stdio {
        reader: ChildStdout,
        writer: ChildStdin,
        child: Child,
    },
}
impl Connection {
    fn exchange(&mut self, request: &Request) -> io::Result<Response> {
        let frame = wire::encode(request, wire::MAX_REQUEST_BYTES)?;
        self.exchange_frame(&frame, matches!(request.command, Command::Hello { .. }))
    }
    fn exchange_frame(&mut self, frame: &[u8], hello: bool) -> io::Result<Response> {
        match self {
            Self::Local(stream) => {
                let mut timed = DeadlineStream {
                    stream,
                    deadline: Instant::now() + DEFAULT_TIMEOUT,
                };
                timed.write_all(frame)?;
                timed.flush()?;
                wire::read(&mut timed, wire::MAX_RESPONSE_BYTES)
            }
            Self::Stdio { reader, writer, .. } => {
                let timeout = if hello {
                    Duration::from_secs(30)
                } else {
                    DEFAULT_TIMEOUT
                };
                let deadline = Instant::now() + timeout;
                let mut output = DeadlinePipe {
                    stream: writer,
                    deadline,
                };
                output.write_all(frame)?;
                output.flush()?;
                wire::read(
                    &mut DeadlinePipe {
                        stream: reader,
                        deadline,
                    },
                    wire::MAX_RESPONSE_BYTES,
                )
            }
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Self::Stdio { child, .. } = self {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}
struct DeadlineStream<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}
fn remaining_until(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "attachment request deadline elapsed",
            )
        })
}
pub(crate) fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: the caller retains the owned pipe descriptor; fcntl neither closes
    // nor transfers it, and F_SETFL preserves all existing flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same live owned descriptor and the valid flags read above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
/// Forward one exact request locally with a whole-operation deadline.
pub(crate) fn exchange_local(stream: &mut UnixStream, request: &Request) -> io::Result<Response> {
    let mut timed = DeadlineStream {
        stream,
        deadline: Instant::now() + DEFAULT_TIMEOUT,
    };
    wire::write(&mut timed, request, wire::MAX_REQUEST_BYTES)?;
    wire::read(&mut timed, wire::MAX_RESPONSE_BYTES)
}
fn wait_pipe(fd: RawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let millis = remaining_until(deadline)?
            .as_millis()
            .clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: descriptor is one initialized pollfd; its fd remains owned by
        // the caller throughout this synchronous wait.
        let ready = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if ready > 0 {
            return Ok(());
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "SSH pipe deadline elapsed",
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
struct DeadlinePipe<'a, T> {
    stream: &'a mut T,
    deadline: Instant,
}
impl<T: Read + AsRawFd> Read for DeadlinePipe<'_, T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            remaining_until(self.deadline)?;
            match self.stream.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_pipe(self.stream.as_raw_fd(), libc::POLLIN, self.deadline)?
                }
                result => return result,
            }
        }
    }
}
impl<T: Write + AsRawFd> Write for DeadlinePipe<'_, T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            remaining_until(self.deadline)?;
            match self.stream.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_pipe(self.stream.as_raw_fd(), libc::POLLOUT, self.deadline)?
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
impl DeadlineStream<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "attachment request deadline elapsed",
                )
            })
    }
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Blocking worker-thread client. Pins owner identity and never automatically
/// retries a mutation whose outcome is uncertain.
pub struct AttachmentClient {
    connection: Connection,
    owner: OwnerRef,
    grant_id: u64,
    capabilities: Vec<Capability>,
    admin: bool,
    controller: bool,
    mutation_id: u64,
    read_id: u64,
    pending: Option<Request>,
}
impl AttachmentClient {
    /// Authenticate to one named local session using an explicit credential.
    pub fn connect(session: &str, token: &str, controller: bool) -> io::Result<Self> {
        Self::connect_at(&session_endpoint(session)?, session, token, controller)
    }
    /// Connect to an explicit local endpoint for isolated fixtures.
    pub fn connect_at(
        endpoint: &Path,
        session: &str,
        token: &str,
        controller: bool,
    ) -> io::Result<Self> {
        Self::handshake(
            Connection::Local(UnixStream::connect(endpoint)?),
            session,
            token,
            controller,
        )
    }
    /// Authenticate over a separately spawned SSH child with piped streams.
    /// Only scoped credentials are accepted; local admin credentials are refused.
    pub fn from_ssh_child(
        mut child: Child,
        session: &str,
        token: &str,
        controller: bool,
    ) -> io::Result<Self> {
        let result = (|| {
            if token.starts_with("0.") {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "SSH requires a scoped token",
                ));
            }
            let reader = child.stdout.take().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "SSH stdout must be piped")
            })?;
            let writer = child.stdin.take().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "SSH stdin must be piped")
            })?;
            set_nonblocking(reader.as_raw_fd())?;
            set_nonblocking(writer.as_raw_fd())?;
            Ok((reader, writer))
        })();
        let (reader, writer) = match result {
            Ok(parts) => parts,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Self::handshake(
            Connection::Stdio {
                reader,
                writer,
                child,
            },
            session,
            token,
            controller,
        )
    }
    fn handshake(
        mut connection: Connection,
        session: &str,
        token: &str,
        controller: bool,
    ) -> io::Result<Self> {
        let command = Command::Hello {
            version: wire::VERSION,
            session: session.into(),
            token: Token::new(token),
            controller,
        };
        command.validate()?;
        let response = connection.exchange(&Request {
            id: 0,
            owner: None,
            grant_id: None,
            command,
        })?;
        if response.id != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "uncorrelated attachment hello",
            ));
        }
        let Reply::Welcome {
            grant_id,
            version: wire::VERSION,
            capabilities,
            admin,
            controller,
        } = response.reply
        else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("attachment hello refused: {:?}", response.reply),
            ));
        };
        Ok(Self {
            connection,
            owner: response.owner,
            grant_id,
            capabilities,
            admin,
            controller,
            mutation_id: response.last_mutation_id,
            read_id: 0,
            pending: None,
        })
    }
    /// Exact owner lifetime.
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    /// Grant lifetime bound to this connection and its operation IDs.
    pub fn grant_id(&self) -> u64 {
        self.grant_id
    }
    /// Authority returned by the authenticated owner.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
    /// Whether the explicit local owner credential was used.
    pub fn is_admin(&self) -> bool {
        self.admin
    }
    /// Whether this connection holds the controller lease.
    pub fn controller(&self) -> bool {
        self.controller
    }
    /// Highest acknowledged/reserved mutation sequence.
    pub fn last_mutation_id(&self) -> u64 {
        self.mutation_id
    }
    /// Uncertain operation retained for explicit reconciliation.
    pub fn pending_request(&self) -> Option<&Request> {
        self.pending.as_ref()
    }
    /// Execute once. Transport errors never cause automatic replay.
    pub fn request(&mut self, command: Command) -> io::Result<Response> {
        if self.pending.is_some() {
            return Err(io::Error::other(
                "previous mutation outcome unknown; reconnect and explicitly reconcile",
            ));
        }
        if matches!(command, Command::Hello { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Hello already completed",
            ));
        }
        command.validate()?;
        let id = if command.is_mutation() {
            self.mutation_id.checked_add(1)
        } else {
            self.read_id = self
                .read_id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("read IDs exhausted"))?;
            Some(self.read_id)
        }
        .ok_or_else(|| io::Error::other("mutation IDs exhausted"))?;
        self.send_exact(Request {
            id,
            owner: Some(self.owner.clone()),
            grant_id: Some(self.grant_id),
            command,
        })
    }
    /// Reconcile an exact previously issued mutation after explicit reconnect.
    /// An unknown/evicted receipt never executes the operation again.
    pub fn retry_exact(&mut self, request: Request) -> io::Result<Response> {
        if request.owner.as_ref() != Some(&self.owner)
            || request.grant_id != Some(self.grant_id)
            || !request.command.is_mutation()
            || request.id > self.mutation_id
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "retry must retain exact owner and mutation identity",
            ));
        }
        self.send_exact(request)
    }
    fn send_exact(&mut self, request: Request) -> io::Result<Response> {
        // Encoding failure is provably not sent and must not reserve an
        // uncertain operation. Byte arrays may expand substantially in JSON.
        let frame = wire::encode(&request, wire::MAX_REQUEST_BYTES)?;
        let new_mutation = request.command.is_mutation() && request.id > self.mutation_id;
        if request.command.is_mutation() {
            self.pending = Some(request.clone());
        }
        let response = self.connection.exchange_frame(&frame, false)?;
        if response.id != request.id || response.owner != self.owner {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attachment reply owner/correlation mismatch",
            ));
        }
        self.mutation_id = self.mutation_id.max(response.last_mutation_id);
        self.pending = None;
        if new_mutation && !matches!(response.reply, Reply::Error { .. }) {
            if matches!(request.command, Command::ClaimControl) {
                self.controller = true;
            }
            if matches!(request.command, Command::ReleaseControl) {
                self.controller = false;
            }
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::super::auth::{new_owner, Authority, Decision};
    use super::super::wire::ErrorCode;
    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }
    use super::*;
    use std::process::Command as ProcessCommand;
    use std::thread;

    #[test]
    fn test_client_socket_roundtrip_dedup_and_known_unsent_overflow() {
        let directory = private_directory();
        let mut server = Server::bind_at(directory.path(), "main").unwrap();
        assert!(Server::bind_at(directory.path(), "main").is_err());
        let mut authority = Authority::new("main", new_owner().unwrap()).unwrap();
        let token = authority.admin_token().expose_secret().to_string();
        let endpoint = server.endpoint().to_owned();
        let worker = thread::spawn(move || {
            let mut client = AttachmentClient::connect_at(&endpoint, "main", &token, true).unwrap();
            let command = Command::Spawn {
                program: "/bin/true".into(),
                args: vec![],
                cwd: None,
            };
            let result = client.request(command.clone()).unwrap();
            assert!(matches!(result.reply, Reply::Applied { pane: Some(7), .. }));
            let replay = Request {
                id: 1,
                owner: Some(client.owner().clone()),
                grant_id: Some(client.grant_id()),
                command,
            };
            assert!(matches!(
                client.retry_exact(replay).unwrap().reply,
                Reply::Applied { pane: Some(7), .. }
            ));
            let invalid = Command::Input {
                pane: 7,
                data: vec![255; 400_000],
            };
            assert!(client.request(invalid).is_err());
            assert!(
                client.pending_request().is_none(),
                "encoding failure is provably not sent"
            );
            assert_eq!(client.last_mutation_id(), 1);
            client
                .request(Command::Input {
                    pane: 7,
                    data: vec![1],
                })
                .unwrap();
            assert_eq!(client.last_mutation_id(), 2);
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut mutations = 0;
        while !worker.is_finished() {
            assert!(Instant::now() < deadline);
            let events = server.poll(Instant::now()).unwrap();
            for client in events.disconnected {
                authority.disconnect(client);
            }
            for (client, request) in events.requests {
                let response = match authority.process(client, &request, Instant::now()) {
                    Decision::Reply(response) => response,
                    Decision::Execute { .. } => {
                        mutations += 1;
                        authority.complete(
                            client,
                            &request,
                            Reply::Applied {
                                pane: Some(7),
                                tab: None,
                                warning: None,
                            },
                            Instant::now(),
                        )
                    }
                };
                assert!(server.send(client, &response).unwrap());
            }
            thread::sleep(Duration::from_millis(1));
        }
        worker.join().unwrap();
        assert_eq!(mutations, 2, "receipt replay must not launch again");
    }

    #[test]
    fn test_unread_peers_global_bound_stall_and_independent_response() {
        let directory = private_directory();
        let mut server = Server::bind_at(directory.path(), "main").unwrap();
        let mut readers = Vec::new();
        let hello = Request {
            id: 0,
            owner: None,
            grant_id: None,
            command: Command::Hello {
                version: wire::VERSION,
                session: "main".into(),
                token: Token::new("1.fixture"),
                controller: false,
            },
        };
        let hello_frame = wire::encode(&hello, wire::MAX_REQUEST_BYTES).unwrap();
        for _ in 0..3 {
            let mut stream = UnixStream::connect(server.endpoint()).unwrap();
            stream.write_all(&hello_frame).unwrap();
            readers.push(stream);
        }
        server.poll(Instant::now()).unwrap();
        let mut clients: Vec<_> = server.peers.keys().copied().collect();
        clients.sort_unstable();
        let response = Response {
            id: 1,
            owner: new_owner().unwrap(),
            last_mutation_id: 0,
            reply: Reply::Error {
                code: ErrorCode::Failed,
                message: "x".repeat(1024 * 1024 - 1024),
            },
        };
        let frame_len = wire::encode(&response, wire::MAX_RESPONSE_BYTES)
            .unwrap()
            .len();
        let mut crossed_global = false;
        for index in 0..80 {
            let client = clients[index % clients.len()];
            if !server.is_connected(client) {
                continue;
            }
            let queued: usize = server.peers.values().map(|peer| peer.queued).sum();
            let should_drop = queued + frame_len > MAX_GLOBAL_OUTGOING_BYTES;
            let sent = server.send(client, &response).unwrap();
            if should_drop {
                assert!(!sent);
                crossed_global = true;
            }
            assert!(
                server.peers.values().map(|peer| peer.queued).sum::<usize>() <= 64 * 1024 * 1024
            );
        }
        assert!(crossed_global, "non-reading peers must hit the global cap");
        // Authentication remains possible for another peer, and a small
        // independent response is delivered while other snapshots are stalled.
        let mut responsive = UnixStream::connect(server.endpoint()).unwrap();
        responsive.write_all(&hello_frame).unwrap();
        responsive
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        server.poll(Instant::now()).unwrap();
        let client = *server.peers.keys().max().unwrap();
        let small = Response {
            reply: Reply::Ok,
            ..response
        };
        assert!(server.send(client, &small).unwrap());
        assert!(matches!(
            wire::read::<Response>(&mut responsive, wire::MAX_RESPONSE_BYTES)
                .unwrap()
                .reply,
            Reply::Ok
        ));
        let future = Instant::now() + DEFAULT_TIMEOUT + Duration::from_secs(1);
        let events = server.poll(future).unwrap();
        assert!(!events.disconnected.is_empty());
        assert_eq!(
            server.peers.values().map(|peer| peer.queued).sum::<usize>(),
            0
        );
        assert!(
            server.is_connected(client),
            "stalled peers must not disconnect the responsive one"
        );
        drop(readers);
    }

    #[test]
    fn test_oversized_header_and_partial_frame_deadline_disconnect() {
        let directory = private_directory();
        let mut server = Server::bind_at(directory.path(), "main").unwrap();
        let mut peer = UnixStream::connect(server.endpoint()).unwrap();
        peer.write_all(&[b'U', b'A', b'T', b'1', 255, 255, 255, 127])
            .unwrap();
        let events = server.poll(Instant::now()).unwrap();
        assert!(events.requests.is_empty());
        assert!(server.peers.is_empty());
        let mut partial = UnixStream::connect(server.endpoint()).unwrap();
        partial.write_all(b"U").unwrap();
        server.poll(Instant::now()).unwrap();
        let events = server
            .poll(Instant::now() + DEFAULT_TIMEOUT + Duration::from_secs(1))
            .unwrap();
        assert!(events.requests.is_empty());
        assert!(server.peers.is_empty());
    }

    #[test]
    fn test_nonreading_and_silent_ssh_pipes_respect_deadline() {
        let mut child = ProcessCommand::new("/bin/sleep")
            .arg("5")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = child.stdout.take().unwrap();
        let mut writer = child.stdin.take().unwrap();
        set_nonblocking(reader.as_raw_fd()).unwrap();
        set_nonblocking(writer.as_raw_fd()).unwrap();
        let start = Instant::now();
        let read = DeadlinePipe {
            stream: &mut reader,
            deadline: start + Duration::from_millis(50),
        }
        .read(&mut [0]);
        assert!(matches!(read, Err(error) if error.kind() == io::ErrorKind::TimedOut));
        let write = DeadlinePipe {
            stream: &mut writer,
            deadline: Instant::now() + Duration::from_millis(50),
        }
        .write_all(&vec![1; 1024 * 1024]);
        assert!(matches!(write, Err(error) if error.kind() == io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(1));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
