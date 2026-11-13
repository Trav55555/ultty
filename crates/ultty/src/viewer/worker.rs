//! Bounded viewer I/O, independent of the window event loop and GPU.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::attach::model::SessionSnapshot;
use crate::attach::transport::AttachmentClient;
use crate::attach::wire::{Command, Reply};

const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(50);

/// Coalesced publication and connection state waiting for the GUI thread.
#[derive(Default)]
pub(super) struct Delivery {
    /// Latest complete replacement, retained across unchanged replies.
    pub(super) snapshot: Option<SessionSnapshot>,
    /// Most recent refusal or connection error.
    pub(super) error: Option<String>,
    /// Current connection's controller lease, cleared on connection loss.
    pub(super) controller: bool,
}

/// Start bounded I/O. Closing discards queued work, while an already started
/// request finishes under the transport's deadline. `wake` returns false once
/// the GUI event loop is gone.
pub(super) fn spawn(
    mut client: AttachmentClient,
    receiver: mpsc::Receiver<Command>,
    output: Arc<Mutex<Delivery>>,
    closing: Arc<AtomicBool>,
    wake: impl Fn() -> bool + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("ultty-viewer-link".into())
        .spawn(move || {
            let mut snapshot_due = Instant::now();
            let mut after_sequence = None;
            loop {
                if closing.load(Ordering::Acquire) {
                    break;
                }
                let request = if Instant::now() >= snapshot_due {
                    Command::Snapshot {
                        after_sequence: None,
                    }
                } else {
                    match receiver
                        .recv_timeout(snapshot_due.saturating_duration_since(Instant::now()))
                    {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => Command::Snapshot {
                            after_sequence: None,
                        },
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
                let request = match request {
                    Command::Snapshot { .. } => Command::Snapshot { after_sequence },
                    other => other,
                };
                // Closing discards queued work. Only an already started exchange
                // can finish, within the transport's whole-request deadline.
                if closing.load(Ordering::Acquire) {
                    break;
                }
                let snapshot_request = matches!(request, Command::Snapshot { .. });
                let result = client.request(request);
                if snapshot_request {
                    snapshot_due = Instant::now() + SNAPSHOT_INTERVAL;
                }
                let mut connection_lost = result.is_err();
                let Ok(mut delivery) = output.lock() else {
                    tracing::warn!("viewer delivery mutex was poisoned");
                    break;
                };
                let controller = !connection_lost && client.controller();
                let mut changed = delivery.controller != controller;
                delivery.controller = controller;
                match result {
                    Ok(response) => match response.reply {
                        Reply::Snapshot { snapshot } => {
                            after_sequence = Some(snapshot.sequence);
                            delivery.snapshot = Some(*snapshot);
                            changed = true;
                        }
                        Reply::Unchanged { sequence } => {
                            if after_sequence != Some(sequence) {
                                delivery.error = Some(
                                    "owner returned an inconsistent unchanged sequence".into(),
                                );
                                delivery.controller = false;
                                connection_lost = true;
                                changed = true;
                            }
                        }
                        Reply::Error { message, .. } => {
                            delivery.error = Some(message);
                            changed = true;
                        }
                        _ => {}
                    },
                    Err(error) => {
                        delivery.error = Some(error.to_string());
                        changed = true;
                    }
                }
                drop(delivery);
                if (changed && !wake()) || connection_lost {
                    break;
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use ultty_protocol::control::OwnerRef;

    use super::*;
    use crate::attach::model::SNAPSHOT_VERSION;
    use crate::attach::wire::{self, Capability, Request, Response};

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
            let hello: Request = wire::read(&mut stream, wire::MAX_REQUEST_BYTES).unwrap();
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
            while let Ok(request) = wire::read::<Request>(&mut stream, wire::MAX_REQUEST_BYTES) {
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

    fn flood(sender: &mpsc::SyncSender<Command>) {
        for byte in 0..128u8 {
            sender
                .send(Command::Input {
                    pane: 7,
                    data: vec![byte],
                })
                .unwrap();
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
        worker.join().unwrap();
        drop(sender);
        let commands = server.join().unwrap();
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
        worker.join().unwrap();
        drop(sender);
        let commands = server.join().unwrap();
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
}
