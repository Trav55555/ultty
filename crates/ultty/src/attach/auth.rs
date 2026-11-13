//! Host-owned scoped grants, one controller and bounded mutation receipts.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read};
use std::time::{Duration, Instant};

use ultty_protocol::control::OwnerRef;

use super::wire::{self, Capability, Command, ErrorCode, Reply, Request, Response, Token};

/// Transport connection identity, never a process or native-session identity.
pub type ClientId = u64;
/// Most active grants, including the distinct local owner credential.
pub const MAX_GRANTS: usize = 256;
/// Most completed mutation receipts.
pub const MAX_RECEIPTS: usize = 256;
/// Exact command/receipt byte budget; evictions retain the grant watermark.
pub const MAX_RECEIPT_BYTES: usize = 8 * 1024 * 1024;
/// Receipt retention never extends credential expiry.
pub const RECEIPT_TTL: Duration = Duration::from_secs(300);
const MAX_CLIENTS: usize = 32;
const ALL_CAPS: [Capability; 4] = [
    Capability::Read,
    Capability::Input,
    Capability::Spawn,
    Capability::Layout,
];

struct Grant {
    secret: [u8; 32],
    capabilities: Vec<Capability>,
    expires: Option<Instant>,
    watermark: u64,
}
struct Receipt {
    grant: u64,
    id: u64,
    command: Vec<u8>,
    reply: Reply,
    bytes: usize,
    expires: Instant,
}
struct Pending {
    command: Vec<u8>,
}

/// Runtime work is explicitly separated from authority/receipt reservation.
pub enum Decision {
    /// Reply immediately without executing runtime work.
    Reply(Response),
    /// Execute once, then call complete even if runtime execution fails.
    Execute {
        /// Authenticated grant owning the mutation namespace.
        grant_id: u64,
        /// Request correlation/mutation identity.
        id: u64,
        /// Validated operation.
        command: Command,
    },
}

/// One host lifetime's authority. Grant/admin state never comes from a viewer.
pub struct Authority {
    owner: OwnerRef,
    admin_secret: [u8; 32],
    session: String,
    grants: HashMap<u64, Grant>,
    clients: HashMap<ClientId, u64>,
    controller: Option<ClientId>,
    next_grant: Option<u64>,
    receipts: VecDeque<Receipt>,
    receipt_bytes: usize,
    pending: HashMap<(u64, u64), Pending>,
    pending_bytes: usize,
    disconnected: Vec<ClientId>,
}

/// Generate a fresh host lifetime, independent of native/control credentials.
pub fn new_owner() -> io::Result<OwnerRef> {
    let secret = random_secret()?;
    Ok(OwnerRef {
        pid: std::process::id(),
        epoch: hexadecimal(&secret),
    })
}

