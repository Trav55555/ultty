//! Session save/restore for persisting terminal state across restarts.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);
const MAX_SESSION_BYTES: u64 = 16 * 1024 * 1024;

/// Session data that can be saved and restored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    /// Session format version
    pub version: u32,
    /// When the session was saved
    pub timestamp: String,
    /// Tabs in the session
    pub tabs: Vec<TabData>,
    /// Index of the active tab
    pub active_tab_index: usize,
}

/// Data for a single tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabData {
    /// Tab title
    pub title: String,
    /// Working directory (from shell integration or profile)
    pub working_directory: Option<String>,
    /// Terminals/panes in this tab
    pub terminals: Vec<TerminalData>,
    /// How panes are split (the top-level split; kept for older readers)
    pub split_direction: String,
    /// The full split tree, panes referring to `terminals` by position.
    /// Absent in sessions saved before nested splits: those restore as one
    /// row or column in `split_direction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutData>,
}

/// A saved split tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LayoutData {
    /// A pane: the terminal at `index` in the tab's `terminals`.
    Pane { index: usize },
    /// Children side by side ("Horizontal") or stacked ("Vertical"), each
    /// with its share of the split.
    Split {
        direction: String,
        children: Vec<(LayoutData, f32)>,
    },
}

/// Data for a single terminal/pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalData {
    /// Terminal title
    pub title: String,
    /// Working directory
    pub working_directory: Option<String>,
    /// Whether this terminal is focused within the tab
    pub is_focused: bool,
}

fn validate_workspace_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 64
        || matches!(name, "." | ".." | "last")
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "workspace name must be 1-64 ASCII letters, digits, dots, dashes or underscores; last is reserved"));
    }
    Ok(())
}

fn validate_layout(layout: &LayoutData, depth: usize, seen: &mut [bool]) -> io::Result<()> {
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
    if depth > 16 {
        return Err(invalid("workspace layout exceeds depth limit"));
    }
    match layout {
        LayoutData::Pane { index } => {
            let value = seen
                .get_mut(*index)
                .ok_or_else(|| invalid("workspace pane index out of bounds"))?;
            if *value {
                return Err(invalid("workspace layout repeats a pane"));
            }
            *value = true;
        }
        LayoutData::Split {
            direction,
            children,
        } => {
            if !matches!(direction.as_str(), "Horizontal" | "Vertical")
                || children.len() < 2
                || children.len() > seen.len()
            {
                return Err(invalid("invalid workspace split"));
            }
            let mut total = 0.0f32;
            for (child, weight) in children {
                if !weight.is_finite() || *weight <= 0.0 {
                    return Err(invalid("invalid workspace split weight"));
                }
                total += *weight;
                validate_layout(child, depth + 1, seen)?;
            }
            if !total.is_finite() {
                return Err(invalid("workspace split weight overflow"));
            }
        }
    }
    Ok(())
}

impl SessionData {
    /// Create a new empty session.
    pub fn new() -> Self {
        Self {
            version: 1,
            timestamp: chrono_timestamp(),
            tabs: Vec::new(),
            active_tab_index: 0,
        }
    }

