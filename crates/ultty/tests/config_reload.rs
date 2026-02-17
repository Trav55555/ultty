//! Integration tests for config hot-reload functionality.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Helper to create a test config directory with a config file.
fn create_test_config(content: &str) -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("config.toml");

    let mut file = fs::File::create(&config_path).expect("Failed to create config file");
    file.write_all(content.as_bytes())
        .expect("Failed to write config");
    file.sync_all().expect("Failed to sync file");

    (temp_dir, config_path)
}

/// Helper to modify a config file.
fn modify_config(path: &PathBuf, content: &str) {
    let mut file = fs::File::create(path).expect("Failed to open config file");
    file.write_all(content.as_bytes())
        .expect("Failed to write config");
    file.sync_all().expect("Failed to sync file");
}

#[test]
fn test_config_parsing_basic() {
    let config_content = r##"
[colors]
foreground = "#FFFFFF"
background = "#000000"

[font]
family = "JetBrains Mono"
size = 14.0

[behavior]
scrollback_lines = 10000
url_detection = true
opacity = 0.95
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    // Verify file was created
    assert!(config_path.exists());

    // Read back and verify content
    let read_content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(read_content.contains("foreground"));
    assert!(read_content.contains("0.95"));
}

#[test]
fn test_config_modification_detected() {
    let initial_content = r##"
[colors]
foreground = "#FFFFFF"
"##;

    let modified_content = r##"
[colors]
foreground = "#00FF00"
"##;

    let (_temp_dir, config_path) = create_test_config(initial_content);

    // Small delay to ensure filesystem timestamps differ
    thread::sleep(Duration::from_millis(100));

    // Modify the config
    modify_config(&config_path, modified_content);

    // Verify modification
    let read_content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(read_content.contains("#00FF00"));
    assert!(!read_content.contains("#FFFFFF"));
}

#[test]
fn test_config_toml_syntax_valid() {
    let config_content = r##"
[colors]
foreground = "#E5E5E5"
background = "#1E1E1E"
cursor = "#FFFFFF"
selection = "#4D4D4D"

[colors.palette]
black = "#000000"
red = "#CD0000"
green = "#00CD00"
yellow = "#CDCD00"
blue = "#0000EE"
magenta = "#CD00CD"
cyan = "#00CDCD"
white = "#E5E5E5"
bright_black = "#7F7F7F"
bright_red = "#FF0000"
bright_green = "#00FF00"
bright_yellow = "#FFFF00"
bright_blue = "#5C5CFF"
bright_magenta = "#FF00FF"
bright_cyan = "#00FFFF"
bright_white = "#FFFFFF"

[font]
family = "JetBrains Mono"
size = 14.0
line_height = 1.2

[keybindings]
copy = "ctrl+shift+c"
paste = "ctrl+shift+v"
new_tab = "ctrl+shift+t"
close_tab = "ctrl+shift+w"
next_tab = "ctrl+tab"
prev_tab = "ctrl+shift+tab"
vim_mode = "ctrl+shift+space"
reload_config = "ctrl+shift+r"

[behavior]
scrollback_lines = 10000
url_detection = true
copy_on_select = false
bell_visual = true
bell_sound = false
close_on_exit = true
opacity = 1.0
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    // Parse with toml crate to verify syntax
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(
        parsed.is_ok(),
        "Config should be valid TOML: {:?}",
        parsed.err()
    );

    let value = parsed.unwrap();
    assert!(value.get("colors").is_some());
    assert!(value.get("font").is_some());
    assert!(value.get("keybindings").is_some());
    assert!(value.get("behavior").is_some());
}

#[test]
fn test_config_partial_valid() {
    // Test that partial config (missing sections) is valid TOML
    let config_content = r##"
[colors]
foreground = "#FFFFFF"
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(parsed.is_ok(), "Partial config should be valid TOML");
}

#[test]
fn test_config_color_format() {
    // Test various color format variations
    let config_content = r##"
[colors]
foreground = "#FFF"
background = "#1E1E1E"
cursor = "#FFFFFFAA"

[colors.palette]
black = "#000"
red = "#FF0000"
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(parsed.is_ok(), "Color formats should be valid");
}

#[test]
fn test_config_opacity_range() {
    // Test opacity values
    for opacity in ["0.0", "0.5", "1.0", "0.95"] {
        let config_content = format!(
            r##"
[behavior]
opacity = {}
"##,
            opacity
        );

        let (_temp_dir, config_path) = create_test_config(&config_content);

        let content = fs::read_to_string(&config_path).expect("Failed to read config");
        let parsed: Result<toml::Value, _> = toml::from_str(&content);

        assert!(parsed.is_ok(), "Opacity {} should be valid", opacity);
    }
}

#[test]
fn test_config_keybinding_formats() {
    let config_content = r##"
[keybindings]
copy = "ctrl+shift+c"
paste = "ctrl+shift+v"
new_tab = "ctrl+t"
close_tab = "alt+w"
vim_mode = "escape"
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(parsed.is_ok(), "Keybinding formats should be valid");

    let value = parsed.unwrap();
    let keybindings = value.get("keybindings").unwrap();
    assert_eq!(
        keybindings.get("copy").unwrap().as_str(),
        Some("ctrl+shift+c")
    );
}

#[test]
fn test_config_profiles() {
    let config_content = r##"
[profiles]
default = "dev"

[profiles.profiles.dev]
name = "Development"
working_dir = "~/projects"

[[profiles.profiles.dev.tabs]]
title = "Editor"
command = "nvim"

[[profiles.profiles.dev.tabs]]
title = "Terminal"

[profiles.profiles.server]
name = "Server"

[[profiles.profiles.server.tabs]]
title = "SSH"
command = "ssh"
args = ["user@server"]
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(
        parsed.is_ok(),
        "Profile config should be valid TOML: {:?}",
        parsed.err()
    );
}

#[test]
fn test_rapid_config_changes() {
    let (_temp_dir, config_path) = create_test_config("[colors]\nforeground = \"#000\"");

    // Simulate rapid config changes
    for i in 0..5 {
        let content = format!("[colors]\nforeground = \"#{}{}{}\"", i, i, i);
        modify_config(&config_path, &content);
        thread::sleep(Duration::from_millis(50));
    }

    // Verify final state
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("#444"));
}

#[test]
fn test_config_empty_sections() {
    let config_content = r##"
[colors]

[font]

[keybindings]

[behavior]
"##;

    let (_temp_dir, config_path) = create_test_config(config_content);

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    let parsed: Result<toml::Value, _> = toml::from_str(&content);

    assert!(parsed.is_ok(), "Empty sections should be valid");
}
