//! `ulttyctl` input commands name their pane (#241): without `--pane` or
//! `--focused` they fail before contacting Ultty, and `--focused` names the
//! pane `panes` marks focused instead of leaving the choice to the server.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

use serde_json::Value;
use ultty::control::paths::ControlEndpoint;
use ultty_protocol::control::{ControlData, ControlResponse, PaneInfo};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn ulttyctl(runtime: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ulttyctl"))
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("ULTTY_PANE", "9")
        .env_remove("ULTTY_PID")
        .env_remove("ULTTY_AUTH_TOKEN")
        .output()
        .unwrap()
}

fn pane(id: u64, focused: bool) -> PaneInfo {
    PaneInfo {
        id,
        tab_index: 0,
        window: 0,
        active: true,
        focused,
        title: String::new(),
        foreground: Some("tmux: client".to_string()),
        pid: None,
        cwd: None,
        cols: 80,
        rows: 24,
        status: None,
    }
}

/// The request inside a JSON envelope's `with_token` wrapper.
fn inner_request(line: &str) -> Value {
    let envelope: Value = serde_json::from_str(line).unwrap();
    let request = &envelope["request"];
    assert_eq!(request["type"], "with_token", "{line}");
    request["request"].clone()
}

#[test]
fn test_ulttyctl_input_without_a_pane_fails_before_connecting() {
    let runtime = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let endpoint = ControlEndpoint::for_pid(runtime.path(), pid);
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    std::fs::write(&endpoint.token, format!("{TOKEN}\n")).unwrap();
    let pid = pid.to_string();

    for command in [
        &["send-text", "echo hi"][..],
        &["send-key", "enter"],
        &["send-raw", "0a"],
        &["run", "ls"],
        &["run-wait", "ls"],
    ] {
        let mut args = vec!["--pid", pid.as_str()];
        args.extend_from_slice(command);
        let output = ulttyctl(runtime.path(), &args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(2), "{command:?}: {stderr}");
        assert!(
            stderr.contains(
                "name a pane with --pane N (ulttyctl panes lists them) or pass --focused"
            ),
            "{command:?}: {stderr}"
        );
    }
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().map(|_| ()).unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "an untargeted input command reached the socket"
    );
}

#[test]
fn test_ulttyctl_focused_names_the_focused_pane_explicitly() {
    let runtime = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let endpoint = ControlEndpoint::for_pid(runtime.path(), pid);
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    std::fs::write(&endpoint.token, format!("{TOKEN}\n")).unwrap();

    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        while requests.len() < 2 {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            if line.is_empty() {
                continue;
            }
            let request = inner_request(&line);
            let reply = if request["type"] == "list_panes" {
                ControlResponse::Data(ControlData::Panes {
                    panes: vec![pane(2, false), pane(5, true), pane(9, false)],
                })
            } else {
                ControlResponse::Ok
            };
            (&stream)
                .write_all(serde_json::to_string(&reply).unwrap().as_bytes())
                .unwrap();
            requests.push(request);
        }
        requests
    });

    let output = ulttyctl(
        runtime.path(),
        &[
            "--pid",
            &pid.to_string(),
            "--focused",
            "--through-multiplexer",
            "send-text",
            "echo hi",
        ],
    );
    let requests = server.join().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert_eq!(requests[0]["type"], "list_panes");
    assert_eq!(
        requests[1],
        serde_json::json!({
            "type": "send_text",
            "text": "echo hi",
            "target": 5,
            "through_multiplexer": true,
        }),
        "the focused pane, named, not ULTTY_PANE"
    );
}
