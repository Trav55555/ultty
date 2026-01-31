//! Session save/restore for persisting terminal state across restarts.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

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
    /// How panes are split
    pub split_direction: String,
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

    /// Save session to a file.
    pub fn save(&self, path: &PathBuf) -> io::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load session from a file.
    pub fn load(path: &PathBuf) -> io::Result<Self> {
        let json = fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
        Self::last_session_path()
            .and_then(|path| Self::load(&path).ok())
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
    use std::env;

    #[test]
    fn new_session_has_version() {
        let session = SessionData::new();
        assert_eq!(session.version, 1);
        assert!(session.tabs.is_empty());
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut session = SessionData::new();
        session.tabs.push(TabData {
            title: "Test Tab".to_string(),
            working_directory: Some("/home/user".to_string()),
            terminals: vec![TerminalData {
                title: "bash".to_string(),
                working_directory: Some("/home/user".to_string()),
                is_focused: true,
            }],
            split_direction: "Horizontal".to_string(),
        });

        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].title, "Test Tab");
    }

    #[test]
    fn save_and_load() {
        let mut session = SessionData::new();
        session.tabs.push(TabData {
            title: "Tab 1".to_string(),
            working_directory: Some("/tmp".to_string()),
            terminals: vec![],
            split_direction: "Vertical".to_string(),
        });

        let temp_dir = env::temp_dir();
        let test_path = temp_dir.join("ultty_test_session.json");

        // Save
        session.save(&test_path).unwrap();
        assert!(test_path.exists());

        // Load
        let loaded = SessionData::load(&test_path).unwrap();
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].title, "Tab 1");

        // Cleanup
        fs::remove_file(&test_path).ok();
    }
}
