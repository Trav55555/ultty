//! Explicit local or SSH viewer connection selection.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::Options;
use crate::attach::remote::{connect_ssh, ssh_command};
use crate::attach::transport::{read_admin_token, AttachmentClient};
use crate::attach::wire::valid_session_name;

const HELP: &str = "Usage: ultty --attach SESSION [--observe] [--token-file PATH] [--ssh HOST]\n\nClose the window to detach; terminate the host explicitly with ultty-session.\nAlt+1..9 selects a tab. Ctrl+Shift+T claims a vacant controller lease.\nCtrl+Shift+R reconnects to the same owner and grant; queued input is discarded.\nUnconfirmed input is never replayed; some failures require reopening to resume control.\nCtrl+Shift+C/V copies/pastes through the viewer clipboard. Shift-drag selects text.\nSSH requires an explicit scoped token file and an installed remote ultty-session.\n";

/// Parse attachment options, authenticate once, then run the replica viewer.
pub fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let parsed = parse(args)?;
    let Some(parsed) = parsed else {
        print!("{HELP}");
        return Ok(());
    };
    let token = match parsed.token_file {
        Some(path) => {
            let mut token = String::new();
            std::fs::File::open(&path)
                .with_context(|| format!("reading {}", path.display()))?
                .take(129)
                .read_to_string(&mut token)?;
            if token.len() > 128 || token.trim().is_empty() {
                bail!("invalid attachment credential file");
            }
            token.trim().to_owned()
        }
        None if parsed.ssh.is_none() => read_admin_token(&parsed.name)?.expose_secret().to_owned(),
        None => bail!("--ssh requires --token-file with a scoped credential"),
    };
    let client = if let Some(host) = parsed.ssh {
        connect_ssh(&host, &parsed.name, &token, !parsed.observe)?
    } else {
        AttachmentClient::connect(&parsed.name, &token, !parsed.observe)?
    };
    super::run(
        Options {
            name: parsed.name,
            screenshot: parsed.screenshot,
            exit_after: parsed.exit_after,
        },
        client,
    )
}

struct Parsed {
    name: String,
    observe: bool,
    token_file: Option<PathBuf>,
    ssh: Option<String>,
    screenshot: Option<PathBuf>,
    exit_after: Option<Duration>,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Parsed>> {
    let mut args = args.into_iter();
    let Some(name) = args.next() else { bail!(HELP) };
    if name == "--help" || name == "-h" {
        return Ok(None);
    }
    if !valid_session_name(&name) {
        bail!("invalid attachment session name");
    }
    let mut parsed = Parsed {
        name,
        observe: false,
        token_file: None,
        ssh: None,
        screenshot: None,
        exit_after: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--observe" => parsed.observe = true,
            "--token-file" => {
                parsed.token_file = Some(args.next().context("--token-file needs a path")?.into())
            }
            "--ssh" => {
                let host = args.next().context("--ssh needs a host or SSH alias")?;
                ssh_command(&host)?;
                parsed.ssh = Some(host);
            }
            "--screenshot" => {
                parsed.screenshot = Some(args.next().context("--screenshot needs a path")?.into())
            }
            "--exit-after-ms" => {
                let ms = args
                    .next()
                    .context("--exit-after-ms needs milliseconds")?
                    .parse::<u64>()?;
                if !(1..=3_600_000).contains(&ms) {
                    bail!("exit duration must be 1..3600000 milliseconds");
                }
                parsed.exit_after = Some(Duration::from_millis(ms));
            }
            "--help" | "-h" => return Ok(None),
            _ => bail!("unknown attachment option {arg}"),
        }
    }
    if parsed.ssh.is_some() && parsed.token_file.is_none() {
        bail!("--ssh requires --token-file with a scoped credential");
    }
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_remote_viewer_cannot_default_to_local_admin_token() {
        assert!(parse(["work", "--ssh", "host"].map(str::to_owned)).is_err());
        let parsed = parse(
            [
                "work",
                "--observe",
                "--ssh",
                "host",
                "--token-file",
                "grant",
            ]
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert!(parsed.observe);
        assert_eq!(parsed.token_file, Some(PathBuf::from("grant")));
    }
}
