//! Explicit SSH stdio attachment. No public listener, installation, enrollment,
//! privileged credential forwarding or permission inferred from diagnostics.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::transport::{self, AttachmentClient};
use super::wire::{self, Command, Reply, Request};

/// Fixed remote program. No caller-controlled argument enters a remote shell.
pub const REMOTE_HELPER: &str = "ultty-session attach-stdio";

/// Validate an SSH destination as one argument, excluding options and shell text.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-@:%[]".contains(&byte))
}

/// Build exact SSH argv. Authentication is noninteractive and credentials travel
/// only in the framed Hello, never argv or forwarded local environment.
pub fn ssh_command(host: &str) -> io::Result<ProcessCommand> {
    if !valid_host(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH destination",
        ));
    }
    let mut command = ProcessCommand::new("ssh");
    command.args([
        "-T",
        "-oBatchMode=yes",
        "-oConnectTimeout=5",
        "-oServerAliveInterval=5",
        "-oServerAliveCountMax=1",
        "-oForwardAgent=no",
        "-oPermitLocalCommand=no",
        "--",
        host,
        REMOTE_HELPER,
    ]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for name in [
        "ULTTY_AUTH_TOKEN",
        "ULTTY_SOCKET",
        "ULTTY_PID",
        "ULTTY_OWNER_EPOCH",
    ] {
        command.env_remove(name);
    }
    Ok(command)
}

/// Connect using an explicit scoped credential. This never reads a local owner
/// token and never starts a session host automatically.
pub fn connect_ssh(
    host: &str,
    session: &str,
    token: &str,
    controller: bool,
) -> io::Result<AttachmentClient> {
    if !wire::valid_session_name(session) || !scoped_token(token) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH requires a valid session and scoped token",
        ));
    }
    let mut client =
        AttachmentClient::from_ssh_child(ssh_command(host)?.spawn()?, session, token, controller)?;
    client.set_ssh_reconnect_target(host);
    Ok(client)
}

fn scoped_token(token: &str) -> bool {
    token.len() <= 128
        && token
            .split_once('.')
            .and_then(|(id, _)| id.parse::<u64>().ok())
            .is_some_and(|id| id != 0)
}

fn read_optional(input: &mut impl Read) -> io::Result<Option<Request>> {
    let mut first = [0];
    loop {
        match input.read(&mut first) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let mut framed = first.as_slice().chain(input);
    wire::read(&mut framed, wire::MAX_REQUEST_BYTES).map(Some)
}

/// Fixed remote helper entrypoint. The first scoped Hello selects a validated
/// named private endpoint; no remote filesystem path or local admin token is used.
pub fn serve_stdio(mut input: impl Read, mut output: impl Write) -> io::Result<()> {
    let Some(first) = read_optional(&mut input)? else {
        return Ok(());
    };
    first.command.validate()?;
    let Command::Hello { session, token, .. } = &first.command else {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "first frame must be scoped Hello",
        ));
    };
    if !scoped_token(token.expose_secret()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "admin credentials cannot cross the SSH helper",
        ));
    }
    let mut stream = UnixStream::connect(transport::session_endpoint(session)?)?;
    let response = transport::exchange_local(&mut stream, &first)?;
    let accepted = matches!(response.reply, Reply::Welcome { admin: false, .. });
    wire::write(&mut output, &response, wire::MAX_RESPONSE_BYTES)?;
    if !accepted {
        return Ok(());
    }
    while let Some(request) = read_optional(&mut input)? {
        if matches!(request.command, Command::Hello { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "repeated Hello refused",
            ));
        }
        let response = transport::exchange_local(&mut stream, &request)?;
        wire::write(&mut output, &response, wire::MAX_RESPONSE_BYTES)?;
    }
    Ok(())
}

/// Bounded read-only process diagnostic; it never grants attachment authority.
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    /// Fixed command arguments, excluding credentials.
    pub command: Vec<String>,
    /// Whether the executable could be started.
    pub available: bool,
    /// Whether it exited successfully within the byte/time budgets.
    pub success: bool,
    /// Whether the short wall deadline expired.
    pub timed_out: bool,
    /// At most 256 KiB of stdout.
    pub stdout: String,
    /// At most 32 KiB of stderr.
    pub stderr: String,
}

/// Inspect SSH configuration/executable availability and optionally local
/// Tailscale status. No connection, installation or enrollment is attempted.
pub fn diagnostics(host: Option<&str>, tailscale: bool) -> io::Result<Vec<Diagnostic>> {
    let mut checks = Vec::new();
    let arguments = if let Some(host) = host {
        if !valid_host(host) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SSH destination",
            ));
        }
        vec!["-G", "--", host]
    } else {
        vec!["-V"]
    };
    checks.push(check_command("ssh", &arguments)?);
    if tailscale {
        checks.push(check_command("tailscale", &["status", "--json"])?);
    }
    Ok(checks)
}

fn drain(reader: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut chunk = [0; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if count > limit.saturating_sub(bytes.len()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "diagnostic output exceeds limit",
                    ));
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn check_command(program: &str, args: &[&str]) -> io::Result<Diagnostic> {
    let description = std::iter::once(program)
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    let mut child = match ProcessCommand::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Diagnostic {
                command: description,
                available: false,
                success: false,
                timed_out: false,
                stdout: String::new(),
                stderr: error.to_string(),
            })
        }
        Err(error) => return Err(error),
    };
    let result: io::Result<_> = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("diagnostic stdout unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("diagnostic stderr unavailable"))?;
        transport::set_nonblocking(stdout.as_raw_fd())?;
        transport::set_nonblocking(stderr.as_raw_fd())?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let out_done = drain(&mut stdout, &mut out, 256 * 1024)?;
            let err_done = drain(&mut stderr, &mut err, 32 * 1024)?;
            let status = child.try_wait()?;
            if let Some(status) = status.filter(|_| out_done && err_done) {
                return Ok((status.success(), false, out, err));
            }
            if Instant::now() >= deadline {
                return Ok((false, true, out, err));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if child.try_wait()?.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let (success, timed_out, stdout, stderr) = result?;
    Ok(Diagnostic {
        command: description,
        available: true,
        success,
        timed_out,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_ssh_argv_is_fixed_and_never_carries_tokens() {
        let command = ssh_command("user@host-name").unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[args.len() - 3..],
            ["--", "user@host-name", REMOTE_HELPER]
        );
        assert!(command
            .get_envs()
            .any(|(name, value)| name == "ULTTY_AUTH_TOKEN" && value.is_none()));
        for host in [
            "-oProxyCommand=bad",
            "host; touch bad",
            "bad\nhost",
            "host/path",
            "",
        ] {
            assert!(ssh_command(host).is_err());
        }
        assert!(!scoped_token("0.secret"));
        assert!(!scoped_token("00.secret"));
        assert!(scoped_token("1.secret"));
    }
    #[test]
    fn test_stdio_refuses_privileged_hello_before_opening_any_endpoint() {
        let request = Request {
            id: 0,
            owner: None,
            grant_id: None,
            command: Command::Hello {
                version: wire::VERSION,
                session: "main".into(),
                token: wire::Token::new("0.secret"),
                controller: false,
            },
        };
        let frame = wire::encode(&request, wire::MAX_REQUEST_BYTES).unwrap();
        assert!(serve_stdio(frame.as_slice(), Vec::new()).is_err());
    }
}
