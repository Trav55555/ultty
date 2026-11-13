//! Bounded, versioned viewer/host messages, distinct from native app and control.
use std::fmt;
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use ultty_protocol::control::{OwnerRef, SplitDirection};
use ultty_protocol::{Decode, InputEvent, TerminalMessage};

use super::model::SessionSnapshot;

/// Attachment schema version.
pub const VERSION: u16 = 1;
/// Independent framing magic, followed by a little-endian u32 body length.
pub const MAGIC: [u8; 4] = *b"UAT1";
/// Maximum encoded client body.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Maximum encoded host body.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum UTF-8 error explanation.
pub const MAX_ERROR_BYTES: usize = 4096;

/// Explicit bearer credential; Debug output never contains the secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);
impl Token {
    /// Wrap an explicitly supplied credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// Expose the credential only for explicit authentication or writing.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Token {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("Token(<redacted>)")
    }
}

/// Delegated capabilities never confer owner administration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read complete owner snapshots.
    Read,
    /// Send PTY/native input with the controller lease.
    Input,
    /// Launch processes with the controller lease.
    Spawn,
    /// Change canonical layout/viewport with the controller lease.
    Layout,
}

/// Exact layout mutations; swaps stay within one tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayoutCommand {
    /// Focus a pane and its tab.
    Focus { pane: u64 },
    /// Create a sibling with exact argv, or a shell without launch options.
    Split {
        pane: u64,
        direction: SplitDirection,
        program: Option<String>,
        args: Vec<String>,
        cwd: Option<String>,
    },
    /// Explicitly close/terminate one pane.
    Close { pane: u64 },
    /// Set zoom while preserving split ratios.
    Zoom { pane: u64, zoomed: bool },
    /// Exchange two panes in one tab.
    Swap { first: u64, second: u64 },
}

