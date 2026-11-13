//! Shared authentication helpers for local control endpoints.

use std::io;
use std::path::Path;

/// Random bytes in one control credential.
pub const AUTH_TOKEN_BYTES: usize = 32;

/// Generate a fresh random control auth token and write it to `path`.
///
/// The file is created with mode 0600 (and its parent directory 0700 when
/// it has to be created), so only the owning user can talk to the control
/// socket. Called once per control-server start; a running instance owns
/// the socket, so rotating the token on start never races another instance.
pub fn write_auth_token(path: &Path) -> io::Result<String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let mut random = [0u8; AUTH_TOKEN_BYTES];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
    }

    // Replace atomically-ish: truncate an existing file rather than leaving
    // a stale token readable while we write.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies on creation; enforce it for pre-existing files too.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(token)
}

/// Read the expected control auth token from `path`.
pub fn read_auth_token(path: &Path) -> io::Result<String> {
    let token = std::fs::read_to_string(path)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "auth token file is empty",
        ));
    }
    Ok(token.to_string())
}

/// Compare a presented token with the expected one without early exit on
/// the first differing byte.
pub fn token_matches(presented: &str, expected: &str) -> bool {
    let presented = presented.trim().as_bytes();
    let expected = expected.as_bytes();
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Shared control-plane helper.
pub fn control_request_requires_auth(request: &ultty_protocol::control::ControlRequest) -> bool {
    use ultty_protocol::control::ControlRequest;

    match request {
        // Harmless liveness/version probes and the auth handshake itself.
        ControlRequest::Ping
        | ControlRequest::Version
        | ControlRequest::Authenticate { .. }
        | ControlRequest::WithToken { .. }
        | ControlRequest::WithOwner { .. } => false,
        // Batch members are checked individually when they run.
        ControlRequest::Batch { .. } => false,
        // Everything that reads screen content or drives the terminal.
        ControlRequest::GetInfo
        | ControlRequest::Describe
        | ControlRequest::SubscribeV2 { .. }
        | ControlRequest::ListTabs
        | ControlRequest::ListPanes
        | ControlRequest::WorkspaceList
        | ControlRequest::WorkspaceSave { .. }
        | ControlRequest::GetText { .. }
        | ControlRequest::GetSelection { .. }
        | ControlRequest::LastCommand { .. }
        | ControlRequest::Regions { .. }
        | ControlRequest::InvokeAction { .. }
        | ControlRequest::Search { .. }
        | ControlRequest::WaitFor { .. }
        | ControlRequest::Subscribe { .. }
        | ControlRequest::PushOverride { .. }
        | ControlRequest::ClearOverride { .. }
        | ControlRequest::SendText { .. }
        | ControlRequest::SendKey { .. }
        | ControlRequest::SendRaw { .. }
        | ControlRequest::NewTab { .. }
        | ControlRequest::SplitPane { .. }
        | ControlRequest::SwitchTab { .. }
        | ControlRequest::CloseTab { .. }
        | ControlRequest::RunCommand { .. }
        | ControlRequest::RunCommandAndWait { .. }
        | ControlRequest::JobStatus { .. }
        | ControlRequest::WaitJob { .. }
        | ControlRequest::ProgramStatus { .. }
        | ControlRequest::FocusPane { .. }
        | ControlRequest::ZoomPane { .. }
        | ControlRequest::SwapPanes { .. }
        | ControlRequest::FocusWindow
        | ControlRequest::ToggleFullscreen
        | ControlRequest::Resize { .. }
        | ControlRequest::SetOpacity { .. }
        | ControlRequest::SetPaneStatus { .. }
        | ControlRequest::Screenshot { .. } => true,
    }
}