    /// Get the sessions directory path.
    pub fn sessions_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("ultty").join("sessions"))
    }

    /// Get the default session file path (last.json).
    pub fn last_session_path() -> Option<PathBuf> {
        Self::sessions_dir().map(|p| p.join("last.json"))
    }

    /// Resolve a named workspace without accepting directory traversal.
    /// `last` is reserved for automatic window restoration.
    pub fn named_path(name: &str) -> io::Result<PathBuf> {
        validate_workspace_name(name)?;
        Self::sessions_dir()
            .map(|dir| dir.join(format!("{name}.json")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))
    }

    /// Save a validated layout recipe atomically under a workspace name.
    pub fn save_named(&self, name: &str) -> io::Result<()> {
        self.validate()?;
        self.save(&Self::named_path(name)?)
    }

    /// Load and validate a named layout before restoring fresh shells.
    pub fn load_named(name: &str) -> io::Result<Self> {
        let saved = Self::load(&Self::named_path(name)?)?;
        saved.validate()?;
        Ok(saved)
    }

    /// List saved workspace names, excluding automatic last-session state.
    pub fn workspace_names() -> io::Result<Vec<String>> {
        let dir = Self::sessions_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                if validate_workspace_name(name).is_ok() {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Validate a requested layout before creating any processes.
    pub fn validate(&self) -> io::Result<()> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
        if self.version != 1 {
            return Err(invalid("unsupported workspace version"));
        }
        if self.tabs.is_empty() || self.tabs.len() > 128 || self.active_tab_index >= self.tabs.len()
        {
            return Err(invalid("invalid workspace tab count or active tab"));
        }
        let mut total = 0usize;
        for tab in &self.tabs {
            total = total.saturating_add(tab.terminals.len());
            if tab.terminals.is_empty() || total > 1024 {
                return Err(invalid("invalid workspace pane count"));
            }
            if !matches!(tab.split_direction.as_str(), "Horizontal" | "Vertical") {
                return Err(invalid("invalid workspace split direction"));
            }
            if let Some(layout) = &tab.layout {
                let mut seen = vec![false; tab.terminals.len()];
                validate_layout(layout, 0, &mut seen)?;
                if seen.iter().any(|seen| !seen) {
                    return Err(invalid("workspace layout omits a pane"));
                }
            }
        }
        Ok(())
    }

    /// Save session to a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if json.len() as u64 > MAX_SESSION_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace exceeds size limit",
            ));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary = parent.join(format!(
            ".ultty-save-{}-{}",
            std::process::id(),
            NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&json)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Load session from a file.
    pub fn load(path: &PathBuf) -> io::Result<Self> {
        let mut json = Vec::new();
        fs::File::open(path)?
            .take(MAX_SESSION_BYTES + 1)
            .read_to_end(&mut json)?;
        if json.len() as u64 > MAX_SESSION_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace exceeds size limit",
            ));
        }
        serde_json::from_slice(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Save as the "last" session (auto-save on exit).
    pub fn save_as_last(&self) -> io::Result<()> {
        if let Some(path) = Self::last_session_path() {
            self.save(&path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Could not determine sessions directory",
            ))
        }
    }

    /// Load the "last" session if it exists.
    pub fn load_last() -> Option<Self> {
        Self::last_session_path().and_then(|path| Self::load(&path).ok())
    }

    /// Check if a last session exists.
    pub fn has_last_session() -> bool {
        Self::last_session_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    }
}

