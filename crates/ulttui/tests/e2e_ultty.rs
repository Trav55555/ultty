use std::process::Command;
use std::thread;
use std::time::Duration;

const ULTTY_BIN: &str = "target/release/ultty";
const ULTTYCTL_BIN: &str = "target/release/ulttyctl";
const TMUX_SESSION: &str = "ulttui-e2e";

fn ulttyctl(args: &[&str]) -> Option<String> {
    let output = Command::new(ULTTYCTL_BIN).args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

fn cleanup() {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", TMUX_SESSION])
        .output();
}

#[test]
#[ignore]
fn counter_app_button_click_via_ulttyctl() {
    cleanup();

    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", TMUX_SESSION, ULTTY_BIN])
        .status()
        .expect("tmux start");
    assert!(status.success(), "Failed to start tmux session");

    thread::sleep(Duration::from_secs(2));

    let pong = ulttyctl(&["ping"]);
    if pong.as_deref().map(|s| s.trim()) != Some("PONG") {
        cleanup();
        panic!("ulttyctl ping failed: {pong:?}");
    }

    ulttyctl(&["send-text", "cargo run -p counter-app"]);
    ulttyctl(&["send-key", "enter"]);
    thread::sleep(Duration::from_secs(5));

    ulttyctl(&["send-key", "tab"]);
    thread::sleep(Duration::from_millis(200));
    ulttyctl(&["send-key", "enter"]);
    thread::sleep(Duration::from_millis(500));

    let text = ulttyctl(&["text"]).unwrap_or_default();

    cleanup();

    assert!(
        text.contains("Count: -1"),
        "Expected 'Count: -1' in output, got:\n{text}"
    );
}
