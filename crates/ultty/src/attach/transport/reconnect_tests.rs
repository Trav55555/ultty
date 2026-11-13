//! Real socket regressions for owner-pinned reconnect and uncertain mutations.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::super::auth::{new_owner, Authority, Decision};
use super::*;

#[derive(Clone, Copy)]
enum DropMutation {
    Never,
    BeforeReservation,
    AfterReservation,
}

enum Control {
    ReplaceOwner(mpsc::Sender<()>),
    Controller(mpsc::Sender<bool>),
    Stop,
}

struct Harness {
    _directory: tempfile::TempDir,
    endpoint: PathBuf,
    token: String,
    executed: Arc<AtomicUsize>,
    control: mpsc::Sender<Control>,
    worker: Option<JoinHandle<()>>,
}

impl Harness {
    fn new(drop_mutation: DropMutation) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("ultty-reconnect-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut server = Server::bind_at(directory.path(), "main").unwrap();
        let endpoint = server.endpoint().to_owned();
        let mut authority = Authority::new("main", new_owner().unwrap()).unwrap();
        let token = authority.admin_token().expose_secret().to_owned();
        let executed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&executed);
        let (control, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut drop_next = drop_mutation;
            loop {
                while let Ok(command) = receiver.try_recv() {
                    match command {
                        Control::ReplaceOwner(done) => {
                            authority = Authority::new("main", new_owner().unwrap()).unwrap();
                            done.send(()).unwrap();
                        }
                        Control::Controller(done) => {
                            done.send(authority.controller().is_some()).unwrap();
                        }
                        Control::Stop => return,
                    }
                }
                let events = server.poll(Instant::now()).unwrap();
                for client in events.disconnected {
                    authority.disconnect(client);
                }
                for (client, request) in events.requests {
                    if request.command.is_mutation()
                        && matches!(drop_next, DropMutation::BeforeReservation)
                    {
                        drop_next = DropMutation::Never;
                        server.disconnect(client);
                        authority.disconnect(client);
                        continue;
                    }
                    let response = match authority.process(client, &request, Instant::now()) {
                        Decision::Reply(response) => response,
                        Decision::Execute { .. } => {
                            let reply = if request.command.is_mutation() {
                                counter.fetch_add(1, Ordering::SeqCst);
                                Reply::Ok
                            } else {
                                Reply::Unchanged { sequence: 1 }
                            };
                            authority.complete(client, &request, reply, Instant::now())
                        }
                    };
                    if request.command.is_mutation()
                        && matches!(drop_next, DropMutation::AfterReservation)
                    {
                        drop_next = DropMutation::Never;
                        server.disconnect(client);
                        authority.disconnect(client);
                        continue;
                    }
                    assert!(server.send(client, &response).unwrap());
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            _directory: directory,
            endpoint,
            token,
            executed,
            control,
            worker: Some(worker),
        }
    }

    fn client(&self) -> AttachmentClient {
        AttachmentClient::connect_at(&self.endpoint, "main", &self.token, true).unwrap()
    }

    fn replace_owner(&self) {
        let (send, recv) = mpsc::channel();
        self.control.send(Control::ReplaceOwner(send)).unwrap();
        recv.recv_timeout(Duration::from_secs(3)).unwrap();
    }

    fn controller_held(&self) -> bool {
        let (send, recv) = mpsc::channel();
        self.control.send(Control::Controller(send)).unwrap();
        recv.recv_timeout(Duration::from_secs(3)).unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn input() -> Command {
    Command::Input {
        pane: 0,
        data: b"x".to_vec(),
    }
}

#[test]
fn test_same_owner_reconnect_is_observer_until_verified_claim() {
    let host = Harness::new(DropMutation::Never);
    let mut client = host.client();
    let owner = client.owner().clone();
    let grant = client.grant_id();
    client.disconnect();
    let outcome = client.reconnect().unwrap();
    assert!(!outcome.mutations_blocked);
    assert!(!outcome.previous_mutation_unconfirmed);
    assert_eq!(client.owner(), &owner);
    assert_eq!(client.grant_id(), grant);
    assert!(!client.controller());
    assert!(client.claim_verified_control().unwrap());
    assert!(client.controller());
    assert!(matches!(client.request(input()).unwrap().reply, Reply::Ok));
    assert_eq!(host.executed.load(Ordering::SeqCst), 1);
}

#[test]
fn test_unreserved_unknown_id_cannot_be_reused_by_retry_or_claim() {
    let host = Harness::new(DropMutation::BeforeReservation);
    let mut client = host.client();
    assert!(client.request(input()).is_err());
    client.disconnect();
    assert!(client.has_unconfirmed_mutation());
    let stale = Request {
        id: 0,
        owner: Some(client.owner().clone()),
        grant_id: Some(client.grant_id()),
        command: input(),
    };
    assert_eq!(
        client.retry_exact(stale).unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    assert_eq!(
        client.claim_verified_control().unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    let outcome = client.reconnect().unwrap();
    assert!(outcome.mutations_blocked);
    assert!(outcome.previous_mutation_unconfirmed);
    assert!(client.mutations_blocked());
    assert_eq!(client.last_mutation_id(), 0);
    assert!(matches!(
        client
            .request(Command::Snapshot {
                after_sequence: None
            })
            .unwrap()
            .reply,
        Reply::Unchanged { .. }
    ));
    assert_eq!(
        client.claim_verified_control().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        client.request(input()).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(host.executed.load(Ordering::SeqCst), 0);
}

#[test]
fn test_reserved_lost_reply_is_not_replayed_and_next_id_advances() {
    let host = Harness::new(DropMutation::AfterReservation);
    let mut client = host.client();
    assert!(client.request(input()).is_err());
    let outcome = client.reconnect().unwrap();
    assert!(!outcome.mutations_blocked);
    assert!(outcome.previous_mutation_unconfirmed);
    assert_eq!(client.last_mutation_id(), 1);
    assert_eq!(
        host.executed.load(Ordering::SeqCst),
        1,
        "reconnect must not replay input"
    );
    assert!(client.claim_verified_control().unwrap());
    assert!(matches!(client.request(input()).unwrap().reply, Reply::Ok));
    assert_eq!(client.last_mutation_id(), 2);
    assert_eq!(host.executed.load(Ordering::SeqCst), 2);
}

#[test]
fn test_replacement_owner_cannot_receive_controller_claim() {
    let host = Harness::new(DropMutation::Never);
    let mut client = host.client();
    client.disconnect();
    host.replace_owner();
    assert!(client.reconnect().is_err());
    assert!(
        !host.controller_held(),
        "replacement owner must not acquire a controller"
    );
    assert_eq!(host.executed.load(Ordering::SeqCst), 0);
}

#[test]
fn test_reconnect_refuses_revoked_and_expired_grants() {
    let host = Harness::new(DropMutation::Never);
    let mut admin = host.client();
    for expire in [false, true] {
        let grant = admin
            .request(Command::Grant {
                capabilities: vec![Capability::Read],
                ttl_seconds: if expire { 1 } else { 60 },
            })
            .unwrap();
        let Reply::Granted {
            grant_id, token, ..
        } = grant.reply
        else {
            panic!("expected a scoped grant");
        };
        let mut observer =
            AttachmentClient::connect_at(&host.endpoint, "main", token.expose_secret(), false)
                .unwrap();
        observer.disconnect();
        if expire {
            thread::sleep(Duration::from_millis(1200));
        } else {
            assert!(matches!(
                admin.request(Command::Revoke { grant_id }).unwrap().reply,
                Reply::Ok
            ));
        }
        assert!(observer.reconnect().is_err());
        assert!(!observer.controller());
        assert!(observer
            .request(Command::Snapshot {
                after_sequence: None
            })
            .is_err());
    }
}

#[test]
fn test_reconnect_rejects_changed_grant_capabilities_and_watermark() {
    for change in ["grant", "capabilities", "watermark"] {
        let directory = tempfile::Builder::new()
            .prefix("ultty-pinned-")
            .tempdir_in("/tmp")
            .unwrap();
        let endpoint = directory.path().join("socket");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let owner = new_owner().unwrap();
        let expected_owner = owner.clone();
        let server = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request: Request = wire::read(&mut stream, wire::MAX_REQUEST_BYTES).unwrap();
                assert_eq!(
                    request.owner,
                    (attempt == 1).then_some(expected_owner.clone())
                );
                let Command::Hello { controller, .. } = request.command else {
                    panic!("expected hello");
                };
                assert_eq!(controller, attempt == 0);
                let grant_id = if attempt == 1 && change == "grant" {
                    8
                } else {
                    7
                };
                let capabilities = if attempt == 1 && change == "capabilities" {
                    vec![Capability::Read]
                } else {
                    vec![Capability::Read, Capability::Input]
                };
                let watermark = if attempt == 1 && change == "watermark" {
                    4
                } else {
                    5
                };
                wire::write(
                    &mut stream,
                    &Response {
                        id: 0,
                        owner: expected_owner.clone(),
                        last_mutation_id: watermark,
                        reply: Reply::Welcome {
                            grant_id,
                            version: wire::VERSION,
                            capabilities,
                            admin: false,
                            controller,
                        },
                    },
                    wire::MAX_RESPONSE_BYTES,
                )
                .unwrap();
            }
        });
        let mut client =
            AttachmentClient::connect_at(&endpoint, "main", "1.fixture", true).unwrap();
        client.disconnect();
        assert_eq!(
            client.reconnect().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "{change}"
        );
        assert!(!client.controller());
        server.join().unwrap();
    }
}
