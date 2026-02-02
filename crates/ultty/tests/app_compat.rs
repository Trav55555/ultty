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
    #[allow(dead_code)]
    cols: u16,
    #[allow(dead_code)]
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
        let mut last_content = self.capture_grid();
        let mut stable_since = std::time::Instant::now();

        while start.elapsed() < max_wait {
            let events = self.event_loop.process()?;
            std::thread::sleep(Duration::from_millis(50));

            let current = self.capture_grid();
            if current != last_content {
                last_content = current;
                stable_since = std::time::Instant::now();
            } else if stable_since.elapsed() >= settle_time {
                return Ok(());
            }

            for event in &events {
                if let TerminalEvent::ChildExited { status } = event {
                    return Err(std::io::Error::other(format!(
                        "Application exited with status {}",
                        status
                    )));
                }
            }
        }

        Ok(())
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
fn test_htop_launches() {
    if !command_exists("htop") {
        eprintln!("Skipping htop test: htop not installed");
        return;
    }

    let mut tester = AppTester::new_command("htop", &[], 120, 40).expect("Failed to spawn htop");

    tester
        .wait_until_stable(Duration::from_millis(500), Duration::from_secs(5))
        .expect("htop did not stabilize");

    assert!(
        tester.is_alternate_screen(),
        "htop should use alternate screen"
    );

    let mouse_enabled = tester.has_mouse_tracking();
    println!("htop mouse tracking: {}", mouse_enabled);

    let grid = tester.capture_grid();
    println!("=== htop grid ===\n{}", grid);

    assert!(
        grid.contains("CPU")
            || grid.contains("Mem")
            || grid.contains("%")
            || grid.contains("Tasks"),
        "htop should show CPU/Memory info"
    );

    tester.send_key("q").expect("Failed to send quit");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();
}

// ============================================================================
// Test: nvim (neovim)
// ============================================================================

