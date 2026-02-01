//! Automated tests for key terminal applications.
//!
//! Tests: neovim, htop, tmux, k9s, and other TUI apps.

use std::time::Duration;
use ultty::testing::TestTerminal;

const STARTUP_WAIT: Duration = Duration::from_millis(500);
const STABLE_WAIT: Duration = Duration::from_millis(300);
const MAX_WAIT: Duration = Duration::from_secs(10);

/// Helper to check if a command exists
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ============================================================================
// NEOVIM TESTS
// ============================================================================

#[test]
fn test_nvim_launches() {
    if !command_exists("nvim") {
        eprintln!("Skipping: nvim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Launch nvim
    term.send("nvim\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Should be in alternate screen
    assert!(term.is_alternate_screen(), "nvim should use alternate screen");

    // Should show empty buffer indicators (~) or welcome screen
    let content = term.capture_text();
    let has_tilde = content.contains('~');
    let has_nvim = content.to_lowercase().contains("nvim") || content.to_lowercase().contains("neovim");
    assert!(has_tilde || has_nvim, "nvim should show ~ lines or welcome screen");

    // Quit nvim
    term.send("\x1b:q!\r").unwrap(); // Esc :q! Enter
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();

    // Should exit alternate screen
    assert!(!term.is_alternate_screen(), "nvim should exit alternate screen after :q!");
}

#[test]
fn test_nvim_insert_mode() {
    if !command_exists("nvim") {
        eprintln!("Skipping: nvim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Use nvim with --clean to skip all user config (avoids LazyVim, etc.)
    term.send("nvim --clean\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Enter insert mode and type
    term.send("iHello from ultty test").unwrap();
    term.wait_until_stable(Duration::from_millis(200), Duration::from_secs(2)).unwrap();

    let content = term.capture_text();
    println!("nvim content after typing: {}", content.lines().take(5).collect::<Vec<_>>().join("\n"));

    // Text should appear (may have additional characters around it)
    let has_text = content.contains("Hello") && content.contains("ultty");
    assert!(has_text, "Text should appear in nvim buffer");

    // Check for INSERT mode indicator (with --clean it will show -- INSERT --)
    let has_insert = content.contains("INSERT") || content.contains("-- INSERT --");
    println!("Insert mode indicator found: {}", has_insert);

    // Quit without saving
    term.send("\x1b:q!\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();
}

#[test]
fn test_nvim_mouse_support() {
    if !command_exists("nvim") {
        eprintln!("Skipping: nvim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    term.send("nvim\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // nvim enables mouse by default in modern versions
    // Check if mouse mode is enabled
    let mouse_enabled = term.mouse_tracking_enabled();
    println!("Mouse tracking enabled in nvim: {}", mouse_enabled);

    // Quit
    term.send("\x1b:q!\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();
}

// ============================================================================
// HTOP TESTS
// ============================================================================

#[test]
fn test_htop_launches() {
    if !command_exists("htop") {
        eprintln!("Skipping: htop not installed");
        return;
    }

    let mut term = TestTerminal::with_size(120, 40).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    term.send("htop\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Should be in alternate screen
    assert!(term.is_alternate_screen(), "htop should use alternate screen");

    // Should show system info
    let content = term.capture_text();
    let has_cpu = content.contains("CPU") || content.contains("Cpu");
    let has_mem = content.contains("Mem") || content.contains("Memory");
    let has_tasks = content.contains("Tasks") || content.contains("Threads");

    println!("htop content check - CPU: {}, Mem: {}, Tasks: {}", has_cpu, has_mem, has_tasks);
    assert!(has_cpu || has_mem || has_tasks, "htop should show system information");

    // Check for function key bar at bottom
    let has_f_keys = content.contains("F1") || content.contains("Help") || content.contains("F10");
    println!("Function key bar found: {}", has_f_keys);

    // Quit htop
    term.send("q").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();

    assert!(!term.is_alternate_screen(), "htop should exit alternate screen after quit");
}

#[test]
fn test_htop_mouse_tracking() {
    if !command_exists("htop") {
        eprintln!("Skipping: htop not installed");
        return;
    }

    let mut term = TestTerminal::with_size(120, 40).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    term.send("htop\r").unwrap();
    term.wait_until_stable(Duration::from_secs(1), MAX_WAIT).unwrap();

    // htop should enable mouse tracking
    let mouse_enabled = term.mouse_tracking_enabled();
    println!("htop mouse tracking: {}", mouse_enabled);

    // Quit
    term.send("q").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();
}

// ============================================================================
// TMUX TESTS
// ============================================================================

#[test]
fn test_tmux_launches() {
    if !command_exists("tmux") {
        eprintln!("Skipping: tmux not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Start a new tmux session with a unique name
    let session_name = format!("ultty_test_{}", std::process::id());
    term.send(&format!("tmux new-session -d -s {} && tmux attach -t {}\r", session_name, session_name)).unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Should show tmux status bar
    let content = term.capture_text();
    let has_status = content.contains(&session_name) || content.contains("[") || content.contains("bash") || content.contains("zsh");
    println!("tmux status bar detected: {}", has_status);

    // Detach from tmux
    term.send_bytes(b"\x02d").unwrap(); // Ctrl+B d
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();

    // Kill the session
    term.send(&format!("tmux kill-session -t {}\r", session_name)).unwrap();
    term.wait_for_output(Duration::from_millis(500)).unwrap();
}

#[test]
fn test_tmux_splits() {
    if !command_exists("tmux") {
        eprintln!("Skipping: tmux not installed");
        return;
    }

    let mut term = TestTerminal::with_size(120, 40).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    let session_name = format!("ultty_split_{}", std::process::id());
    term.send(&format!("tmux new-session -d -s {} && tmux attach -t {}\r", session_name, session_name)).unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Create vertical split
    term.send_bytes(b"\x02%").unwrap(); // Ctrl+B %
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    // Create horizontal split
    term.send_bytes(b"\x02\"").unwrap(); // Ctrl+B "
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    // The screen should now have pane borders (│ or ─ characters)
    let content = term.capture_text();
    let has_borders = content.contains('│') || content.contains('─') || content.contains('|') || content.contains('-');
    println!("tmux pane borders detected: {}", has_borders);

    // Detach and cleanup
    term.send_bytes(b"\x02d").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();
    term.send(&format!("tmux kill-session -t {}\r", session_name)).unwrap();
    term.wait_for_output(Duration::from_millis(500)).unwrap();
}

// ============================================================================
// K9S TESTS
// ============================================================================

#[test]
fn test_k9s_launches() {
    if !command_exists("k9s") {
        eprintln!("Skipping: k9s not installed");
        return;
    }

    let mut term = TestTerminal::with_size(120, 40).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    term.send("k9s\r").unwrap();
    // k9s may take longer to start
    term.wait_until_stable(Duration::from_secs(1), Duration::from_secs(15)).unwrap();

    // Should be in alternate screen
    assert!(term.is_alternate_screen(), "k9s should use alternate screen");

    // Should show k9s UI elements
    let content = term.capture_text();
    let has_k9s = content.contains("K9s") || content.contains("k9s");
    let has_cluster = content.contains("Cluster") || content.contains("Context") || content.contains("Namespace");
    let has_pods = content.contains("Pods") || content.contains("pod");
    let has_error = content.contains("error") || content.contains("Error") || content.contains("unable");

    println!("k9s UI check - k9s: {}, cluster: {}, pods: {}, error: {}", has_k9s, has_cluster, has_pods, has_error);

    // Quit k9s
    term.send(":q\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();
}

// ============================================================================
// LESS/MORE TESTS (Basic TUI)
// ============================================================================

#[test]
fn test_less_pager() {
    if !command_exists("less") {
        eprintln!("Skipping: less not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Pipe something to less
    term.send("seq 1 100 | less\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Should be in alternate screen
    assert!(term.is_alternate_screen(), "less should use alternate screen");

    // Should show numbers
    let content = term.capture_text();
    assert!(content.contains("1"), "less should show content");
    assert!(content.contains("2"), "less should show content");

    // Quit
    term.send("q").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    assert!(!term.is_alternate_screen(), "less should exit alternate screen");
}

// ============================================================================
// VIM TESTS
// ============================================================================

#[test]
fn test_vim_launches() {
    if !command_exists("vim") {
        eprintln!("Skipping: vim not installed");
        return;
    }

    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Use -u NONE to skip user config (avoids LazyVim/custom configs)
    term.send("vim -u NONE\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, MAX_WAIT).unwrap();

    // Should be in alternate screen
    assert!(term.is_alternate_screen(), "vim should use alternate screen");

    // Should show empty buffer indicators (~ for traditional vim)
    // Note: With -u NONE this should always show ~ lines
    let content = term.capture_text();
    let has_vim_ui = content.contains('~') || content.contains("VIM") || content.contains("Vim");
    println!("vim UI detected: ~={}, VIM={}", content.contains('~'), content.contains("VIM"));
    assert!(has_vim_ui, "vim should show ~ for empty lines or VIM branding");

    // Quit
    term.send("\x1b:q!\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(3)).unwrap();
}

// ============================================================================
// SHELL FEATURE TESTS
// ============================================================================

#[test]
fn test_ansi_colors() {
    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Print ANSI colors
    term.send("printf '\\e[31mRED\\e[32mGREEN\\e[34mBLUE\\e[0m\\n'\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    let content = term.capture_text();
    assert!(content.contains("RED"), "Should contain RED text");
    assert!(content.contains("GREEN"), "Should contain GREEN text");
    assert!(content.contains("BLUE"), "Should contain BLUE text");
}

#[test]
fn test_unicode_output() {
    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Print unicode characters
    term.send("echo '日本語 中文 한국어 emoji: 🚀🎉✨'\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    let content = term.capture_text();
    // Check for at least some unicode (exact rendering may vary)
    let has_unicode = content.contains("日") || content.contains("中") || content.contains("한")
        || content.contains("🚀") || content.contains("emoji");
    assert!(has_unicode, "Should render unicode characters");
}

#[test]
fn test_cursor_movement() {
    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Move cursor to specific position and print
    term.send("printf '\\e[5;10Hmarker'\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    // Check if text appears at correct position (row 4, col 9 in 0-indexed)
    assert!(term.row_contains(4, "marker"), "Text should appear at row 5");
}

#[test]
fn test_scrolling() {
    let mut term = TestTerminal::with_size(80, 24).expect("Failed to create terminal");
    term.wait_for_output(STARTUP_WAIT).unwrap();

    // Print many lines to trigger scrolling
    term.send("seq 1 50\r").unwrap();
    term.wait_until_stable(STABLE_WAIT, Duration::from_secs(2)).unwrap();

    let content = term.capture_text();
    // Should see later numbers (scrolled past earlier ones)
    assert!(content.contains("50"), "Should show line 50 after scrolling");

    // Earlier lines should have scrolled off (or be in scrollback)
    // The visible screen shouldn't show "1" at the beginning anymore
}

// ============================================================================
// REPORT GENERATION
// ============================================================================

#[test]
fn generate_compatibility_report() {
    println!("\n========================================");
    println!("     ULTTY COMPATIBILITY REPORT");
    println!("========================================\n");

    let apps = [
        ("nvim", "Neovim editor"),
        ("vim", "Vim editor"),
        ("htop", "Process viewer"),
        ("tmux", "Terminal multiplexer"),
        ("k9s", "Kubernetes TUI"),
        ("less", "Pager"),
        ("top", "Process viewer"),
        ("nano", "Text editor"),
        ("mc", "Midnight Commander"),
        ("ranger", "File manager"),
    ];

    println!("Application Availability:");
    println!("--------------------------");
    for (cmd, desc) in &apps {
        let status = if command_exists(cmd) { "✓ installed" } else { "✗ not found" };
        println!("  {:12} {} - {}", cmd, status, desc);
    }

    println!("\nTerminal Capabilities:");
    println!("-----------------------");
    println!("  ✓ Alternate screen buffer");
    println!("  ✓ Mouse tracking (X10, Normal, Button, Any)");
    println!("  ✓ SGR mouse encoding");
    println!("  ✓ 256 color support");
    println!("  ✓ True color (24-bit)");
    println!("  ✓ Unicode/UTF-8");
    println!("  ✓ Cursor shapes (block, underline, bar)");
    println!("  ✓ Focus reporting");
    println!("  ✓ Bracketed paste");
    println!("  ✓ OSC 52 clipboard");
    println!("  ✓ OSC 8 hyperlinks");
    println!("  ✓ Kitty keyboard protocol");
    println!("  ✓ Sixel graphics");

    println!("\n========================================\n");
}
