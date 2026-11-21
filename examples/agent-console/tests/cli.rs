//! Runs the `agent-console` binary on fixtures and on broken input.

use std::io::Write;
use std::process::{Command, Output, Stdio};

/// The binary under test. Some Cargo releases pass its path at compile time
/// and the pinned one only at run time, so look in both places.
fn binary() -> String {
    option_env!("CARGO_BIN_EXE_agent-console")
        .map(String::from)
        .or_else(|| std::env::var("CARGO_BIN_EXE_agent-console").ok())
        .expect("cargo sets CARGO_BIN_EXE_agent-console for integration tests")
}

fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn replay_fast_prints_one_line_per_event_and_a_total() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/short.jsonl");
    let out = run(&["replay", path, "--fast"], "");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 73 + 1);
    assert!(lines[0].contains("prompt"), "{}", lines[0]);
    assert_eq!(
        *lines.last().unwrap(),
        "73 events; 0 malformed lines skipped; 0 events of unknown type skipped"
    );
}

#[test]
fn replay_from_stdin_skips_bad_lines_and_fails_on_malformed_ones() {
    let input = concat!(
        "{\"t\":0,\"type\":\"prompt\",\"text\":\"hi\"}\n",
        "{\"t\":1,\"type\":\"future_thing\"}\n",
        "not json\n",
        "{\"t\":2,\"type\":\"turn_end\",\"turn\":1}\n",
    );
    let out = run(&["replay", "-", "--speed", "1000"], input);
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.ends_with("2 events; 1 malformed lines skipped; 1 events of unknown type skipped\n"),
        "{stdout}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("line 3"), "{stderr}");
    assert!(stderr.contains("future_thing"), "{stderr}");
}

#[test]
fn replay_rejects_a_bad_speed() {
    let out = run(&["replay", "-", "--speed", "0"], "");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("positive"), "{stderr}");
}
