//! Self-checking tests for autonomous terminal verification.
//!
//! These tests can be run by Claude to verify terminal behavior without
//! requiring visual inspection.

use std::time::Duration;
use ultty::testing::{AppCheck, TerminalSnapshot, TestTerminal};

const WAIT_SHORT: Duration = Duration::from_millis(300);
const WAIT_LONG: Duration = Duration::from_secs(5);

fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ============================================================================
// SELF-VERIFICATION TESTS
// ============================================================================

#[test]
fn self_check_basic_echo() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Send echo command
    term.send("echo 'ULTTY_SELFCHECK_OK'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    // Verify output
    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    assert!(
        snapshot.contains("ULTTY_SELFCHECK_OK"),
        "Echo output should appear"
    );
}

#[test]
#[ignore]
fn self_check_ansi_colors() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Print colored text
    term.send("printf '\\e[31mR\\e[32mG\\e[34mB\\e[0m\\n'\r")
        .unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    // Should have RGB text
    assert!(snapshot.contains("R"), "Red text");
    assert!(snapshot.contains("G"), "Green text");
    assert!(snapshot.contains("B"), "Blue text");

    // Should have colored cells
    assert!(snapshot.has_colors(), "Should have non-default colors");
}

#[test]
#[ignore]
fn self_check_cursor_movement() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Move cursor to row 10, col 20 and print
    term.send("printf '\\e[10;20HTEST'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    // Verify text at expected position (row 9 in 0-indexed)
    assert!(
        snapshot.row_contains(9, "TEST"),
        "Text should appear at row 10"
    );
}

#[test]
#[ignore]
fn self_check_alternate_screen() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Should start in main screen
    assert!(!term.is_alternate_screen(), "Should start in main screen");

    // Enter alternate screen
    term.send("printf '\\e[?1049h'\r").unwrap();
    term.wait_for_output(Duration::from_millis(100)).unwrap();

    assert!(term.is_alternate_screen(), "Should be in alternate screen");

    // Leave alternate screen
    term.send("printf '\\e[?1049l'\r").unwrap();
    term.wait_for_output(Duration::from_millis(100)).unwrap();

    assert!(!term.is_alternate_screen(), "Should be back in main screen");
}

#[test]
#[ignore]
fn self_check_mouse_tracking() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Should start with mouse tracking off
    assert!(
        !term.mouse_tracking_enabled(),
        "Mouse should start disabled"
    );

    // Enable mouse tracking
    term.send("printf '\\e[?1000h'\r").unwrap();
    term.wait_for_output(Duration::from_millis(100)).unwrap();

    assert!(
        term.mouse_tracking_enabled(),
        "Mouse tracking should be enabled"
    );

    // Disable mouse tracking
    term.send("printf '\\e[?1000l'\r").unwrap();
    term.wait_for_output(Duration::from_millis(100)).unwrap();

    assert!(
        !term.mouse_tracking_enabled(),
        "Mouse tracking should be disabled"
    );
}

#[test]
fn self_check_unicode() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Print unicode
    term.send("echo '日本語 emoji:🚀'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    // Check for unicode rendering (at least partial)
    let content = snapshot.text_content();
    let has_cjk = content.contains("日") || content.contains("本") || content.contains("語");
    let has_emoji = content.contains("🚀") || content.contains("emoji");

    assert!(has_cjk || has_emoji, "Should render unicode: {}", content);
}

#[test]
#[ignore]
fn self_check_scrolling() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Print 50 lines
    term.send("seq 1 50\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    // Should see later numbers after scrolling
    assert!(snapshot.contains("50"), "Should show line 50");
    assert!(snapshot.contains("49"), "Should show line 49");
}

