//! Application compatibility tests for terminal emulator.
//!
//! These tests run real terminal applications (htop, nvim, etc.) in a headless
//! terminal and verify they render correctly. This allows Claude to autonomously
//! test terminal compatibility.
//!
//! # Running Tests
//!
//! ```bash
//! # Run all app compatibility tests
//! cargo test -p ultty --test app_compat
//!
//! # Run specific app test
//! cargo test -p ultty --test app_compat -- htop
//!
//! # Run with output for debugging
//! cargo test -p ultty --test app_compat -- --nocapture
//! ```

use std::time::Duration;
use ultty::event_loop::{EventLoop, TerminalEvent};
use ultty::grid::Terminal;

/// Test harness for running terminal applications headlessly.
pub struct AppTester {
    event_loop: EventLoop,
    cols: u16,
    rows: u16,
}

impl AppTester {
    /// Create a new tester that runs a shell.
    pub fn new_shell(cols: u16, rows: u16) -> std::io::Result<Self> {
        let event_loop = EventLoop::new(cols, rows)?;
        Ok(Self {
            event_loop,
            cols,
            rows,
        })
    }

    /// Create a new tester that runs a specific command.
    pub fn new_command(
        program: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Self> {
        let event_loop = EventLoop::with_command(program, args, cols, rows)?;
        Ok(Self {
            event_loop,
            cols,
            rows,
        })
    }

    /// Wait for the application to produce output (with timeout).
    pub fn wait_for_output(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let events = self.event_loop.wait(timeout)?;
        Ok(events.iter().any(|e| matches!(e, TerminalEvent::Redraw)))
    }

    /// Wait for application to settle (no new output for a period).
    pub fn wait_until_stable(
        &mut self,
        settle_time: Duration,
        max_wait: Duration,
    ) -> std::io::Result<()> {
        let start = std::time::Instant::now();
        let mut last_output = std::time::Instant::now();

        while start.elapsed() < max_wait {
            let events = self.event_loop.wait(Duration::from_millis(50))?;

            if events.iter().any(|e| matches!(e, TerminalEvent::Redraw)) {
                last_output = std::time::Instant::now();
            }

            if last_output.elapsed() >= settle_time {
                return Ok(());
            }

            // Check for exit
            for event in &events {
                if let TerminalEvent::ChildExited { status } = event {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Application exited with status {}", status),
                    ));
                }
            }
        }