impl Authority {
    /// Create a distinct random local admin credential for a named owner.
    pub fn new(session: &str, owner: OwnerRef) -> io::Result<Self> {
        if !wire::valid_session_name(session) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session name",
            ));
        }
        let mut grants = HashMap::new();
        let admin_secret = random_secret()?;
        grants.insert(
            0,
            Grant {
                secret: admin_secret,
                capabilities: ALL_CAPS.to_vec(),
                expires: None,
                watermark: 0,
            },
        );
        Ok(Self {
            owner,
            admin_secret,
            session: session.into(),
            grants,
            clients: HashMap::new(),
            controller: None,
            next_grant: Some(1),
            receipts: VecDeque::new(),
            receipt_bytes: 0,
            pending: HashMap::new(),
            pending_bytes: 0,
            disconnected: Vec::new(),
        })
    }
    /// Exact host lifetime.
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    /// Initial local-only owner credential; never forward it through SSH.
    pub fn admin_token(&self) -> Token {
        token(0, &self.admin_secret)
    }
    /// Current sole controller, if any.
    pub fn controller(&self) -> Option<ClientId> {
        self.controller
    }
    /// Whether a connection has a still-registered authenticated grant.
    pub fn is_authenticated(&self, client: ClientId) -> bool {
        self.clients.contains_key(&client)
    }
    /// Authenticated viewer count after expire has enforced grant deadlines.
    pub fn authenticated_clients(&self) -> usize {
        self.clients.len()
    }
    /// Viewer loss releases authority over dimensions/input, never its children.
    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
        if self.controller == Some(client) {
            self.controller = None;
        }
    }
    /// Expire grants and return connections that the transport must close.
    pub fn expire(&mut self, now: Instant) -> Vec<ClientId> {
        self.expire_grants(now);
        std::mem::take(&mut self.disconnected)
    }
    fn expire_grants(&mut self, now: Instant) {
        let expired: Vec<_> = self
            .grants
            .iter()
            .filter(|(_, grant)| grant.expires.is_some_and(|until| now >= until))
            .map(|(id, _)| *id)
            .collect();
        for grant in expired {
            self.remove_grant(grant);
        }
        while self
            .receipts
            .front()
            .is_some_and(|receipt| now >= receipt.expires)
        {
            if let Some(receipt) = self.receipts.pop_front() {
                self.receipt_bytes -= receipt.bytes;
            }
        }
    }
    fn remove_grant(&mut self, grant: u64) {
        self.grants.remove(&grant);
        let clients: Vec<_> = self
            .clients
            .iter()
            .filter(|(_, id)| **id == grant)
            .map(|(client, _)| *client)
            .collect();
        for client in clients {
            self.disconnect(client);
            self.disconnected.push(client);
        }
        self.pending.retain(|(id, _), _| *id != grant);
        self.pending_bytes = self
            .pending
            .values()
            .map(|pending| pending.command.len())
            .sum();
        self.receipts.retain(|receipt| receipt.grant != grant);
        self.receipt_bytes = self.receipts.iter().map(|receipt| receipt.bytes).sum();
    }
    fn response(&self, client: ClientId, id: u64, reply: Reply) -> Response {
        let watermark = self
            .clients
            .get(&client)
            .and_then(|grant| self.grants.get(grant))
            .map_or(0, |grant| grant.watermark);
        Response {
            id,
            owner: self.owner.clone(),
            last_mutation_id: watermark,
            reply,
        }
    }
    fn refusal(&self, client: ClientId, id: u64, code: ErrorCode, message: &str) -> Decision {
        Decision::Reply(self.response(client, id, Reply::error(code, message)))
    }
    /// Authenticate, fence, authorize and reserve one operation. Expiry/revocation
    /// is checked before every operation, including cached receipt retrieval.
    pub fn process(&mut self, client: ClientId, request: &Request, now: Instant) -> Decision {
        self.expire_grants(now);
        if let Command::Hello {
            version,
            session,
            token,
            controller,
        } = &request.command
        {
            if self.clients.contains_key(&client) {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::Conflict,
                    "Hello already completed",
                );
            }
            if *version != wire::VERSION {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::InvalidVersion,
                    "unsupported attachment version",
                );
            }
            if request.grant_id.is_some()
                || request
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner != &self.owner)
            {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::StaleOwner,
                    "Hello owner/grant mismatch",
                );
            }
            if session != &self.session || !wire::valid_session_name(session) {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::InvalidRequest,
                    "session mismatch",
                );
            }
            let Some((id, supplied)) = parse_token(token) else {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::Unauthorized,
                    "invalid attachment credential",
                );
            };
            let Some(grant) = self
                .grants
                .get(&id)
                .filter(|grant| secret_matches(&grant.secret, &supplied))
            else {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::Unauthorized,
                    "invalid or expired attachment credential",
                );
            };
            if self.clients.len() >= MAX_CLIENTS {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::Capacity,
                    "attachment connection capacity reached",
                );
            }
            if *controller && (!can_control(id, grant) || self.controller.is_some()) {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::Busy,
                    "controller unavailable or capability refused",
                );
            }
            let capabilities = grant.capabilities.clone();
            self.clients.insert(client, id);
            if *controller {
                self.controller = Some(client);
            }
            return Decision::Reply(self.response(
                client,
                request.id,
                Reply::Welcome {
                    grant_id: id,
                    version: wire::VERSION,
                    capabilities,
                    admin: id == 0,
                    controller: *controller,
                },
            ));
        }
        let Some(grant_id) = self.clients.get(&client).copied() else {
            return self.refusal(
                client,
                request.id,
                ErrorCode::Unauthorized,
                "Hello required or grant expired/revoked",
            );
        };
        if request.owner.as_ref() != Some(&self.owner) || request.grant_id != Some(grant_id) {
            return self.refusal(
                client,
                request.id,
                ErrorCode::StaleOwner,
                "owner/grant identity changed",
            );
        }
        if let Err(error) = request.command.validate() {
            return self.refusal(
                client,
                request.id,
                ErrorCode::InvalidRequest,
                &error.to_string(),
            );
        }
        let Some(grant) = self.grants.get(&grant_id) else {
            return self.refusal(
                client,
                request.id,
                ErrorCode::Unauthorized,
                "grant unavailable",
            );
        };
        if request
            .command
            .capability()
            .is_some_and(|capability| !grant.capabilities.contains(&capability))
            || (matches!(
                request.command,
                Command::Layout {
                    command: wire::LayoutCommand::Split { .. }
                }
            ) && !grant.capabilities.contains(&Capability::Spawn))
            || (matches!(
                request.command,
                Command::Grant { .. } | Command::Revoke { .. } | Command::Terminate
            ) && grant_id != 0)
            || (matches!(request.command, Command::ClaimControl) && !can_control(grant_id, grant))
        {
            return self.refusal(
                client,
                request.id,
                ErrorCode::PermissionDenied,
                "capability refused",
            );
        }
        if !request.command.is_mutation() {
            return Decision::Execute {
                grant_id,
                id: request.id,
                command: request.command.clone(),
            };
        }
        let canonical = match wire::encode(&request.command, wire::MAX_REQUEST_BYTES) {
            Ok(bytes) => bytes,
            Err(_) => {
                return self.refusal(
                    client,
                    request.id,
                    ErrorCode::InvalidRequest,
                    "command exceeds encoding limit",
                )
            }
        };
        if request.id <= grant.watermark {
            return match self.receipts.iter().find(|receipt| receipt.grant == grant_id && receipt.id == request.id) {
                Some(receipt) if receipt.command == canonical => Decision::Reply(self.response(client, request.id, receipt.reply.clone())),
                Some(_) => self.refusal(client, request.id, ErrorCode::Conflict, "mutation ID already used for another command"),
                None => self.refusal(client, request.id, ErrorCode::UnknownOperation, "mutation outcome unknown; receipt expired/evicted or still pending; never re-executed"),
            };
        }
        if grant.watermark.checked_add(1) != Some(request.id) {
            return self.refusal(
                client,
                request.id,
                ErrorCode::OutOfOrder,
                "mutation must use the exact next grant sequence",
            );
        }
        if (request.command.needs_controller()
            || matches!(request.command, Command::ReleaseControl))
            && self.controller != Some(client)
        {
            return self.refusal(
                client,
                request.id,
                ErrorCode::PermissionDenied,
                "controller lease required",
            );
        }
        if matches!(request.command, Command::ClaimControl)
            && self.controller.is_some()
            && self.controller != Some(client)
        {
            return self.refusal(
                client,
                request.id,
                ErrorCode::Busy,
                "controller lease is occupied",
            );
        }
        if self.pending.len() >= MAX_RECEIPTS
            || canonical.len() > MAX_RECEIPT_BYTES.saturating_sub(self.pending_bytes)
        {
            return self.refusal(
                client,
                request.id,
                ErrorCode::Capacity,
                "unresolved mutation capacity reached",
            );
        }
        if let Some(grant) = self.grants.get_mut(&grant_id) {
            grant.watermark = request.id;
        }
        self.pending_bytes += canonical.len();
        self.pending
            .insert((grant_id, request.id), Pending { command: canonical });
        let immediate = match &request.command {
            Command::ClaimControl => {
                self.controller = Some(client);
                Some(Reply::Ok)
            }
            Command::ReleaseControl => {
                self.controller = None;
                Some(Reply::Ok)
            }
            Command::Grant {
                capabilities,
                ttl_seconds,
            } => Some(self.issue_grant(capabilities, *ttl_seconds, now)),
            Command::Revoke { grant_id: id } => Some(if *id == 0 {
                Reply::error(
                    ErrorCode::PermissionDenied,
                    "the initial owner credential cannot be delegated or revoked",
                )
            } else if self.grants.contains_key(id) {
                self.remove_grant(*id);
                Reply::Ok
            } else {
                Reply::error(ErrorCode::InvalidRequest, "unknown grant")
            }),
            _ => None,
        };
        match immediate {
            Some(reply) => Decision::Reply(self.complete(client, request, reply, now)),
            None => Decision::Execute {
                grant_id,
                id: request.id,
                command: request.command.clone(),
            },
        }
    }
    fn issue_grant(
        &mut self,
        capabilities: &[Capability],
        ttl_seconds: u64,
        now: Instant,
    ) -> Reply {
        if self.grants.len() >= MAX_GRANTS {
            return Reply::error(ErrorCode::Capacity, "grant capacity reached");
        }
        let Some(id) = self.next_grant else {
            return Reply::error(ErrorCode::Capacity, "grant identities exhausted");
        };
        let secret = match random_secret() {
            Ok(secret) => secret,
            Err(error) => return Reply::error(ErrorCode::Failed, error.to_string()),
        };
        self.next_grant = id.checked_add(1);
        self.grants.insert(
            id,
            Grant {
                secret,
                capabilities: capabilities.to_vec(),
                expires: Some(now + Duration::from_secs(ttl_seconds)),
                watermark: 0,
            },
        );
        Reply::Granted {
            grant_id: id,
            token: token(id, &secret),
            capabilities: capabilities.to_vec(),
            ttl_seconds,
        }
    }
    /// Complete every executed command, including runtime refusals. Failed
    /// mutations consume their reserved ID and are safely replayable too.
    pub fn complete(
        &mut self,
        client: ClientId,
        request: &Request,
        reply: Reply,
        now: Instant,
    ) -> Response {
        if let Some(grant) = request.grant_id {
            if let Some(pending) = self.pending.remove(&(grant, request.id)) {
                self.pending_bytes -= pending.command.len();
                let encoded_reply = wire::encode(&reply, wire::MAX_RESPONSE_BYTES);
                if let Ok(encoded) = encoded_reply {
                    let bytes = pending.command.len() + encoded.len();
                    if bytes <= MAX_RECEIPT_BYTES {
                        self.receipt_bytes += bytes;
                        self.receipts.push_back(Receipt {
                            grant,
                            id: request.id,
                            command: pending.command,
                            reply: reply.clone(),
                            bytes,
                            expires: now + RECEIPT_TTL,
                        });
                    }
                }
                while self.receipts.len() > MAX_RECEIPTS || self.receipt_bytes > MAX_RECEIPT_BYTES {
                    if let Some(receipt) = self.receipts.pop_front() {
                        self.receipt_bytes -= receipt.bytes;
                    }
                }
            }
        }
        self.response(client, request.id, reply)
    }
}