/// One typed host operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Authenticate to a validated named session before other operations.
    Hello {
        version: u16,
        session: String,
        token: Token,
        controller: bool,
    },
    /// Full replacement; an old/equal sequence does not imply unchanged state.
    Snapshot { after_sequence: Option<u64> },
    /// Queue literal PTY bytes.
    Input { pane: u64, data: Vec<u8> },
    /// Exactly one native TerminalMessage::Input frame.
    NativeInput {
        session: u64,
        generation: u64,
        payload: Vec<u8>,
    },
    /// Change the canonical viewport.
    Resize {
        /// Canonical grid columns.
        cols: u16,
        /// Canonical grid rows.
        rows: u16,
        /// Controlling viewer's measured cell width in pixels.
        #[serde(default = "default_cell_width")]
        cell_width: u16,
        /// Controlling viewer's measured cell height in pixels.
        #[serde(default = "default_cell_height")]
        cell_height: u16,
    },
    /// Launch a child with exact argv.
    Spawn {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
    },
    /// Change the owner's pane layout.
    Layout { command: LayoutCommand },
    /// Claim a vacant lease, never steal it.
    ClaimControl,
    /// Release this connection's lease.
    ReleaseControl,
    /// Owner-admin only: delegate bounded expiring authority.
    Grant {
        capabilities: Vec<Capability>,
        ttl_seconds: u64,
    },
    /// Owner-admin only: revoke a grant and disconnect its clients.
    Revoke { grant_id: u64 },
    /// Owner-admin only: stop the host and its children.
    Terminate,
}
impl Command {
    /// Whether this consumes a grant-scoped mutation sequence.
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::Hello { .. } | Self::Snapshot { .. })
    }
    /// Capability required independently of the controller lease.
    pub fn capability(&self) -> Option<Capability> {
        match self {
            Self::Snapshot { .. } => Some(Capability::Read),
            Self::Input { .. } | Self::NativeInput { .. } => Some(Capability::Input),
            Self::Spawn { .. } => Some(Capability::Spawn),
            Self::Resize { .. } | Self::Layout { .. } => Some(Capability::Layout),
            _ => None,
        }
    }
    /// Whether this mutates canonical interaction and needs the lease.
    pub fn needs_controller(&self) -> bool {
        matches!(
            self,
            Self::Input { .. }
                | Self::NativeInput { .. }
                | Self::Spawn { .. }
                | Self::Resize { .. }
                | Self::Layout { .. }
        )
    }
    /// Reject malformed input before reserving a mutation ID.
    pub fn validate(&self) -> io::Result<()> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
        match self {
            Self::Hello {
                version,
                session,
                token,
                ..
            } => {
                if *version != VERSION {
                    return Err(invalid("unsupported attachment version"));
                }
                if !valid_session_name(session) || token.expose_secret().len() > 128 {
                    return Err(invalid("invalid session/token"));
                }
            }
            Self::Resize {
                cols,
                rows,
                cell_width,
                cell_height,
            } => {
                if *cols == 0
                    || *rows == 0
                    || !(1..=512).contains(cell_width)
                    || !(1..=512).contains(cell_height)
                    || u32::from(*cols) * u32::from(*cell_width) > u32::from(u16::MAX)
                    || u32::from(*rows) * u32::from(*cell_height) > u32::from(u16::MAX)
                {
                    return Err(invalid("invalid grid or controlling cell pixel metrics"));
                }
            }
            Self::NativeInput { payload, .. } => {
                decode_native_input(payload)?;
            }
            Self::Spawn { program, args, cwd } => validate_launch(program, args, cwd.as_deref())?,
            Self::Layout {
                command:
                    LayoutCommand::Split {
                        program, args, cwd, ..
                    },
            } => {
                if let Some(program) = program {
                    validate_launch(program, args, cwd.as_deref())?;
                } else if !args.is_empty() || cwd.is_some() {
                    return Err(invalid("args/cwd require a split program"));
                }
            }
            Self::Grant {
                capabilities,
                ttl_seconds,
            } => {
                if capabilities.is_empty()
                    || capabilities.len() > 4
                    || *ttl_seconds == 0
                    || *ttl_seconds > 86400
                {
                    return Err(invalid(
                        "grant requires capabilities and TTL 1..86400 seconds",
                    ));
                }
                for (index, capability) in capabilities.iter().enumerate() {
                    if capabilities[..index].contains(capability) {
                        return Err(invalid("duplicate capability"));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn default_cell_width() -> u16 {
    8
}
fn default_cell_height() -> u16 {
    16
}

fn validate_launch(program: &str, args: &[String], cwd: Option<&str>) -> io::Result<()> {
    if program.is_empty()
        || program.len() > 4096
        || program.contains('\0')
        || args.len() > 256
        || args
            .iter()
            .any(|arg| arg.len() > 65536 || arg.contains('\0'))
        || cwd.is_some_and(|value| value.len() > 4096 || value.contains('\0'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid structured launch",
        ));
    }
    Ok(())
}
/// Names cannot select arbitrary filesystem paths.
pub fn valid_session_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}
/// Decode exactly one native input event; other messages/trailing bytes fail.
pub fn decode_native_input(payload: &[u8]) -> io::Result<InputEvent> {
    let mut remaining = payload;
    let message = TerminalMessage::decode(&mut remaining)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    match message {
        TerminalMessage::Input(input) if remaining.is_empty() => Ok(input),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected exactly one native input frame",
        )),
    }
}

