//! Reference tests for the VT100/ANSI escape sequence parser.
//!
//! These tests feed raw escape sequences to the parser and verify
//! it produces the correct sequence of actions. Tests are data-driven
//! using JSON files in the `tests/ref/` directory.
//!
//! # Test Format
//!
//! Each test directory contains:
//! - `input.bin` - Raw bytes to feed to the parser
//! - `expected.json` - Expected actions as JSON array
//!
//! # Running Tests
//!
//! ```bash
//! cargo test -p ultty --test ref
//! ```

use serde::Deserialize;
use std::fs;
use std::path::Path;

// Import the actual parser types from the library
use ultty::protocol::ansi::{Action, AnsiParser, Attribute};

/// Expected test result.
#[derive(Debug, Deserialize)]
pub struct ExpectedResult {
    pub actions: Vec<Action>,
}

/// Macro to generate test functions for each reference test.
macro_rules! ref_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let test_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ref"));
                let test_path = test_dir.join(stringify!($name));
                ref_test(&test_path);
            }
        )*
    };
}

// Register all reference tests
ref_tests! {
    cursor_up,
    cursor_down,
    cursor_forward,
    cursor_back,
    cursor_position,
    erase_display,
    erase_line,
    sgr_bold,
    sgr_colors,
    sgr_reset,
    scroll_up,
    scroll_down,
    save_restore_cursor,
}

/// Run a reference test by loading input and expected output from files.
fn ref_test(dir: &Path) {
    // Load input bytes
    let input_path = dir.join("input.bin");
    let input = fs::read(&input_path)
        .unwrap_or_else(|e| panic!("Failed to read input file {}: {}", input_path.display(), e));

    // Load expected actions
    let expected_path = dir.join("expected.json");
    let expected_json = fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read expected file {}: {}",
            expected_path.display(),
            e
        )
    });
    let expected: ExpectedResult = serde_json::from_str(&expected_json).unwrap_or_else(|e| {
        panic!(
            "Failed to parse expected JSON {}: {}",
            expected_path.display(),
            e
        )
    });

    // Parse input using the actual parser
    let actions = parse_and_collect(&input);

    // Compare
    if actions != expected.actions {
        eprintln!("=== Input bytes ===");
        eprintln!("{:?}", input);
        eprintln!("\n=== Expected actions ===");
        for (i, action) in expected.actions.iter().enumerate() {
            eprintln!("[{}] {:?}", i, action);
        }
        eprintln!("\n=== Actual actions ===");
        for (i, action) in actions.iter().enumerate() {
            eprintln!("[{}] {:?}", i, action);
        }
        panic!(
            "Reference test failed: {} actions expected, {} actual",
            expected.actions.len(),
            actions.len()
        );
    }
}

/// Parse bytes using the AnsiParser and collect resulting actions.
fn parse_and_collect(input: &[u8]) -> Vec<Action> {
    let mut parser = AnsiParser::new();
    let mut actions = Vec::new();
    parser.feed(input, |action| actions.push(action));
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the parser handles basic cursor movement.
    #[test]
    fn inline_cursor_up() {
        let input = b"\x1b[5A";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::CursorUp(5)]);
    }

    /// Test that SGR bold is parsed correctly.
    #[test]
    fn inline_sgr_bold() {
        let input = b"\x1b[1m";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::SetAttribute(Attribute::Bold)]);
    }

    /// Test that SGR reset is parsed correctly.
    #[test]
    fn inline_sgr_reset() {
        let input = b"\x1b[0m";
        let actions = parse_and_collect(input);
        assert_eq!(actions, vec![Action::ResetAttributes]);
    }

    /// Test print characters.
    #[test]
    fn inline_print() {
        let input = b"Hello";
        let actions = parse_and_collect(input);
        assert_eq!(
            actions,
            vec![
                Action::Print('H'),
                Action::Print('e'),
                Action::Print('l'),
                Action::Print('l'),
                Action::Print('o'),
            ]
        );
    }
}