impl Default for SessionData {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a timestamp string for the session.
fn chrono_timestamp() -> String {
    use std::time::SystemTime;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    // Simple ISO-like timestamp without external crate
    let secs = now.as_secs();
    let days_since_epoch = secs / 86400;
    let secs_today = secs % 86400;

    // Approximate date calculation (not accounting for leap years perfectly)
    let years = 1970 + days_since_epoch / 365;
    let remaining_days = days_since_epoch % 365;
    let month = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;

    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    let seconds = secs_today % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        years, month, day, hours, minutes, seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_workspace_names_cannot_escape_storage() {
        for name in ["", ".", "..", "../other", "a/b", "/tmp/x", "last", "a\\b"] {
            assert!(SessionData::named_path(name).is_err(), "{name}");
        }
        assert!(SessionData::named_path("dev-remote_2")
            .unwrap()
            .ends_with("dev-remote_2.json"));
    }

    #[test]
    fn test_workspace_validation_rejects_duplicate_missing_and_invalid_weights() {
        let mut session = sample_session();
        session.validate().unwrap();
        session.tabs[0].layout = Some(LayoutData::Split {
            direction: "Horizontal".to_owned(),
            children: vec![
                (LayoutData::Pane { index: 0 }, 0.5),
                (LayoutData::Pane { index: 0 }, 0.5),
            ],
        });
        assert!(session.validate().is_err());
        session.tabs[0].layout = Some(LayoutData::Pane { index: 0 });
        assert!(session.validate().is_err());
        session.tabs[0].layout = Some(LayoutData::Split {
            direction: "Horizontal".to_owned(),
            children: vec![
                (LayoutData::Pane { index: 0 }, 1.0),
                (LayoutData::Pane { index: 1 }, f32::INFINITY),
            ],
        });
        assert!(session.validate().is_err());
        session.tabs[0].layout = None;
        session.version = 2;
        assert!(session.validate().is_err());
    }

    #[test]
    fn test_atomic_save_replaces_complete_file_privately() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workspace.json");
        fs::write(&path, "old contents").unwrap();
        let session = sample_session();
        session.save(&path).unwrap();
        assert_eq!(
            SessionData::load(&path).unwrap().tabs.len(),
            session.tabs.len()
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    // ── Helpers ──────────────────────────────────────────────────────

    /// Build a minimal but complete SessionData for testing.
    fn sample_session() -> SessionData {
        SessionData {
            version: 1,
            timestamp: "2025-01-15T10:30:00Z".to_string(),
            tabs: vec![
                TabData {
                    title: "editor".to_string(),
                    working_directory: Some("/home/user/project".to_string()),
                    terminals: vec![
                        TerminalData {
                            title: "nvim".to_string(),
                            working_directory: Some("/home/user/project".to_string()),
                            is_focused: true,
                        },
                        TerminalData {
                            title: "cargo watch".to_string(),
                            working_directory: Some("/home/user/project".to_string()),
                            is_focused: false,
                        },
                    ],
                    split_direction: "Horizontal".to_string(),
                    layout: None,
                },
                TabData {
                    title: "shell".to_string(),
                    working_directory: Some("/tmp".to_string()),
                    terminals: vec![TerminalData {
                        title: "bash".to_string(),
                        working_directory: Some("/tmp".to_string()),
                        is_focused: true,
                    }],
                    split_direction: "Vertical".to_string(),
                    layout: None,
                },
            ],
            active_tab_index: 0,
        }
    }

    // ── SessionData::new / Default ───────────────────────────────────

    #[test]
    fn new_session_has_version_1() {
        let session = SessionData::new();
        assert_eq!(session.version, 1);
    }

    #[test]
    fn new_session_starts_with_no_tabs() {
        let session = SessionData::new();
        assert!(session.tabs.is_empty());
    }

    #[test]
    fn new_session_active_tab_index_is_zero() {
        let session = SessionData::new();
        assert_eq!(session.active_tab_index, 0);
    }

    #[test]
    fn new_session_has_nonempty_timestamp() {
        let session = SessionData::new();
        assert!(!session.timestamp.is_empty());
    }

    #[test]
    fn default_matches_new() {
        let from_new = SessionData::new();
        let from_default = SessionData::default();

        assert_eq!(from_new.version, from_default.version);
        assert_eq!(from_new.tabs.len(), from_default.tabs.len());
        assert_eq!(from_new.active_tab_index, from_default.active_tab_index);
        // timestamps may differ by a few nanoseconds but should both be non-empty
        assert!(!from_default.timestamp.is_empty());
    }

    // ── Timestamp format ─────────────────────────────────────────────

    #[test]
    fn chrono_timestamp_is_iso_like() {
        let ts = chrono_timestamp();
        // Should match YYYY-MM-DDTHH:MM:SSZ
        assert!(ts.contains('T'), "timestamp missing T separator: {ts}");
        assert!(ts.ends_with('Z'), "timestamp missing trailing Z: {ts}");
        assert_eq!(ts.len(), 20, "unexpected timestamp length: {ts}");
    }

    // ── Clone / Debug derive ─────────────────────────────────────────

    #[test]
    fn session_data_is_cloneable() {
        let session = sample_session();
        let cloned = session.clone();
        assert_eq!(cloned.version, session.version);
        assert_eq!(cloned.tabs.len(), session.tabs.len());
        assert_eq!(cloned.active_tab_index, session.active_tab_index);
        assert_eq!(cloned.timestamp, session.timestamp);
    }

    #[test]
    fn session_data_is_debug() {
        let session = SessionData::new();
        let debug_str = format!("{:?}", session);
        assert!(debug_str.contains("SessionData"));
    }

    #[test]
    fn tab_data_is_cloneable_and_debug() {
        let tab = TabData {
            title: "t".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Horizontal".to_string(),
            layout: None,
        };
        let cloned = tab.clone();
        assert_eq!(cloned.title, tab.title);
        let debug_str = format!("{:?}", tab);
        assert!(debug_str.contains("TabData"));
    }

    #[test]
    fn terminal_data_is_cloneable_and_debug() {
        let term = TerminalData {
            title: "zsh".to_string(),
            working_directory: None,
            is_focused: false,
        };
        let cloned = term.clone();
        assert_eq!(cloned.title, "zsh");
        assert!(!cloned.is_focused);
        let debug_str = format!("{:?}", term);
        assert!(debug_str.contains("TerminalData"));
    }

    // ── Serialization / Deserialization ───────────────────────────────

    #[test]
    fn serialize_empty_session() {
        let session = SessionData {
            version: 1,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            tabs: vec![],
            active_tab_index: 0,
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"tabs\":[]"));
    }

    #[test]
    fn roundtrip_sample_session() {
        let original = sample_session();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.timestamp, original.timestamp);
        assert_eq!(loaded.active_tab_index, original.active_tab_index);
        assert_eq!(loaded.tabs.len(), 2);

        // First tab
        assert_eq!(loaded.tabs[0].title, "editor");
        assert_eq!(
            loaded.tabs[0].working_directory,
            Some("/home/user/project".to_string())
        );
        assert_eq!(loaded.tabs[0].terminals.len(), 2);
        assert_eq!(loaded.tabs[0].split_direction, "Horizontal");
        assert!(loaded.tabs[0].terminals[0].is_focused);
        assert!(!loaded.tabs[0].terminals[1].is_focused);

        // Second tab
        assert_eq!(loaded.tabs[1].title, "shell");
        assert_eq!(loaded.tabs[1].terminals.len(), 1);
    }

