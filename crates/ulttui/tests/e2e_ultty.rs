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

/// `tmux` on a server of this test's own: its own socket, so the user's
/// server is never touched, and no config, so the user's `~/.tmux.conf` and
/// its plugins never load.
fn tmux() -> Command {
    let socket = std::env::temp_dir().join(format!("ulttui-e2e-tmux-{}.sock", std::process::id()));
    let mut command = Command::new("tmux");
    command.arg("-f").arg("/dev/null").arg("-S").arg(&socket);
    command
}

fn cleanup() {
    let _ = tmux().arg("kill-server").output();
}

#[test]
#[ignore]
fn counter_app_button_click_via_ulttyctl() {
    cleanup();

    let status = tmux()
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
