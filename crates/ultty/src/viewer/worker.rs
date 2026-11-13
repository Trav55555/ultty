//! Bounded viewer I/O and explicit recovery, independent of the GPU thread.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::attach::model::SessionSnapshot;
use crate::attach::transport::AttachmentClient;
use crate::attach::wire::{Command, ErrorCode, Reply};

const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionState {
    #[default]
    Reconnecting,
    Connected,
    Disconnected,
    Refused,
}

pub(super) enum Action {
    Command(Command),
    Reconnect,
    ClaimControl,
}

pub(super) struct Request {
    pub(super) generation: u64,
    pub(super) action: Action,
}

/// Last complete replacement and connection state, coalesced for the GUI.
#[derive(Default)]
pub(super) struct Delivery {
    pub(super) snapshot: Option<SessionSnapshot>,
    pub(super) error: Option<String>,
    pub(super) controller: bool,
    pub(super) state: ConnectionState,
    pub(super) generation: u64,
    pub(super) mutations_blocked: bool,
    pub(super) unconfirmed: bool,
    pub(super) control_unavailable: bool,
}

fn fatal(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidData
            | io::ErrorKind::InvalidInput
            | io::ErrorKind::Unsupported
    )
}

fn advance_generation(generation: &mut u64) -> bool {
    match generation.checked_add(1) {
        Some(next) => {
            *generation = next;
            true
        }
        None => false,
    }
}