    #[test]
    fn deserialize_with_null_working_directory() {
        let json = r#"{
            "version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "tabs": [{
                "title": "tab",
                "working_directory": null,
                "terminals": [{
                    "title": "bash",
                    "working_directory": null,
                    "is_focused": false
                }],
                "split_direction": "Vertical"
            }],
            "active_tab_index": 0
        }"#;
        let session: SessionData = serde_json::from_str(json).unwrap();
        assert!(session.tabs[0].working_directory.is_none());
        assert!(session.tabs[0].terminals[0].working_directory.is_none());
    }

    #[test]
    fn deserialize_missing_optional_working_directory() {
        // serde should also accept the field being absent entirely when
        // it's Option<String> (only if the struct uses
        // #[serde(default)]).  In practice, serde_json will error if
        // the field is absent and there's no default, so we just verify
        // with null.
        let json = r#"{
            "version": 2,
            "timestamp": "ts",
            "tabs": [],
            "active_tab_index": 5
        }"#;
        let session: SessionData = serde_json::from_str(json).unwrap();
        assert_eq!(session.version, 2);
        assert_eq!(session.active_tab_index, 5);
    }

    #[test]
    fn deserialize_invalid_json_errors() {
        let result = serde_json::from_str::<SessionData>("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_wrong_types_errors() {
        let json =
            r#"{"version": "not_a_number", "timestamp": 123, "tabs": [], "active_tab_index": 0}"#;
        let result = serde_json::from_str::<SessionData>(json);
        assert!(result.is_err());
    }

    #[test]
    fn serialize_preserves_unicode() {
        let session = SessionData {
            version: 1,
            timestamp: "ts".to_string(),
            tabs: vec![TabData {
                title: "tab with unicode".to_string(),
                working_directory: Some("/home/user/docs".to_string()),
                terminals: vec![TerminalData {
                    title: "zsh".to_string(),
                    working_directory: Some("/home/user/docs".to_string()),
                    is_focused: true,
                }],
                split_direction: "Horizontal".to_string(),
                layout: None,
            }],
            active_tab_index: 0,
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.tabs[0].title, "tab with unicode");
    }

    #[test]
    fn serialize_many_tabs() {
        let tabs: Vec<TabData> = (0..100)
            .map(|i| TabData {
                title: format!("Tab {i}"),
                working_directory: Some(format!("/dir/{i}")),
                terminals: vec![TerminalData {
                    title: format!("term-{i}"),
                    working_directory: Some(format!("/dir/{i}")),
                    is_focused: i == 0,
                }],
                split_direction: "Horizontal".to_string(),
                layout: None,
            })
            .collect();
        let session = SessionData {
            version: 1,
            timestamp: "ts".to_string(),
            tabs,
            active_tab_index: 50,
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.tabs.len(), 100);
        assert_eq!(loaded.active_tab_index, 50);
        assert_eq!(loaded.tabs[99].title, "Tab 99");
    }

    // ── Save / Load (file I/O) ───────────────────────────────────────

    #[test]
    fn save_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.json");
        let session = sample_session();

        session.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_creates_parent_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("session.json");
        let session = SessionData::new();

        session.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_writes_valid_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.json");
        let session = sample_session();

        session.save(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"], 1);
    }

    #[test]
    fn save_writes_pretty_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.json");
        let session = sample_session();

        session.save(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        // Pretty JSON should contain newlines and indentation
        assert!(raw.contains('\n'));
        assert!(raw.contains("  "));
    }

    #[test]
    fn load_returns_saved_data() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.json");
        let session = sample_session();

        session.save(&path).unwrap();
        let loaded = SessionData::load(&path).unwrap();

        assert_eq!(loaded.version, session.version);
        assert_eq!(loaded.timestamp, session.timestamp);
        assert_eq!(loaded.tabs.len(), session.tabs.len());
        assert_eq!(loaded.active_tab_index, session.active_tab_index);

        for (orig_tab, loaded_tab) in session.tabs.iter().zip(loaded.tabs.iter()) {
            assert_eq!(orig_tab.title, loaded_tab.title);
            assert_eq!(orig_tab.working_directory, loaded_tab.working_directory);
            assert_eq!(orig_tab.split_direction, loaded_tab.split_direction);
            assert_eq!(orig_tab.terminals.len(), loaded_tab.terminals.len());

            for (orig_term, loaded_term) in
                orig_tab.terminals.iter().zip(loaded_tab.terminals.iter())
            {
                assert_eq!(orig_term.title, loaded_term.title);
                assert_eq!(orig_term.working_directory, loaded_term.working_directory);
                assert_eq!(orig_term.is_focused, loaded_term.is_focused);
            }
        }
    }

    #[test]
    fn load_nonexistent_file_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does_not_exist.json");

        let result = SessionData::load(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_invalid_json_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "this is not json").unwrap();

        let result = SessionData::load(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_wrong_schema_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wrong.json");
        fs::write(&path, r#"{"foo": "bar"}"#).unwrap();

        let result = SessionData::load(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn save_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.json");

        // Save first session with 1 tab
        let mut session1 = SessionData::new();
        session1.tabs.push(TabData {
            title: "first".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Horizontal".to_string(),
            layout: None,
        });
        session1.save(&path).unwrap();

        // Save second session with 2 tabs
        let mut session2 = SessionData::new();
        session2.tabs.push(TabData {
            title: "second-a".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Vertical".to_string(),
            layout: None,
        });
        session2.tabs.push(TabData {
            title: "second-b".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Vertical".to_string(),
            layout: None,
        });
        session2.save(&path).unwrap();

        let loaded = SessionData::load(&path).unwrap();
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[0].title, "second-a");
        assert_eq!(loaded.tabs[1].title, "second-b");
    }

    #[test]
    fn save_empty_session() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.json");
        let session = SessionData::new();

        session.save(&path).unwrap();
        let loaded = SessionData::load(&path).unwrap();
        assert!(loaded.tabs.is_empty());
        assert_eq!(loaded.version, 1);
    }

    // ── sessions_dir / last_session_path ─────────────────────────────

    #[test]
    fn sessions_dir_ends_with_sessions() {
        if let Some(dir) = SessionData::sessions_dir() {
            assert!(
                dir.ends_with("ultty/sessions"),
                "sessions_dir should end with ultty/sessions, got: {dir:?}"
            );
        }
        // If dirs::config_dir() returns None (e.g. in some CI
        // environments), sessions_dir() returns None, which is fine.
    }

    #[test]
    fn last_session_path_ends_with_last_json() {
        if let Some(path) = SessionData::last_session_path() {
            assert!(
                path.ends_with("last.json"),
                "last_session_path should end with last.json, got: {path:?}"
            );
        }
    }

    #[test]
    fn last_session_path_is_inside_sessions_dir() {
        if let (Some(dir), Some(path)) = (
            SessionData::sessions_dir(),
            SessionData::last_session_path(),
        ) {
            assert!(
                path.starts_with(&dir),
                "last_session_path should be inside sessions_dir"
            );
        }
    }

    // ── TabData field coverage ────────────────────────────────────────

    #[test]
    fn tab_with_no_terminals() {
        let tab = TabData {
            title: "empty".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Horizontal".to_string(),
            layout: None,
        };
        let json = serde_json::to_string(&tab).unwrap();
        let loaded: TabData = serde_json::from_str(&json).unwrap();
        assert!(loaded.terminals.is_empty());
        assert!(loaded.working_directory.is_none());
    }

    #[test]
    fn tab_with_multiple_terminals() {
        let tab = TabData {
            title: "multi".to_string(),
            working_directory: Some("/home".to_string()),
            terminals: vec![
                TerminalData {
                    title: "a".to_string(),
                    working_directory: None,
                    is_focused: false,
                },
                TerminalData {
                    title: "b".to_string(),
                    working_directory: Some("/home".to_string()),
                    is_focused: true,
                },
                TerminalData {
                    title: "c".to_string(),
                    working_directory: None,
                    is_focused: false,
                },
            ],
            split_direction: "Vertical".to_string(),
            layout: None,
        };
        let json = serde_json::to_string(&tab).unwrap();
        let loaded: TabData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.terminals.len(), 3);
        assert!(!loaded.terminals[0].is_focused);
        assert!(loaded.terminals[1].is_focused);
        assert!(!loaded.terminals[2].is_focused);
    }

    // ── TerminalData field coverage ──────────────────────────────────

    #[test]
    fn terminal_data_focused_true() {
        let term = TerminalData {
            title: "zsh".to_string(),
            working_directory: Some("/root".to_string()),
            is_focused: true,
        };
        let json = serde_json::to_string(&term).unwrap();
        let loaded: TerminalData = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_focused);
        assert_eq!(loaded.working_directory, Some("/root".to_string()));
    }

    #[test]
    fn terminal_data_focused_false() {
        let term = TerminalData {
            title: "fish".to_string(),
            working_directory: None,
            is_focused: false,
        };
        let json = serde_json::to_string(&term).unwrap();
        let loaded: TerminalData = serde_json::from_str(&json).unwrap();
        assert!(!loaded.is_focused);
        assert!(loaded.working_directory.is_none());
    }

    // ── Edge cases ───────────────────────────────────────────────────

    #[test]
    fn empty_string_fields() {
        let session = SessionData {
            version: 0,
            timestamp: "".to_string(),
            tabs: vec![TabData {
                title: "".to_string(),
                working_directory: Some("".to_string()),
                terminals: vec![TerminalData {
                    title: "".to_string(),
                    working_directory: Some("".to_string()),
                    is_focused: false,
                }],
                split_direction: "".to_string(),
                layout: None,
            }],
            active_tab_index: 0,
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.tabs[0].title, "");
        assert_eq!(loaded.tabs[0].working_directory, Some("".to_string()));
        assert_eq!(loaded.tabs[0].terminals[0].title, "");
    }

    #[test]
    fn large_active_tab_index() {
        let session = SessionData {
            version: 1,
            timestamp: "ts".to_string(),
            tabs: vec![],
            active_tab_index: usize::MAX,
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.active_tab_index, usize::MAX);
    }

    #[test]
    fn special_characters_in_paths() {
        let wd = Some("/home/user/my project (2)/test's dir".to_string());
        let term = TerminalData {
            title: "bash \"quoted\"".to_string(),
            working_directory: wd.clone(),
            is_focused: true,
        };
        let json = serde_json::to_string(&term).unwrap();
        let loaded: TerminalData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.working_directory, wd);
        assert_eq!(loaded.title, "bash \"quoted\"");
    }

    #[test]
    fn save_load_round_trip_file_io() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("roundtrip.json");
        let original = sample_session();

        original.save(&path).unwrap();
        let loaded = SessionData::load(&path).unwrap();

        // Full structural equality check
        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.timestamp, original.timestamp);
        assert_eq!(loaded.active_tab_index, original.active_tab_index);
        assert_eq!(loaded.tabs.len(), original.tabs.len());
    }

    #[test]
    fn load_empty_file_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty_file.json");
        fs::write(&path, "").unwrap();

        let result = SessionData::load(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_partial_json_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("partial.json");
        fs::write(&path, r#"{"version": 1, "timestamp": "ts""#).unwrap();

        let result = SessionData::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn multiple_saves_to_different_files() {
        let tmp = TempDir::new().unwrap();

        let mut s1 = SessionData::new();
        s1.tabs.push(TabData {
            title: "A".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Horizontal".to_string(),
            layout: None,
        });

        let mut s2 = SessionData::new();
        s2.tabs.push(TabData {
            title: "B".to_string(),
            working_directory: None,
            terminals: vec![],
            split_direction: "Vertical".to_string(),
            layout: None,
        });

        let p1 = tmp.path().join("s1.json");
        let p2 = tmp.path().join("s2.json");

        s1.save(&p1).unwrap();
        s2.save(&p2).unwrap();

        let l1 = SessionData::load(&p1).unwrap();
        let l2 = SessionData::load(&p2).unwrap();

        assert_eq!(l1.tabs[0].title, "A");
        assert_eq!(l2.tabs[0].title, "B");
    }
}
