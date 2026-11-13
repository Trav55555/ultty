//! Local persistent session owner and fixed remote attachment helper.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ultty::attach::auth::{new_owner, Authority};
use ultty::attach::control::HostControl;
use ultty::attach::host::{serve, HostState};
use ultty::attach::transport::{self, AttachmentClient, Server};
use ultty::attach::wire::{self, Capability, Command, Reply, Response};
use ultty::platform::socket::{ensure_private_dir, private_socket_dir};
use ultty::protocol::server::ProtocolServer;
use ultty::runtime::SessionRuntime;
use ultty::terminal_manager::TerminalManager;

struct NativeDirectory(PathBuf);
impl Drop for NativeDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir(&self.0) {
            tracing::warn!(%error, "could not remove native session directory");
        }
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_writer(io::stderr).init();
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(
            command @ ("grant" | "revoke" | "stop" | "snapshot" | "client" | "input"
            | "diagnostics"),
        ) => operational(command, &args.collect::<Vec<_>>()),
        Some("attach-stdio") => {
            if args.next().is_some() {
                anyhow::bail!("usage: ultty-session attach-stdio");
            }
            ultty::attach::remote::serve_stdio(io::stdin().lock(), io::stdout().lock())?;
            Ok(())
        }
        Some("host") => {
            let session = args.next().ok_or_else(|| {
                anyhow::anyhow!("usage: ultty-session host NAME [-e PROGRAM ARGS...]")
            })?;
            let remaining: Vec<_> = args.collect();
            let (program, arguments) = if remaining.is_empty() {
                (
                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                    Vec::new(),
                )
            } else if remaining.len() >= 2 && remaining[0] == "-e" {
                (remaining[1].clone(), remaining[2..].to_vec())
            } else {
                anyhow::bail!("usage: ultty-session host NAME [-e PROGRAM ARGS...]");
            };
            let server = Server::bind(&session)?;
            let owner = new_owner()?;
            let authority = Authority::new(&session, owner.clone())?;
            let control = HostControl::new(owner.clone())?;
            let base = private_socket_dir(&std::env::temp_dir())?;
            ensure_private_dir(&base)?;
            let directory = NativeDirectory(base.join(format!("un-{}", &owner.epoch[..16])));
            ensure_private_dir(&directory.0)?;
            let protocol = ProtocolServer::new(&directory.0)?;
            let runtime =
                SessionRuntime::new(TerminalManager::empty(80, 24), Some(protocol), (80, 24))?;
            let mut host = HostState::new(runtime, owner, [80, 24])?;
            match host.execute(&Command::Spawn {
                program,
                args: arguments,
                cwd: None,
            }) {
                Reply::Applied { .. } => serve(host, server, authority, control)?,
                Reply::Error { message, .. } => anyhow::bail!("could not start session: {message}"),
                _ => anyhow::bail!("unexpected initial session result"),
            }
            Ok(())
        }
        _ => anyhow::bail!("{USAGE}"),
    }
}

const USAGE: &str = "usage:
  ultty-session host NAME [-e PROGRAM ARGS...]
  ultty-session grant NAME --caps read,input,spawn,layout --ttl SECONDS --out FILE
  ultty-session revoke NAME GRANT_ID
  ultty-session stop NAME
  ultty-session snapshot NAME [--token-file FILE] [--ssh HOST]
  ultty-session input NAME --token-file FILE --pane ID --text TEXT [--ssh HOST]
  ultty-session client NAME --token-file FILE [--controller] [--ssh HOST]
    Persistent NDJSON Commands on stdin, Responses on stdout.
    claim_control/release_control change this connection's lease.
  ultty-session diagnostics [--ssh HOST] [--tailscale]
  ultty-session attach-stdio";

fn flags(
    args: &[String],
    values: &[&str],
    switches: &[&str],
) -> anyhow::Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    let mut index = 0;
    while index < args.len() {
        let name = &args[index];
        let value = if switches.contains(&name.as_str()) {
            "true".to_string()
        } else if values.contains(&name.as_str()) {
            index += 1;
            args.get(index)
                .ok_or_else(|| anyhow::anyhow!("{name} requires a value"))?
                .clone()
        } else {
            anyhow::bail!("unknown argument {name}\n{USAGE}");
        };
        if result.insert(name.clone(), value).is_some() {
            anyhow::bail!("duplicate argument {name}");
        }
        index += 1;
    }
    Ok(result)
}