/// The worker never replays queued work across a connection generation. Retry
/// requires a fresh user action; an already started RPC retains its deadline.
pub(super) fn spawn(
    mut client: AttachmentClient,
    receiver: mpsc::Receiver<Request>,
    output: Arc<Mutex<Delivery>>,
    closing: Arc<AtomicBool>,
    wake: impl Fn() -> bool + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("ultty-viewer-link".into())
        .spawn(move || {
            let mut generation = 1;
            let mut connected = true;
            let mut refused = false;
            let mut retry = false;
            let mut wants_control = client.controller();
            let mut snapshot_due = Instant::now();
            let mut after_sequence = None;
            let mut fresh_snapshot = false;
            if let Ok(mut state) = output.lock() {
                state.generation = generation;
            } else {
                return;
            }
            loop {
                if closing.load(Ordering::Acquire) {
                    break;
                }
                if retry {
                    retry = false;
                    client.disconnect();
                    if !advance_generation(&mut generation) {
                        break;
                    }
                    if let Ok(mut state) = output.lock() {
                        state.generation = generation;
                        state.state = ConnectionState::Reconnecting;
                        state.controller = false;
                        state.error = None;
                    } else {
                        break;
                    }
                    // Discard all queued commands, including input typed offline.
                    while receiver.try_recv().is_ok() {}
                    if !wake() {
                        break;
                    }
                    let recovery = client.reconnect().and_then(|_| {
                        if closing.load(Ordering::Acquire) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                        if wants_control && !client.mutations_blocked() {
                            client.claim_verified_control()?;
                        }
                        if closing.load(Ordering::Acquire) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                        Ok(())
                    });
                    if closing.load(Ordering::Acquire) {
                        break;
                    }
                    match recovery {
                        Ok(()) => {
                            connected = true;
                            fresh_snapshot = true;
                            snapshot_due = Instant::now();
                        }
                        Err(error) => {
                            connected = false;
                            refused = fatal(&error);
                            client.disconnect();
                            if let Ok(mut state) = output.lock() {
                                state.state = if refused {
                                    ConnectionState::Refused
                                } else {
                                    ConnectionState::Disconnected
                                };
                                state.error = Some(error.to_string());
                                state.unconfirmed = client.has_unconfirmed_mutation();
                                state.mutations_blocked = client.mutations_blocked();
                            } else {
                                break;
                            }
                            if !wake() {
                                break;
                            }
                        }
                    }
                }
                if !connected {
                    match receiver.recv_timeout(SNAPSHOT_INTERVAL) {
                        Ok(request) if request.generation == generation && !refused => {
                            if matches!(request.action, Action::Reconnect) {
                                retry = true;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        _ => {}
                    }
                    continue;
                }
                let command = if Instant::now() >= snapshot_due {
                    Command::Snapshot { after_sequence }
                } else {
                    match receiver
                        .recv_timeout(snapshot_due.saturating_duration_since(Instant::now()))
                    {
                        Ok(request) if request.generation == generation => match request.action {
                            Action::Reconnect => {
                                retry = true;
                                continue;
                            }
                            Action::ClaimControl => {
                                if !client.controller() && !client.mutations_blocked() {
                                    wants_control = true;
                                    retry = true;
                                }
                                continue;
                            }
                            Action::Command(command) => command,
                        },
                        Ok(_) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            Command::Snapshot { after_sequence }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
                if closing.load(Ordering::Acquire) {
                    break;
                }
                let command = match command {
                    Command::Snapshot { .. } => Command::Snapshot {
                        after_sequence: if fresh_snapshot { None } else { after_sequence },
                    },
                    other => other,
                };
                let snapshot_request = matches!(command, Command::Snapshot { .. });
                let mut result = client.request(command);
                if snapshot_request {
                    snapshot_due = Instant::now() + SNAPSHOT_INTERVAL;
                }
                if let Ok(response) = &result {
                    if let Reply::Snapshot { snapshot } = &response.reply {
                        if snapshot.owner != *client.owner()
                            || after_sequence.is_some_and(|last| snapshot.sequence < last)
                        {
                            result = Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "snapshot owner or sequence changed",
                            ));
                        } else if let Err(error) = snapshot.validate() {
                            result = Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("invalid snapshot: {error}"),
                            ));
                        }
                    }
                }
                let Ok(mut state) = output.lock() else {
                    break;
                };
                let mut changed = false;
                match result {
                    Ok(response) => match response.reply {
                        Reply::Snapshot { snapshot } => {
                            fresh_snapshot = false;
                            if after_sequence != Some(snapshot.sequence) {
                                after_sequence = Some(snapshot.sequence);
                                state.snapshot = Some(*snapshot);
                                changed = true;
                            }
                            changed |= state.state != ConnectionState::Connected
                                || state.controller != client.controller();
                            state.state = ConnectionState::Connected;
                            state.controller = client.controller();
                            state.control_unavailable =
                                wants_control && !state.controller && !client.mutations_blocked();
                            state.mutations_blocked = client.mutations_blocked();
                            state.unconfirmed = client.has_unconfirmed_mutation();
                        }
                        Reply::Unchanged { sequence }
                            if !fresh_snapshot && after_sequence == Some(sequence) => {}
                        Reply::Unchanged { .. } => {
                            connected = false;
                            refused = true;
                            state.error =
                                Some("owner returned an inconsistent unchanged sequence".into());
                            changed = true;
                        }
                        Reply::Error { code, message } => {
                            if matches!(
                                code,
                                ErrorCode::Unauthorized
                                    | ErrorCode::Expired
                                    | ErrorCode::StaleOwner
                                    | ErrorCode::InvalidVersion
                            ) {
                                connected = false;
                                refused = true;
                            }
                            state.error = Some(message);
                            changed = true;
                        }
                        _ => {}
                    },
                    Err(error) => {
                        connected = false;
                        refused = fatal(&error);
                        state.error = Some(error.to_string());
                        changed = true;
                    }
                }
                if !connected {
                    client.disconnect();
                    if !advance_generation(&mut generation) {
                        break;
                    }
                    state.generation = generation;
                    state.state = if refused {
                        ConnectionState::Refused
                    } else {
                        ConnectionState::Disconnected
                    };
                    state.controller = false;
                    state.unconfirmed = client.has_unconfirmed_mutation();
                    state.mutations_blocked = client.mutations_blocked();
                }
                drop(state);
                if changed && !wake() {
                    break;
                }
            }
            client.disconnect();
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use ultty_protocol::control::OwnerRef;

    use super::*;
    use crate::attach::model::SNAPSHOT_VERSION;
    use crate::attach::wire::{self, Capability, Request as WireRequest, Response};

    fn snapshot(sequence: u64) -> SessionSnapshot {
        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            owner: owner(),
            sequence,
            viewport: [80, 24],
            cell_pixels: [8, 16],
            active_tab: None,
            tabs: Vec::new(),
            panes: Vec::new(),
        }
    }

    fn owner() -> OwnerRef {
        OwnerRef {
            pid: 1,
            epoch: "a".repeat(64),
        }
    }

    /// Real framing/client correlation, with a controllable fake owner.
    fn fixture(
        first_snapshot: Option<mpsc::Sender<()>>,
        release_snapshot: Option<mpsc::Receiver<()>>,
        input_delay: Duration,
        stop_after_snapshots: Option<usize>,
        unchanged_after_first: bool,
    ) -> (
        AttachmentClient,
        JoinHandle<Vec<Command>>,
        tempfile::TempDir,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let hello: WireRequest = wire::read(&mut stream, wire::MAX_REQUEST_BYTES).unwrap();
            assert!(matches!(hello.command, Command::Hello { .. }));
            wire::write(
                &mut stream,
                &Response {
                    id: hello.id,
                    owner: owner(),
                    last_mutation_id: 0,
                    reply: Reply::Welcome {
                        grant_id: 1,
                        version: wire::VERSION,
                        capabilities: vec![Capability::Read, Capability::Input],
                        admin: false,
                        controller: true,
                    },
                },
                wire::MAX_RESPONSE_BYTES,
            )
            .unwrap();
            let mut commands = Vec::new();
            let mut snapshots = 0usize;
            let mut watermark = 0;
            while let Ok(request) = wire::read::<WireRequest>(&mut stream, wire::MAX_REQUEST_BYTES)
            {
                assert_eq!(request.owner, Some(owner()));
                assert_eq!(request.grant_id, Some(1));
                let reply = match &request.command {
                    Command::Snapshot { after_sequence } => {
                        snapshots += 1;
                        if snapshots == 1 {
                            if let Some(ready) = &first_snapshot {
                                ready.send(()).unwrap();
                            }
                            if let Some(release) = &release_snapshot {
                                release.recv_timeout(Duration::from_secs(5)).unwrap();
                            }
                        }
                        assert_eq!(
                            *after_sequence,
                            (snapshots > 1).then_some(if unchanged_after_first {
                                1
                            } else {
                                snapshots as u64 - 1
                            })
                        );
                        if unchanged_after_first && snapshots > 1 {
                            Reply::Unchanged { sequence: 1 }
                        } else {
                            Reply::Snapshot {
                                snapshot: Box::new(snapshot(snapshots as u64)),
                            }
                        }
                    }
                    Command::Input { .. } => {
                        assert_eq!(request.id, watermark + 1);
                        watermark = request.id;
                        std::thread::sleep(input_delay);
                        Reply::Ok
                    }
                    other => panic!("unexpected fixture command: {other:?}"),
                };
                commands.push(request.command);
                wire::write(
                    &mut stream,
                    &Response {
                        id: request.id,
                        owner: owner(),
                        last_mutation_id: watermark,
                        reply,
                    },
                    wire::MAX_RESPONSE_BYTES,
                )
                .unwrap();
                if stop_after_snapshots.is_some_and(|limit| snapshots >= limit) {
                    break;
                }
            }
            commands
        });
        let client =
            AttachmentClient::connect_at(&endpoint, "fixture", "explicit-test-credential", true)
                .unwrap();
        (client, server, directory)
    }

    fn flood(sender: &mpsc::SyncSender<Request>) {
        for byte in 0..128u8 {
            sender
                .send(Request {
                    generation: 1,
                    action: Action::Command(Command::Input {
                        pane: 7,
                        data: vec![byte],
                    }),
                })
                .unwrap();
        }
    }

    fn disconnected(delivery: &Mutex<Delivery>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while delivery.lock().unwrap().state != ConnectionState::Disconnected {
            assert!(
                Instant::now() < deadline,
                "worker did not observe transport loss"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn test_viewer_worker_close_discards_queued_mutations() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (client, server, _directory) = fixture(
            Some(ready_tx),
            Some(release_rx),
            Duration::ZERO,
            None,
            false,
        );
        let (sender, receiver) = mpsc::sync_channel(128);
        let closing = Arc::new(AtomicBool::new(false));
        let delivery = Arc::new(Mutex::new(Delivery::default()));
        let worker = spawn(client, receiver, delivery, Arc::clone(&closing), || true).unwrap();
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        flood(&sender);
        closing.store(true, Ordering::Release);
        drop(sender);
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        let commands = server.join().unwrap();
        assert_eq!(
            commands.len(),
            1,
            "closing must discard all 128 not-started mutations"
        );
        assert!(matches!(commands[0], Command::Snapshot { .. }));
    }

    #[test]
    fn test_viewer_worker_publication_is_not_starved_by_input_flood() {
        // Two milliseconds per request means 128 queued requests necessarily
        // span several 50ms publication deadlines; the queue stays occupied.
        let (client, server, _directory) =
            fixture(None, None, Duration::from_millis(2), Some(2), false);
        let (sender, receiver) = mpsc::sync_channel(128);
        flood(&sender);
        let delivery = Arc::new(Mutex::new(Delivery::default()));
        let closing = Arc::new(AtomicBool::new(false));
        let worker = spawn(client, receiver, Arc::clone(&delivery), closing, || true).unwrap();
        let commands = server.join().unwrap();
        disconnected(&delivery);
        drop(sender);
        worker.join().unwrap();
        let last = commands
            .iter()
            .rposition(|command| matches!(command, Command::Snapshot { .. }))
            .unwrap();
        let inputs = commands[..last]
            .iter()
            .filter(|command| matches!(command, Command::Input { .. }))
            .count();
        assert!(
            inputs > 0 && inputs < 128,
            "a second snapshot must precede input queue exhaustion"
        );
        let state = delivery.lock().unwrap();
        assert_eq!(state.snapshot.as_ref().unwrap().sequence, 2);
        assert!(
            !state.controller,
            "connection loss must remove local controller affordances"
        );
        assert!(state.error.is_some());
    }

    #[test]
    fn test_viewer_worker_polls_last_revision_without_redelivering_unchanged_state() {
        use std::sync::atomic::AtomicUsize;
        let (client, server, _directory) = fixture(None, None, Duration::ZERO, Some(3), true);
        let (sender, receiver) = mpsc::sync_channel(128);
        let delivery = Arc::new(Mutex::new(Delivery::default()));
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = Arc::clone(&wakes);
        let worker = spawn(
            client,
            receiver,
            Arc::clone(&delivery),
            Arc::new(AtomicBool::new(false)),
            move || {
                wake_count.fetch_add(1, Ordering::Relaxed);
                true
            },
        )
        .unwrap();
        let commands = server.join().unwrap();
        disconnected(&delivery);
        drop(sender);
        worker.join().unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(
            wakes.load(Ordering::Relaxed),
            2,
            "only the initial snapshot and connection loss wake the GUI"
        );
        assert_eq!(
            delivery.lock().unwrap().snapshot.as_ref().unwrap().sequence,
            1
        );
    }
    #[test]
    fn test_idle_reconnect_requires_fresh_snapshot_and_discards_old_generation() {
        for controller in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let endpoint = directory.path().join("recovery.sock");
            let listener = UnixListener::bind(&endpoint).unwrap();
            let server = std::thread::spawn(move || {
                let welcome = |stream: &mut std::os::unix::net::UnixStream,
                               pinned: bool,
                               control: bool| {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let hello: wire::Request = wire::read(stream, wire::MAX_REQUEST_BYTES).unwrap();
                    assert_eq!(hello.owner, pinned.then(owner));
                    assert!(
                        matches!(hello.command, Command::Hello { controller, .. } if controller == control)
                    );
                    wire::write(
                        stream,
                        &Response {
                            id: hello.id,
                            owner: owner(),
                            last_mutation_id: 0,
                            reply: Reply::Welcome {
                                grant_id: 1,
                                version: wire::VERSION,
                                capabilities: vec![Capability::Read, Capability::Input],
                                admin: false,
                                controller: control,
                            },
                        },
                        wire::MAX_RESPONSE_BYTES,
                    )
                    .unwrap();
                };
                let publish = |stream: &mut std::os::unix::net::UnixStream, cursor: Option<u64>| {
                    let request: wire::Request =
                        wire::read(stream, wire::MAX_REQUEST_BYTES).unwrap();
                    assert!(
                        matches!(request.command, Command::Snapshot { after_sequence } if after_sequence == cursor)
                    );
                    wire::write(
                        stream,
                        &Response {
                            id: request.id,
                            owner: owner(),
                            last_mutation_id: 0,
                            reply: Reply::Snapshot {
                                snapshot: Box::new(snapshot(7)),
                            },
                        },
                        wire::MAX_RESPONSE_BYTES,
                    )
                    .unwrap();
                };
                let (mut first, _) = listener.accept().unwrap();
                welcome(&mut first, false, controller);
                publish(&mut first, None);
                drop(first);
                let (mut observer, _) = listener.accept().unwrap();
                welcome(&mut observer, true, false);
                let mut recovered = if controller {
                    let (mut controlling, _) = listener.accept().unwrap();
                    welcome(&mut controlling, true, true);
                    drop(observer);
                    controlling
                } else {
                    observer
                };
                publish(&mut recovered, None);
                while let Ok(request) =
                    wire::read::<wire::Request>(&mut recovered, wire::MAX_REQUEST_BYTES)
                {
                    assert!(
                        matches!(
                            request.command,
                            Command::Snapshot {
                                after_sequence: Some(7)
                            }
                        ),
                        "old-generation input crossed reconnect"
                    );
                    wire::write(
                        &mut recovered,
                        &Response {
                            id: request.id,
                            owner: owner(),
                            last_mutation_id: 0,
                            reply: Reply::Unchanged { sequence: 7 },
                        },
                        wire::MAX_RESPONSE_BYTES,
                    )
                    .unwrap();
                }
            });
            let client =
                AttachmentClient::connect_at(&endpoint, "fixture", "test-token", controller)
                    .unwrap();
            let (sender, receiver) = mpsc::sync_channel(128);
            let delivery = Arc::new(Mutex::new(Delivery::default()));
            let closing = Arc::new(AtomicBool::new(false));
            let worker = spawn(
                client,
                receiver,
                Arc::clone(&delivery),
                Arc::clone(&closing),
                || true,
            )
            .unwrap();
            disconnected(&delivery);
            let generation = {
                let mut state = delivery.lock().unwrap();
                assert_eq!(state.snapshot.take().unwrap().sequence, 7);
                state.generation
            };
            sender
                .send(Request {
                    generation: generation - 1,
                    action: Action::Command(Command::Input {
                        pane: 7,
                        data: vec![42],
                    }),
                })
                .unwrap();
            sender
                .send(Request {
                    generation,
                    action: Action::Reconnect,
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let state = delivery.lock().unwrap();
                if state.state == ConnectionState::Connected && state.generation > generation {
                    assert_eq!(state.controller, controller);
                    assert!(
                        state.snapshot.is_none(),
                        "equal revision need not replace the retained GUI replica"
                    );
                    break;
                }
                drop(state);
                assert!(Instant::now() < deadline, "idle reconnect did not finish");
                std::thread::sleep(Duration::from_millis(1));
            }
            closing.store(true, Ordering::Release);
            drop(sender);
            worker.join().unwrap();
            server.join().unwrap();
        }
    }
}
