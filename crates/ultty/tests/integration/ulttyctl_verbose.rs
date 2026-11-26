//! `ulttyctl -v` explains itself on stderr without leaking the token or the
//! text it sends.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;

use ultty::control::paths::ControlEndpoint;
use ultty_protocol::control::{ControlData, ControlResponse};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn test_ulttyctl_verbose_narrates_selection_request_and_reply_but_not_secrets() {
    let runtime = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let endpoint = ControlEndpoint::for_pid(runtime.path(), pid);
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    std::fs::write(&endpoint.token, format!("{TOKEN}\n")).unwrap();

    let server = std::thread::spawn(move || {
        let reply = serde_json::to_string(&ControlResponse::Ok).unwrap();
        loop {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            if request.is_empty() {
                continue;
            }
            (&stream).write_all(reply.as_bytes()).unwrap();
            return request;
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_ulttyctl"))
        .args([
            "--pid",
            &pid.to_string(),
            "-v",
            "--pane",
            "1",
            "send-text",
            "secret-xyz",
        ])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env_remove("ULTTY_PID")
        .env_remove("ULTTY_PANE")
        .env_remove("ULTTY_AUTH_TOKEN")
        .output()
        .unwrap();
    let request = server.join().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(output.status.success(), "{stderr}");
    assert!(
        request.contains(TOKEN) && request.contains("secret-xyz"),
        "{request}"
    );
    for expected in [
        format!("ulttyctl: instance pid {pid}, named by --pid"),
        format!("ulttyctl: socket {}", endpoint.socket.display()),
        format!(
            "ulttyctl: auth token file {} (read)",
            endpoint.token.display()
        ),
        "ulttyctl: request kind=send_text pane=1 bytes=10 with_token=true".to_owned(),
        "ulttyctl: response ok in ".to_owned(),
    ] {
        assert!(
            stderr.contains(&expected),
            "missing {expected:?} in:\n{stderr}"
        );
    }
    assert!(
        !stderr.contains(TOKEN),
        "the token reached stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("secret-xyz"),
        "the text reached stderr:\n{stderr}"
    );
}

#[test]
fn test_ulttyctl_without_verbose_is_quiet_on_stderr() {
    let runtime = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let endpoint = ControlEndpoint::for_pid(runtime.path(), pid);
    let listener = UnixListener::bind(&endpoint.socket).unwrap();
    let server = std::thread::spawn(move || {
        let reply = serde_json::to_string(&ControlResponse::Data(ControlData::Pong)).unwrap();
        loop {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            if !request.is_empty() {
                (&stream).write_all(reply.as_bytes()).unwrap();
                return;
            }
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_ulttyctl"))
        .args(["--pid", &pid.to_string(), "ping"])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env_remove("ULTTY_PID")
        .env_remove("ULTTY_AUTH_TOKEN")
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}