fn read_token_file(path: &Path) -> anyhow::Result<String> {
    let mut value = String::new();
    File::open(path)?.take(129).read_to_string(&mut value)?;
    if value.len() > 128 || value.trim().is_empty() {
        anyhow::bail!("invalid credential file");
    }
    Ok(value.trim().into())
}

fn connect(
    session: &str,
    options: &HashMap<String, String>,
    controller: bool,
    require_file: bool,
) -> anyhow::Result<AttachmentClient> {
    let token = if let Some(path) = options.get("--token-file") {
        read_token_file(Path::new(path))?
    } else {
        if require_file || options.contains_key("--ssh") {
            anyhow::bail!("an explicit --token-file is required");
        }
        transport::read_admin_token(session)?
            .expose_secret()
            .to_string()
    };
    if let Some(host) = options.get("--ssh") {
        Ok(ultty::attach::remote::connect_ssh(
            host, session, &token, controller,
        )?)
    } else {
        Ok(AttachmentClient::connect(session, &token, controller)?)
    }
}

fn print_response(response: &Response) -> anyhow::Result<()> {
    write_response(response)?;
    if let Reply::Error { code, message } = &response.reply {
        anyhow::bail!("{code:?}: {message}");
    }
    Ok(())
}
fn write_response(response: &Response) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, response)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn write_token_atomic(path: &Path, value: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_extension(format!("tmp.{}.{stamp}", std::process::id()));
    let result = (|| -> anyhow::Result<()> {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        output.write_all(value.as_bytes())?;
        output.write_all(b"\n")?;
        output.sync_all()?;
        // Atomic no-clobber publication. Existing files and symlinks fail.
        fs::hard_link(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = fs::remove_file(&temporary) {
        if error.kind() != io::ErrorKind::NotFound {
            tracing::warn!(%error, "could not remove temporary credential file");
        }
    }
    result
}

fn operational(command: &str, args: &[String]) -> anyhow::Result<()> {
    if command == "diagnostics" {
        let options = flags(args, &["--ssh"], &["--tailscale"])?;
        let checks = ultty::attach::remote::diagnostics(
            options.get("--ssh").map(String::as_str),
            options.contains_key("--tailscale"),
        )?;
        serde_json::to_writer(io::stdout().lock(), &checks)?;
        println!();
        return Ok(());
    }
    let session = args.first().ok_or_else(|| anyhow::anyhow!("{USAGE}"))?;
    let rest = &args[1..];
    match command {
        "grant" => {
            let options = flags(rest, &["--caps", "--ttl", "--out"], &[])?;
            let caps = options
                .get("--caps")
                .ok_or_else(|| anyhow::anyhow!("--caps is required"))?;
            let capabilities = caps
                .split(',')
                .map(|value| match value {
                    "read" => Ok(Capability::Read),
                    "input" => Ok(Capability::Input),
                    "spawn" => Ok(Capability::Spawn),
                    "layout" => Ok(Capability::Layout),
                    _ => Err(anyhow::anyhow!(
                        "capability must be read,input,spawn,layout"
                    )),
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let ttl_seconds = options
                .get("--ttl")
                .ok_or_else(|| anyhow::anyhow!("--ttl is required"))?
                .parse::<u64>()?;
            let path = PathBuf::from(
                options
                    .get("--out")
                    .ok_or_else(|| anyhow::anyhow!("--out is required"))?,
            );
            if fs::symlink_metadata(&path).is_ok() {
                anyhow::bail!("credential output already exists; choose a fresh --out path");
            }
            let mut client = connect(session, &HashMap::new(), false, false)?;
            let response = client.request(Command::Grant {
                capabilities,
                ttl_seconds,
            })?;
            let Reply::Granted {
                grant_id,
                token,
                capabilities,
                ttl_seconds,
            } = response.reply
            else {
                return print_response(&response);
            };
            if let Err(error) = write_token_atomic(&path, token.expose_secret()) {
                match client.request(Command::Revoke { grant_id }) {
                    Ok(response) if matches!(response.reply, Reply::Ok) => {}
                    result => tracing::warn!(
                        grant_id,
                        ?result,
                        "credential write failed; revocation outcome needs inspection"
                    ),
                }
                return Err(error);
            }
            // Credential bytes are deliberately omitted from stdout/logs.
            serde_json::to_writer(
                io::stdout().lock(),
                &serde_json::json!({
                    "grant_id": grant_id, "capabilities": capabilities, "ttl_seconds": ttl_seconds, "token_file": path,
                }),
            )?;
            println!();
            Ok(())
        }
        "revoke" => {
            let [id] = rest else {
                anyhow::bail!("usage: ultty-session revoke NAME GRANT_ID");
            };
            let mut client = connect(session, &HashMap::new(), false, false)?;
            print_response(&client.request(Command::Revoke {
                grant_id: id.parse()?,
            })?)
        }
        "stop" => {
            if !rest.is_empty() {
                anyhow::bail!("usage: ultty-session stop NAME");
            }
            let mut client = connect(session, &HashMap::new(), false, false)?;
            print_response(&client.request(Command::Terminate)?)
        }
        "snapshot" | "input" | "client" => {
            let options = flags(
                rest,
                &["--token-file", "--ssh", "--pane", "--text"],
                &["--controller"],
            )?;
            let controller = command == "input" || options.contains_key("--controller");
            let mut client = connect(session, &options, controller, command != "snapshot")?;
            match command {
                "snapshot" => print_response(&client.request(Command::Snapshot {
                    after_sequence: None,
                })?),
                "input" => print_response(
                    &client.request(Command::Input {
                        pane: options
                            .get("--pane")
                            .ok_or_else(|| anyhow::anyhow!("--pane is required"))?
                            .parse()?,
                        data: options
                            .get("--text")
                            .ok_or_else(|| anyhow::anyhow!("--text is required"))?
                            .as_bytes()
                            .to_vec(),
                    })?,
                ),
                _ => {
                    serde_json::to_writer(
                        io::stdout().lock(),
                        &serde_json::json!({"type":"connected",
                        "owner":client.owner(),"grant_id":client.grant_id(),"capabilities":client.capabilities(),
                        "controller":client.controller(),"last_mutation_id":client.last_mutation_id()}),
                    )?;
                    println!();
                    io::stdout().flush()?;
                    let stdin = io::stdin();
                    let mut input = stdin.lock();
                    let mut refused = false;
                    loop {
                        let mut line = Vec::new();
                        let count = input
                            .by_ref()
                            .take(wire::MAX_REQUEST_BYTES as u64 + 1)
                            .read_until(b'\n', &mut line)?;
                        if count == 0 {
                            break;
                        }
                        if count > wire::MAX_REQUEST_BYTES {
                            anyhow::bail!("client command exceeds byte limit");
                        }
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        let request: Command = serde_json::from_slice(&line)?;
                        if matches!(request, Command::Grant { .. }) {
                            anyhow::bail!("use grant --out FILE to publish credentials privately");
                        }
                        let response = client.request(request)?;
                        write_response(&response)?;
                        if matches!(response.reply, Reply::Error { .. }) {
                            refused = true;
                        }
                    }
                    if refused {
                        anyhow::bail!("one or more client commands were refused");
                    }
                    Ok(())
                }
            }
        }
        _ => anyhow::bail!("{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_grant_file_publication_is_private_atomic_and_no_clobber() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grant.token");
        write_token_atomic(&path, "1.first").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_token_file(&path).unwrap(), "1.first");
        assert!(write_token_atomic(&path, "2.replacement").is_err());
        assert_eq!(read_token_file(&path).unwrap(), "1.first");
    }

    #[test]
    fn test_operational_flags_reject_ambiguity_and_credentials_are_bounded() {
        let args = ["--caps", "read", "--caps", "input"].map(str::to_string);
        assert!(flags(&args, &["--caps"], &[]).is_err());
        assert!(flags(&["--unknown".into()], &[], &[]).is_err());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.token");
        fs::write(&path, "x".repeat(129)).unwrap();
        assert!(read_token_file(&path).is_err());
    }
}