/// Reads use correlation IDs; accepted mutations use the next grant sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Correlation ID or exact next mutation sequence.
    pub id: u64,
    /// Mandatory exact owner after Hello.
    pub owner: Option<OwnerRef>,
    /// Mandatory exact authenticated grant after Hello.
    pub grant_id: Option<u64>,
    /// Operation.
    pub command: Command,
}
/// Structured refusals; unknown outcomes must never be retried automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    PermissionDenied,
    StaleOwner,
    Busy,
    InvalidRequest,
    InvalidVersion,
    UnknownOperation,
    Conflict,
    OutOfOrder,
    Expired,
    Capacity,
    Failed,
}
/// One reply payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// No display-relevant authoritative revision changed since this sequence.
    /// Hosts may send this only with complete revision coverage, never from
    /// lossy redraw notifications alone.
    Unchanged { sequence: u64 },
    /// Authenticated authority and controller status.
    Welcome {
        /// Grant identity to pin on subsequent operations.
        grant_id: u64,
        version: u16,
        capabilities: Vec<Capability>,
        admin: bool,
        controller: bool,
    },
    /// Complete replacement state.
    Snapshot { snapshot: Box<SessionSnapshot> },
    /// Returned owner-scoped identity, when applicable.
    Applied {
        pane: Option<u64>,
        tab: Option<u64>,
        /// A completed mutation may have a follow-up layout warning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warning: Option<String>,
    },
    /// New credential; receipt replay never renews its expiry.
    Granted {
        grant_id: u64,
        token: Token,
        capabilities: Vec<Capability>,
        ttl_seconds: u64,
    },
    /// Accepted mutation with no additional data.
    Ok,
    /// Explicit refusal or unknown receipt.
    Error { code: ErrorCode, message: String },
}
impl Reply {
    /// Construct an error with a bounded UTF-8 explanation.
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_ERROR_BYTES {
            let mut end = MAX_ERROR_BYTES;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self::Error { code, message }
    }
}
/// All replies carry the current authoritative mutation watermark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Echoed request ID.
    pub id: u64,
    /// Exact owner lifetime, including on stale-owner refusals.
    pub owner: OwnerRef,
    /// Highest reserved mutation sequence for this grant.
    pub last_mutation_id: u64,
    /// Payload.
    pub reply: Reply,
}
struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "attachment frame exceeds limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Serialize without accumulating an oversized body.
pub fn encode<T: Serialize>(value: &T, limit: usize) -> io::Result<Vec<u8>> {
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
    let length = u32::try_from(writer.bytes.len()).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(writer.bytes.len() + 8);
    frame.extend_from_slice(&MAGIC);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&writer.bytes);
    Ok(frame)
}
/// Validate a fixed header before allocating its body.
pub fn body_length(header: &[u8; 8], limit: usize) -> io::Result<usize> {
    if header[..4] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid attachment magic",
        ));
    }
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if length == 0 || length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid attachment frame length",
        ));
    }
    Ok(length)
}
/// Read one frame; malformed/partial input is never resynchronized.
pub fn read<T: serde::de::DeserializeOwned>(reader: &mut impl Read, limit: usize) -> io::Result<T> {
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let length = body_length(&header, limit)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
/// Write one frame; a possibly partial mutation must not be auto-retried.
pub fn write<T: Serialize>(writer: &mut impl Write, value: &T, limit: usize) -> io::Result<()> {
    writer.write_all(&encode(value, limit)?)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_resize_pixel_metrics_defaults_and_overflow_boundary() {
        let legacy: Command =
            serde_json::from_str(r#"{"type":"resize","cols":80,"rows":24}"#).unwrap();
        assert_eq!(
            legacy,
            Command::Resize {
                cols: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 16
            }
        );
        assert!(legacy.validate().is_ok());
        for (cols, rows, cell_width, cell_height) in [
            (0, 24, 8, 16),
            (80, 24, 0, 16),
            (80, 24, 8, 513),
            (u16::MAX, 24, 2, 16),
            (80, u16::MAX, 8, 2),
        ] {
            assert!(Command::Resize {
                cols,
                rows,
                cell_width,
                cell_height
            }
            .validate()
            .is_err());
        }
    }
    #[test]
    fn test_framing_is_bounded_and_credentials_redacted() {
        let token = Token::new("credential");
        assert!(!format!("{token:?}").contains("credential"));
        let value = Command::Hello {
            version: VERSION,
            session: "main".into(),
            token,
            controller: false,
        };
        let frame = encode(&value, MAX_REQUEST_BYTES).unwrap();
        assert_eq!(
            read::<Command>(&mut frame.as_slice(), MAX_REQUEST_BYTES).unwrap(),
            value
        );
        assert!(read::<Command>(&mut &frame[..7], MAX_REQUEST_BYTES).is_err());
        assert!(encode(&"x".repeat(100), 50).is_err());
        assert!(body_length(
            &[b'U', b'A', b'T', b'1', 255, 255, 255, 127],
            MAX_REQUEST_BYTES
        )
        .is_err());
        assert!(decode_native_input(&[]).is_err());
        for name in ["", "../escape", "-option", "/path", "bad name"] {
            assert!(!valid_session_name(name));
        }
    }
}