fn can_control(id: u64, grant: &Grant) -> bool {
    id == 0
        || grant.capabilities.iter().any(|capability| {
            matches!(
                capability,
                Capability::Input | Capability::Spawn | Capability::Layout
            )
        })
}
fn random_secret() -> io::Result<[u8; 32]> {
    let mut secret = [0; 32];
    File::open("/dev/urandom")?.read_exact(&mut secret)?;
    Ok(secret)
}
fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn token(id: u64, secret: &[u8; 32]) -> Token {
    Token::new(format!("{id}.{}", hexadecimal(secret)))
}
fn parse_token(token: &Token) -> Option<(u64, [u8; 32])> {
    let (id, text) = token.expose_secret().split_once('.')?;
    let number = id.parse::<u64>().ok()?;
    if id != number.to_string()
        || text.len() != 64
        || !text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let mut secret = [0; 32];
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some((number, secret))
}
fn secret_matches(expected: &[u8; 32], supplied: &[u8; 32]) -> bool {
    expected
        .iter()
        .zip(supplied)
        .fold(0, |difference, (left, right)| difference | left ^ right)
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_split_requires_both_layout_and_spawn_including_default_shell() {
        let now = Instant::now();
        let mut auth = authority();
        let admin = auth.admin_token();
        hello(&mut auth, 1, admin, false, now);
        let (layout_id, layout) = granted(&mut auth, 1, vec![Capability::Layout], 60, now);
        hello(&mut auth, 2, layout, true, now);
        for program in [None, Some("/bin/true".to_string())] {
            let split = request(
                &auth,
                1,
                layout_id,
                Command::Layout {
                    command: wire::LayoutCommand::Split {
                        pane: 0,
                        direction: ultty_protocol::control::SplitDirection::Right,
                        program,
                        args: vec![],
                        cwd: None,
                    },
                },
            );
            let response = immediate(&mut auth, 2, &split, now);
            assert_eq!(
                response.last_mutation_id, 0,
                "refused split must not reserve a mutation"
            );
            refused(response, ErrorCode::PermissionDenied);
        }
        let (spawn_id, spawn) = granted(&mut auth, 2, vec![Capability::Spawn], 60, now);
        hello(&mut auth, 3, spawn, false, now);
        let split = request(
            &auth,
            1,
            spawn_id,
            Command::Layout {
                command: wire::LayoutCommand::Split {
                    pane: 0,
                    direction: ultty_protocol::control::SplitDirection::Right,
                    program: None,
                    args: vec![],
                    cwd: None,
                },
            },
        );
        refused(
            immediate(&mut auth, 3, &split, now),
            ErrorCode::PermissionDenied,
        );
        let (both_id, both) = granted(
            &mut auth,
            3,
            vec![Capability::Spawn, Capability::Layout],
            60,
            now,
        );
        auth.disconnect(2);
        hello(&mut auth, 4, both, true, now);
        let allowed = Request {
            grant_id: Some(both_id),
            ..split
        };
        assert!(matches!(
            auth.process(4, &allowed, now),
            Decision::Execute { .. }
        ));
    }

    fn authority() -> Authority {
        Authority::new(
            "main",
            OwnerRef {
                pid: 42,
                epoch: "a".repeat(64),
            },
        )
        .unwrap()
    }
    fn hello(
        authority: &mut Authority,
        client: ClientId,
        token: Token,
        controller: bool,
        now: Instant,
    ) -> Response {
        let request = Request {
            id: 0,
            owner: None,
            grant_id: None,
            command: Command::Hello {
                version: wire::VERSION,
                session: "main".into(),
                token,
                controller,
            },
        };
        match authority.process(client, &request, now) {
            Decision::Reply(reply) => reply,
            _ => panic!("Hello must not execute runtime work"),
        }
    }
    fn request(authority: &Authority, id: u64, grant: u64, command: Command) -> Request {
        Request {
            id,
            owner: Some(authority.owner().clone()),
            grant_id: Some(grant),
            command,
        }
    }
    fn immediate(
        authority: &mut Authority,
        client: ClientId,
        request: &Request,
        now: Instant,
    ) -> Response {
        match authority.process(client, request, now) {
            Decision::Reply(reply) => reply,
            _ => panic!("unexpected runtime execution"),
        }
    }
    fn granted(
        authority: &mut Authority,
        id: u64,
        capabilities: Vec<Capability>,
        ttl_seconds: u64,
        now: Instant,
    ) -> (u64, Token) {
        let request = request(
            authority,
            id,
            0,
            Command::Grant {
                capabilities,
                ttl_seconds,
            },
        );
        match immediate(authority, 1, &request, now).reply {
            Reply::Granted {
                grant_id, token, ..
            } => (grant_id, token),
            _ => panic!("expected scoped grant"),
        }
    }
    fn refused(response: Response, expected: ErrorCode) {
        assert!(matches!(response.reply, Reply::Error { code, .. } if code == expected));
    }

    #[test]
    fn test_authority_scopes_controller_expiry_and_revocation() {
        let now = Instant::now();
        let mut auth = authority();
        let admin = auth.admin_token();
        assert!(matches!(
            hello(&mut auth, 1, admin, true, now).reply,
            Reply::Welcome {
                admin: true,
                controller: true,
                grant_id: 0,
                ..
            }
        ));
        let (reader_id, reader) = granted(&mut auth, 1, vec![Capability::Read], 10, now);
        assert!(matches!(
            hello(&mut auth, 2, reader, false, now).reply,
            Reply::Welcome { admin: false, .. }
        ));
        let read = request(
            &auth,
            999,
            reader_id,
            Command::Snapshot {
                after_sequence: None,
            },
        );
        assert!(matches!(
            auth.process(2, &read, now),
            Decision::Execute { .. }
        ));
        assert_eq!(auth.complete(2, &read, Reply::Ok, now).last_mutation_id, 0);
        for command in [
            Command::ClaimControl,
            Command::Resize {
                cols: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 16,
            },
            Command::Input {
                pane: 0,
                data: vec![1],
            },
            Command::Spawn {
                program: "/bin/true".into(),
                args: vec![],
                cwd: None,
            },
            Command::Grant {
                capabilities: vec![Capability::Read],
                ttl_seconds: 1,
            },
            Command::Terminate,
        ] {
            let request = request(&auth, 1, reader_id, command);
            refused(
                immediate(&mut auth, 2, &request, now),
                ErrorCode::PermissionDenied,
            );
        }
        let (input_id, input) = granted(&mut auth, 2, vec![Capability::Input], 1, now);
        refused(
            hello(&mut auth, 3, input.clone(), true, now),
            ErrorCode::Busy,
        );
        assert!(!auth.is_authenticated(3));
        auth.disconnect(1);
        assert!(matches!(
            hello(&mut auth, 3, input.clone(), true, now).reply,
            Reply::Welcome {
                controller: true,
                ..
            }
        ));
        assert_eq!(auth.controller(), Some(3));
        let expired = auth.expire(now + Duration::from_secs(1));
        assert!(expired.contains(&3));
        assert_eq!(auth.controller(), None);
        let input_request = request(
            &auth,
            1,
            input_id,
            Command::Input {
                pane: 0,
                data: vec![1],
            },
        );
        refused(
            immediate(&mut auth, 3, &input_request, now + Duration::from_secs(1)),
            ErrorCode::Unauthorized,
        );
        refused(
            hello(&mut auth, 4, input, false, now + Duration::from_secs(1)),
            ErrorCode::Unauthorized,
        );
        let admin = auth.admin_token();
        hello(&mut auth, 1, admin, false, now);
        let revoke = request(
            &auth,
            3,
            0,
            Command::Revoke {
                grant_id: reader_id,
            },
        );
        assert!(matches!(
            immediate(&mut auth, 1, &revoke, now).reply,
            Reply::Ok
        ));
        assert!(auth.expire(now).contains(&2));
        refused(immediate(&mut auth, 2, &read, now), ErrorCode::Unauthorized);
    }

    #[test]
    fn test_mutation_receipts_fence_owner_grant_payload_and_failure() {
        let now = Instant::now();
        let mut auth = authority();
        let admin = auth.admin_token();
        hello(&mut auth, 1, admin, true, now);
        let operation = request(
            &auth,
            1,
            0,
            Command::Spawn {
                program: "/bin/true".into(),
                args: vec![],
                cwd: None,
            },
        );
        assert!(matches!(
            auth.process(1, &operation, now),
            Decision::Execute {
                id: 1,
                grant_id: 0,
                ..
            }
        ));
        auth.complete(
            1,
            &operation,
            Reply::Applied {
                pane: Some(7),
                tab: Some(2),
                warning: None,
            },
            now,
        );
        assert!(matches!(
            immediate(&mut auth, 1, &operation, now).reply,
            Reply::Applied { pane: Some(7), .. }
        ));
        let changed = Request {
            command: Command::Input {
                pane: 7,
                data: vec![2],
            },
            ..operation.clone()
        };
        refused(immediate(&mut auth, 1, &changed, now), ErrorCode::Conflict);
        let wrong_grant = Request {
            grant_id: Some(77),
            ..operation.clone()
        };
        refused(
            immediate(&mut auth, 1, &wrong_grant, now),
            ErrorCode::StaleOwner,
        );
        let wrong_owner = Request {
            owner: Some(OwnerRef {
                pid: 42,
                epoch: "b".repeat(64),
            }),
            ..operation.clone()
        };
        refused(
            immediate(&mut auth, 1, &wrong_owner, now),
            ErrorCode::StaleOwner,
        );
        let future = Request {
            id: 3,
            ..operation.clone()
        };
        refused(immediate(&mut auth, 1, &future, now), ErrorCode::OutOfOrder);
        let failed = Request { id: 2, ..operation };
        assert!(matches!(
            auth.process(1, &failed, now),
            Decision::Execute { .. }
        ));
        assert_eq!(
            auth.complete(
                1,
                &failed,
                Reply::error(ErrorCode::Failed, "exec failure"),
                now
            )
            .last_mutation_id,
            2
        );
        refused(immediate(&mut auth, 1, &failed, now), ErrorCode::Failed);
        refused(
            immediate(&mut auth, 1, &failed, now + RECEIPT_TTL),
            ErrorCode::UnknownOperation,
        );
        assert_eq!(auth.grants[&0].watermark, 2);
    }

    #[test]
    fn test_evicted_receipt_never_reexecutes_or_renews_grant() {
        let now = Instant::now();
        let mut auth = authority();
        let admin = auth.admin_token();
        hello(&mut auth, 1, admin, false, now);
        let original = request(
            &auth,
            1,
            0,
            Command::Grant {
                capabilities: vec![Capability::Read],
                ttl_seconds: 2,
            },
        );
        let issued = immediate(&mut auth, 1, &original, now);
        let Reply::Granted {
            grant_id, token, ..
        } = issued.reply
        else {
            panic!("expected grant");
        };
        let replay = immediate(&mut auth, 1, &original, now + Duration::from_secs(1));
        assert!(matches!(replay.reply, Reply::Granted { token: same, .. } if same == token));
        assert!(auth.expire(now + Duration::from_secs(2)).is_empty());
        assert!(!auth.grants.contains_key(&grant_id));
        for id in 2..=MAX_RECEIPTS as u64 + 2 {
            let revoke = request(&auth, id, 0, Command::Revoke { grant_id: u64::MAX });
            refused(
                immediate(&mut auth, 1, &revoke, now + Duration::from_secs(2)),
                ErrorCode::InvalidRequest,
            );
        }
        assert!(auth.receipts.len() <= MAX_RECEIPTS && auth.receipt_bytes <= MAX_RECEIPT_BYTES);
        refused(
            immediate(&mut auth, 1, &original, now + Duration::from_secs(2)),
            ErrorCode::UnknownOperation,
        );
        assert_eq!(auth.grants.len(), 1);
    }
}