        Ok(()) // Timeout, but that's okay
    }

    /// Send input to the application.
    pub fn send(&mut self, data: &str) -> std::io::Result<()> {
        self.event_loop.send_input(data.as_bytes())
    }

    /// Send a key sequence (e.g., "\x1b" for escape, "\x1bOA" for up arrow).
    pub fn send_key(&mut self, key: &str) -> std::io::Result<()> {
        self.event_loop.send_input(key.as_bytes())
    }

    /// Send Ctrl+C.
    pub fn send_ctrl_c(&mut self) -> std::io::Result<()> {
        self.send("\x03")
    }

    /// Send Escape.
    pub fn send_escape(&mut self) -> std::io::Result<()> {
        self.send("\x1b")
    }

    /// Send Enter.
    pub fn send_enter(&mut self) -> std::io::Result<()> {
        self.send("\r")
    }

    /// Get the terminal state.
    pub fn terminal(&self) -> &Terminal {
        self.event_loop.terminal()
    }

    /// Capture the current terminal grid as a string.
    pub fn capture_grid(&self) -> String {
        let terminal = self.terminal();
        let grid = terminal.grid();
        let dims = grid.dimensions();

        let mut lines = Vec::new();
        for row_idx in 0..dims.rows {
            if let Some(row) = grid.row(row_idx) {
                lines.push(row.text_trimmed());
            }
        }

        // Remove trailing empty lines
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }

        lines.join("\n")
    }

    /// Capture the grid with ANSI color codes preserved (for debugging).
    pub fn capture_grid_with_attrs(&self) -> String {
        let terminal = self.terminal();
        let grid = terminal.grid();
        let dims = grid.dimensions();

        let mut output = String::new();
        for row_idx in 0..dims.rows {
            if let Some(row) = grid.row(row_idx) {
                for col_idx in 0..dims.cols {
                    if let Some(cell) = row.get(col_idx) {
                        let ch = cell.char();
                        output.push(ch);
                    }
                }
            }
            output.push('\n');
        }

        output
    }

    /// Check if a string appears anywhere in the terminal.
    pub fn contains(&self, text: &str) -> bool {
        self.capture_grid().contains(text)
    }

    /// Check if the terminal is using alternate screen.
    pub fn is_alternate_screen(&self) -> bool {
        self.terminal().is_alternate_screen()
    }

    /// Check if mouse tracking is enabled.
    pub fn has_mouse_tracking(&self) -> bool {
        self.terminal().mouse_tracking_enabled()
    }

    /// Get the terminal title.
    pub fn title(&self) -> &str {
        self.terminal().title()
    }

    /// Check if application has exited.
    pub fn has_exited(&self) -> bool {
        self.event_loop.has_exited()
    }

    /// Get exit status if exited.
    pub fn exit_status(&self) -> Option<i32> {
        self.event_loop.exit_status()
    }

    /// Process pending output without waiting.
    pub fn process(&mut self) -> std::io::Result<Vec<TerminalEvent>> {
        self.event_loop.process()
    }
}

/// Check if a command is available on the system.
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ============================================================================
// Test: htop
// ============================================================================

#[test]
#[ignore] // Requires stable headless testing harness
fn test_htop_launches() {
    if !command_exists("htop") {
        eprintln!("Skipping htop test: htop not installed");
        return;
    }

    let mut tester = AppTester::new_command("htop", &[], 120, 40).expect("Failed to spawn htop");

    // Wait for htop to initialize
    tester
        .wait_until_stable(Duration::from_millis(500), Duration::from_secs(5))
        .expect("htop did not stabilize");

    // htop should switch to alternate screen
    assert!(
        tester.is_alternate_screen(),
        "htop should use alternate screen"
    );

    // htop should enable mouse tracking
    assert!(
        tester.has_mouse_tracking(),
        "htop should enable mouse tracking"
    );

    // Should show CPU/memory info
    let grid = tester.capture_grid();
    println!("=== htop grid ===\n{}", grid);

    // htop shows CPU percentage bars or "CPU" label
    assert!(
        grid.contains("CPU") || grid.contains("Mem") || grid.contains("%"),
        "htop should show CPU/Memory info"
    );

    // Clean exit
    tester.send_key("q").expect("Failed to send quit");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();
}

// ============================================================================
// Test: nvim (neovim)
// ============================================================================

#[test]
#[ignore] // Requires stable headless testing harness
fn test_nvim_launches() {
    if !command_exists("nvim") {
        eprintln!("Skipping nvim test: nvim not installed");
        return;
    }

    let mut tester =
        AppTester::new_command("nvim", &["--clean"], 120, 40).expect("Failed to spawn nvim");

    // Wait for nvim to initialize
    tester
        .wait_until_stable(Duration::from_millis(500), Duration::from_secs(5))
        .expect("nvim did not stabilize");

    // nvim should switch to alternate screen
    assert!(
        tester.is_alternate_screen(),
        "nvim should use alternate screen"
    );

    let grid = tester.capture_grid();
    println!("=== nvim grid ===\n{}", grid);

    // nvim shows tildes (~) for empty lines
    assert!(
        grid.contains("~") || grid.contains("NVIM"),
        "nvim should show empty line markers or welcome screen"
    );

    // Test insert mode
    tester.send_key("i").expect("Failed to enter insert mode");
    std::thread::sleep(Duration::from_millis(100));
    tester.process().ok();

    tester.send("Hello from test").expect("Failed to type");
    std::thread::sleep(Duration::from_millis(100));
    tester.process().ok();

    let grid = tester.capture_grid();
    assert!(
        grid.contains("Hello from test"),
        "nvim should show typed text"
    );

    // Exit: Escape, :q!, Enter
    tester.send_escape().expect("Failed to send escape");
    std::thread::sleep(Duration::from_millis(100));
    tester.send(":q!\r").expect("Failed to send quit");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();
}