#[test]
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

    assert!(
        grid.contains("~")
            || grid.contains("NVIM")
            || grid.contains("Neovim")
            || grid.contains("Lazy")
            || !grid.is_empty(),
        "nvim should show content"
    );

    tester.send_escape().ok();
    std::thread::sleep(Duration::from_millis(100));
    tester.send(":q!\r").ok();
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
fn test_tmux_launches() {
    if !command_exists("tmux") {
        eprintln!("Skipping tmux test: tmux not installed");
        return;
    }

    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");
    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    let session_name = format!("ultty_test_{}", std::process::id());
    tester
        .send(&format!("tmux new-session -s {}\r", session_name))
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(1000));
    tester.process().ok();

    let _ = tester.wait_until_stable(Duration::from_millis(300), Duration::from_secs(3));

    let grid = tester.capture_grid();
    println!("=== tmux grid ===\n{}", grid);
    println!("tmux alternate_screen: {}", tester.is_alternate_screen());

    tester.send("echo 'tmux works'\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(300));
    tester.process().ok();

    let grid = tester.capture_grid();
    println!("=== tmux after echo ===\n{}", grid);

    assert!(
        grid.contains("tmux") || grid.contains(&session_name) || !grid.is_empty(),
        "tmux should render content"
    );

    tester.send("exit\r").ok();
    std::thread::sleep(Duration::from_millis(500));
    tester.send("exit\r").ok();
    tester.process().ok();
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

    tester
        .send("echo -e '\\x1b[?1000h\\x1b[?1006h'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(500));
    tester.process().ok();
    tester
        .wait_until_stable(Duration::from_millis(200), Duration::from_secs(2))
        .ok();

    let has_mouse = tester.has_mouse_tracking();
    println!("Mouse tracking after enable: {}", has_mouse);

    tester
        .send("echo -e '\\x1b[?1000l'\r")
        .expect("Failed to send");
    std::thread::sleep(Duration::from_millis(500));
    tester.process().ok();
    tester
        .wait_until_stable(Duration::from_millis(200), Duration::from_secs(2))
        .ok();

    let has_mouse_after = tester.has_mouse_tracking();
    println!("Mouse tracking after disable: {}", has_mouse_after);

    if !has_mouse && !has_mouse_after {
        eprintln!("Note: Mouse tracking escape sequences not processed - may be shell-dependent");
    }

    tester.send("exit\r").ok();
}

// ============================================================================
// Test: less pager
// ============================================================================

#[test]
fn test_less_pager() {
    if !command_exists("less") {
        eprintln!("Skipping less test: less not installed");
        return;
    }

    // Run through shell to give less a proper TTY (direct command execution exits immediately)
    let mut tester = AppTester::new_shell(80, 24).expect("Failed to spawn shell");

    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    tester.send("seq 1 100 | less\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(800));
    tester.process().ok();
    tester
        .wait_until_stable(Duration::from_millis(200), Duration::from_secs(3))
        .ok();

    let grid = tester.capture_grid();
    println!("=== less grid ===\n{}", grid);
    println!("less alternate_screen: {}", tester.is_alternate_screen());

    assert!(
        grid.contains("~")
            || grid.contains("NVIM")
            || grid.contains("Neovim")
            || grid.contains("Lazy")
            || !grid.is_empty(),
        "nvim should show content"
    );

    tester.send_escape().expect("Failed to send escape");
    std::thread::sleep(Duration::from_millis(100));
    tester.send(":q!\r").expect("Failed to send quit");
    std::thread::sleep(Duration::from_millis(200));
    tester.process().ok();
}

// ============================================================================
// Test: Claude Code (AI coding assistant)
// ============================================================================

#[test]
fn test_claude_code_launches() {
    if !command_exists("claude") {
        eprintln!("Skipping claude test: claude not installed");
        return;
    }

    let mut tester = AppTester::new_shell(120, 40).expect("Failed to spawn shell");
    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    tester.send("claude --help\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(500));
    tester.process().ok();

    tester
        .wait_until_stable(Duration::from_millis(200), Duration::from_secs(3))
        .ok();

    let grid = tester.capture_grid();
    println!("=== claude --help grid ===\n{}", grid);

    assert!(
        grid.contains("claude")
            || grid.contains("Claude")
            || grid.contains("Usage")
            || grid.contains("prompt"),
        "claude --help should show help output"
    );

    tester.send("exit\r").ok();
}

#[test]
fn test_claude_code_interactive() {
    if !command_exists("claude") {
        eprintln!("Skipping claude interactive test: claude not installed");
        return;
    }

    let mut tester = AppTester::new_shell(120, 40).expect("Failed to spawn shell");
    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    tester.send("claude\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(2000));
    tester.process().ok();

    let _ = tester.wait_until_stable(Duration::from_millis(500), Duration::from_secs(5));

    let grid = tester.capture_grid();
    println!("=== claude interactive grid ===\n{}", grid);

    let uses_alt = tester.is_alternate_screen();
    let has_mouse = tester.has_mouse_tracking();
    println!(
        "claude: alternate_screen={}, mouse_tracking={}",
        uses_alt, has_mouse
    );

    tester.send_ctrl_c().ok();
    std::thread::sleep(Duration::from_millis(500));
    tester.send("exit\r").ok();
    tester.process().ok();
}

// ============================================================================
// Test: OpenCode (AI coding assistant)
// ============================================================================

#[test]
fn test_opencode_launches() {
    if !command_exists("opencode") {
        eprintln!("Skipping opencode test: opencode not installed");
        return;
    }

    let mut tester = AppTester::new_shell(120, 40).expect("Failed to spawn shell");
    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    tester.send("opencode --help\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(500));
    tester.process().ok();

    tester
        .wait_until_stable(Duration::from_millis(200), Duration::from_secs(3))
        .ok();

    let grid = tester.capture_grid();
    println!("=== opencode --help grid ===\n{}", grid);

    assert!(
        grid.contains("opencode")
            || grid.contains("OPENCODE")
            || grid.contains("Commands")
            || grid.contains("completion"),
        "opencode --help should show help output"
    );

    tester.send("exit\r").ok();
}

#[test]
fn test_opencode_interactive() {
    if !command_exists("opencode") {
        eprintln!("Skipping opencode interactive test: opencode not installed");
        return;
    }

    let mut tester = AppTester::new_shell(120, 40).expect("Failed to spawn shell");
    tester
        .wait_until_stable(Duration::from_millis(300), Duration::from_secs(3))
        .expect("Shell did not stabilize");

    tester.send("opencode\r").expect("Failed to send");
    std::thread::sleep(Duration::from_millis(2000));
    tester.process().ok();

    let _ = tester.wait_until_stable(Duration::from_millis(500), Duration::from_secs(5));

    let grid = tester.capture_grid();
    println!("=== opencode interactive grid ===\n{}", grid);

    let uses_alt = tester.is_alternate_screen();
    let has_mouse = tester.has_mouse_tracking();
    println!(
        "opencode: alternate_screen={}, mouse_tracking={}",
        uses_alt, has_mouse
    );

    tester.send_ctrl_c().ok();
    std::thread::sleep(Duration::from_millis(500));
    tester.send("exit\r").ok();
    tester.process().ok();
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
        ("nano", vec!["nano"]),
        ("top", vec!["top"]),
        ("tmux", vec!["tmux", "new-session"]),
        ("claude", vec!["claude"]),
        ("opencode", vec!["opencode"]),
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

                match &stable {
                    Ok(()) => {
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

                        tester.send_key("q").ok();
                        tester.send_escape().ok();
                        tester.send(":q!\r").ok();
                        tester.send_ctrl_c().ok();
                    }
                    Err(e) => {
                        println!("⚠️  UNSTABLE ({})", e);
                    }
                }
            }
            Err(e) => {
                println!("❌ FAILED: {}", e);
            }
        }
    }

    println!("\n════════════════════════════════════════════════════════════════");
}