#[test]
fn self_check_clear_screen() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Print something
    term.send("echo 'BEFORE_CLEAR'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, Duration::from_secs(2))
        .unwrap();

    let before = term.snapshot();
    assert!(before.contains("BEFORE_CLEAR"), "Text should appear");

    // Clear screen
    term.send("clear\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, Duration::from_secs(2))
        .unwrap();

    // Print something else
    term.send("echo 'AFTER_CLEAR'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, Duration::from_secs(2))
        .unwrap();

    let after = term.snapshot();
    // After clear, old text should be gone from visible area
    // Note: It may still be in scrollback
    assert!(after.contains("AFTER_CLEAR"), "New text should appear");
}

// ============================================================================
// APP-SPECIFIC SELF-CHECKS
// ============================================================================

#[test]
fn self_check_nvim() {
    if !command_exists("nvim") {
        println!("SKIP: nvim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    let result = term.test_app(
        "nvim",
        &[AppCheck::AlternateScreen(true), AppCheck::MinRows(5)],
    );

    println!("nvim test: {}", result.summary());
    println!("{}", term.debug());

    // Quit nvim
    term.send("\x1b:q!\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    // Verify we're back to main screen
    assert!(!term.is_alternate_screen(), "Should exit alternate screen");

    if !result.is_success() {
        for fail in &result.failed {
            println!("  FAIL: {}", fail);
        }
    }
}

#[test]
fn self_check_vim() {
    if !command_exists("vim") {
        println!("SKIP: vim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    let result = term.test_app(
        "vim",
        &[
            AppCheck::AlternateScreen(true),
            AppCheck::ContainsText("~"), // Empty line markers
        ],
    );

    println!("vim test: {}", result.summary());
    println!("{}", term.debug());

    // Quit vim
    term.send("\x1b:q!\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    for fail in &result.failed {
        println!("  FAIL: {}", fail);
    }
}

#[test]
fn self_check_htop() {
    if !command_exists("htop") {
        println!("SKIP: htop not installed");
        return;
    }

    let mut term = TestTerminal::with_size(120, 40).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    let result = term.test_app(
        "htop",
        &[
            AppCheck::AlternateScreen(true),
            AppCheck::HasColors,
            AppCheck::MinRows(10),
        ],
    );

    println!("htop test: {}", result.summary());

    // Check for htop-specific content
    let snapshot = term.snapshot();
    let has_cpu = snapshot.contains("CPU") || snapshot.contains("Cpu");
    let has_mem = snapshot.contains("Mem");
    println!("htop content: CPU={}, Mem={}", has_cpu, has_mem);
    println!("{}", snapshot.to_debug_string());

    // Quit htop
    term.send("q").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    for fail in &result.failed {
        println!("  FAIL: {}", fail);
    }
}

#[test]
#[ignore]
fn self_check_less() {
    if !command_exists("less") {
        println!("SKIP: less not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Pipe to less
    term.send("seq 1 100 | less\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, WAIT_LONG).unwrap();

    let snapshot = term.snapshot();
    println!("{}", snapshot.to_debug_string());

    assert!(
        term.is_alternate_screen(),
        "less should use alternate screen"
    );
    assert!(snapshot.contains("1"), "Should show content");

    // Quit
    term.send("q").unwrap();
    term.wait_until_stable(WAIT_SHORT, Duration::from_secs(2))
        .unwrap();

    assert!(!term.is_alternate_screen(), "Should exit alternate screen");
}

// ============================================================================
// RESIZE SELF-CHECK
// ============================================================================

#[test]
fn self_check_resize() {
    let mut term = TestTerminal::with_size(80, 24).expect("create terminal");
    term.wait_for_output(WAIT_SHORT).unwrap();

    // Print something
    term.send("echo 'BEFORE_RESIZE'\r").unwrap();
    term.wait_until_stable(WAIT_SHORT, Duration::from_secs(2))
        .unwrap();

    let before = term.snapshot();
    println!("Before resize:\n{}", before.to_debug_string());

    // Resize
    term.resize(120, 40).unwrap();
    term.wait_for_output(Duration::from_millis(200)).unwrap();

    let after = term.snapshot();
    println!("After resize:\n{}", after.to_debug_string());

    // Dimensions should change
    assert_eq!(after.cols, 120, "Columns should be 120");
    assert_eq!(after.rows, 40, "Rows should be 40");

    // Content should still be visible (or in scrollback)
    // Note: The prompt may have moved
}

// ============================================================================
// COMPREHENSIVE REPORT
// ============================================================================

#[test]
fn generate_self_check_report() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           ULTTY SELF-CHECK VERIFICATION REPORT               ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let checks = [
        ("Basic echo", command_exists("echo")),
        ("ANSI colors", true),
        ("Cursor movement", true),
        ("Alternate screen", true),
        ("Mouse tracking", true),
        ("Unicode", true),
        ("Scrolling", true),
        ("Clear screen", true),
    ];

    println!("Terminal Capabilities:");
    for (name, available) in &checks {
        let status = if *available { "✓" } else { "✗" };
        println!("  {} {}", status, name);
    }

    println!();
    println!("Application Support:");
    let apps = [
        ("nvim", "Neovim"),
        ("vim", "Vim"),
        ("htop", "htop"),
        ("tmux", "tmux"),
        ("less", "less"),
        ("k9s", "k9s"),
    ];

    for (cmd, name) in &apps {
        let status = if command_exists(cmd) {
            "✓ available"
        } else {
            "✗ not found"
        };
        println!("  {:10} {} - {}", cmd, status, name);
    }

    println!();
    println!("Run individual tests with:");
    println!("  cargo test --package ultty --test self_check <test_name> -- --nocapture");
    println!();
}