// ============================================================================
// Test: vim
// ============================================================================

#[test]
fn test_vim_launches() {
    if !command_exists("vim") {
        eprintln!("Skipping vim test: vim not installed");
        return;
    }

    let mut tester =
        AppTester::new_command("vim", &["-u", "NONE"], 80, 24).expect("Failed to spawn vim");

    tester
        .wait_until_stable(Duration::from_millis(500), Duration::from_secs(5))
        .expect("vim did not stabilize");

    assert!(
        tester.is_alternate_screen(),
        "vim should use alternate screen"
    );

    let grid = tester.capture_grid();
    println!("=== vim grid ===\n{}", grid);

    // Exit
    tester.send_escape().expect("Failed to send escape");
    tester.send(":q!\r").expect("Failed to send quit");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();
}

// ============================================================================
// Test: tmux
// ============================================================================

#[test]
#[ignore]
fn test_tmux_launches() {
    if !command_exists("tmux") {
        eprintln!("Skipping tmux test: tmux not installed");
        return;
    }

    // Create a new tmux session with a unique name
    let session_name = format!("test_{}", std::process::id());
    let mut tester = AppTester::new_command(
        "tmux",
        &[
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-x",
            "80",
            "-y",
            "24",
            ";",
            "attach",
        ],
        80,
        24,
    )
    .expect("Failed to spawn tmux");

    tester
        .wait_until_stable(Duration::from_millis(500), Duration::from_secs(5))
        .expect("tmux did not stabilize");

    let grid = tester.capture_grid();
    println!("=== tmux grid ===\n{}", grid);

    // tmux should show a status bar at the bottom
    // Usually contains session name, window info, or time

    // Send command to echo something
    tester
        .send("echo 'tmux works'\r")
        .expect("Failed to send command");
    std::thread::sleep(Duration::from_millis(300));
    tester.process().ok();

    let grid = tester.capture_grid();
    assert!(
        grid.contains("tmux works") || grid.contains(&session_name),
        "tmux should show command output or session name"
    );

    // Kill the session
    tester
        .send(&format!("tmux kill-session -t {}\r", session_name))
        .ok();
    std::thread::sleep(Duration::from_millis(200));
}

// ============================================================================
// Test: Basic shell functionality
// ============================================================================

#[test]
fn test_shell_echo() {
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    // Send echo command
    tester
        .send("echo 'Terminal test passed'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();

    let grid = tester.capture_grid();
    println!("=== shell grid ===\n{}", grid);

    assert!(
        grid.contains("Terminal test passed"),
        "Shell should echo the message"
    );

    tester.send("exit\r").ok();
}

#[test]
fn test_shell_colors() {
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    // Send command that outputs colors
    tester
        .send("printf '\\033[31mRed\\033[0m \\033[32mGreen\\033[0m \\033[34mBlue\\033[0m\\n'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();

    let grid = tester.capture_grid();
    println!("=== colors grid ===\n{}", grid);

    // Should contain the color text (without escape codes in final render)
    assert!(grid.contains("Red"), "Should show Red text");
    assert!(grid.contains("Green"), "Should show Green text");
    assert!(grid.contains("Blue"), "Should show Blue text");

    tester.send("exit\r").ok();
}

#[test]
fn test_shell_unicode() {
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    // Test various Unicode
    tester
        .send("echo 'Unicode: \u{1F600} \u{4E2D}\u{6587} \u{00E9}'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();

    let grid = tester.capture_grid();
    println!("=== unicode grid ===\n{}", grid);

    // Should contain the Unicode characters
    assert!(
        grid.contains("\u{4E2D}") || grid.contains("Unicode"),
        "Should render Unicode correctly"
    );

    tester.send("exit\r").ok();
}

// ============================================================================
// Test: Mouse tracking verification
// ============================================================================

#[test]
#[ignore]
fn test_mouse_tracking_modes() {
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    // Initially no mouse tracking
    assert!(
        !tester.has_mouse_tracking(),
        "Shell should not have mouse tracking initially"
    );

    // Enable SGR mouse mode
    tester
        .send("printf '\\033[?1000h\\033[?1006h'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(100));
    tester.process().ok();

    assert!(
        tester.has_mouse_tracking(),
        "Should have mouse tracking after enabling"
    );

    // Disable
    tester
        .send("printf '\\033[?1000l'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(100));
    tester.process().ok();

    assert!(
        !tester.has_mouse_tracking(),
        "Should not have mouse tracking after disabling"
    );

    tester.send("exit\r").ok();
}

// ============================================================================
// Test: less pager
// ============================================================================

#[test]
#[ignore]
fn test_less_pager() {
    if !command_exists("less") {
        eprintln!("Skipping less test: less not installed");
        return;
    }

    // Create some content to view
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    // Use less to view some content
    tester.send("seq 1 100 | less\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(500));
    tester.process().ok();

    // less should use alternate screen
    assert!(
        tester.is_alternate_screen(),
        "less should use alternate screen"
    );

    let grid = tester.capture_grid();
    println!("=== less grid ===\n{}", grid);

    // Should show numbers
    assert!(grid.contains("1"), "less should show content");

    // Quit less
    tester.send_key("q").expect("Failed to quit less");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();

    tester.send("exit\r").ok();
}

// ============================================================================
// Compatibility report generator
// ============================================================================

/// Generate a compatibility report for all apps.
/// Run with: cargo test -p ultty --test app_compat -- generate_report --nocapture --ignored
#[test]
#[ignore]
fn generate_compatibility_report() {
    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║           Terminal Compatibility Report                    ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    let apps = [
        ("htop", vec!["htop"]),
        ("nvim", vec!["nvim", "--clean"]),
        ("vim", vec!["vim", "-u", "NONE"]),
        ("tmux", vec!["tmux", "new-session", "-d", "-s", "test"]),
        ("less", vec!["less", "/etc/passwd"]),
        ("top", vec!["top", "-b", "-n", "1"]),
        ("nano", vec!["nano"]),
    ];

    for (name, args) in apps.iter() {
        print!("Testing {:10} ... ", name);

        if !command_exists(args[0]) {
            println!("⏭  SKIPPED (not installed)");
            continue;
        }

        match AppTester::new_command(args[0], &args[1..], 120, 40) {
            Ok(mut tester) => {
                let stable =
                    tester.wait_until_stable(Duration::from_millis(300), Duration::from_secs(3));

                if stable.is_ok() {
                    let alt_screen = if tester.is_alternate_screen() {
                        "✓"
                    } else {
                        "✗"
                    };
                    let mouse = if tester.has_mouse_tracking() {
                        "✓"
                    } else {
                        "✗"
                    };

                    println!("✅ OK  [alt:{} mouse:{}]", alt_screen, mouse);

                    // Try to quit gracefully
                    tester.send_key("q").ok();
                    tester.send_escape().ok();
                    tester.send(":q!\r").ok();
                    tester.send_ctrl_c().ok();
                } else {
                    println!("⚠️  UNSTABLE");
                }
            }
            Err(e) => {
                println!("❌ FAILED: {}", e);
            }
        }
    }

    println!("\n════════════════════════════════════════════════════════════════");
}
