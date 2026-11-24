//! Session save/restore for persisting terminal state across restarts.

use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);
const MAX_SESSION_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_WINDOWS: usize = 64;
const MAX_SESSION_TABS: usize = 128;
const MAX_SESSION_PANES: usize = 1024;
const MIN_WINDOW_LOGICAL_WIDTH: f64 = 200.0;
const MIN_WINDOW_LOGICAL_HEIGHT: f64 = 100.0;
const MAX_WINDOW_LOGICAL_EXTENT: f64 = 32_768.0;

/// Current session file format version (ADR 0027 Slice 4).
pub const SESSION_VERSION: u32 = 2;

/// Saved inner window size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    /// Inner width in logical pixels.
    pub width: f64,
    /// Inner height in logical pixels.
    pub height: f64,
}

/// Saved state of a single window in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowData {
    /// Tabs belonging to this window, in tab-bar order.
    pub tabs: Vec<TabData>,
    /// Index of the active tab within `tabs`.
    pub active_tab_index: usize,
    /// Saved inner window size in logical pixels, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<WindowSize>,
}

/// Session data that can be saved and restored.
///
/// The writer always emits v2 (`windows: [{ tabs, active_tab_index, size }]`).
/// Legacy v1 files (`tabs`, `active_tab_index`) load transparently as a single
/// window with `size: None`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionData {
    /// Session format version (`2`).
    pub version: u32,
    /// When the session was saved.
    pub timestamp: String,
    /// Windows in the session.
    pub windows: Vec<WindowData>,
}

#[derive(Deserialize)]
struct RawSessionData {
    version: u32,
    timestamp: String,
    #[serde(default)]
    windows: Option<Vec<WindowData>>,
    #[serde(default)]
    tabs: Option<Vec<TabData>>,
    #[serde(default)]
    active_tab_index: Option<usize>,
}

impl<'de> Deserialize<'de> for SessionData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawSessionData::deserialize(deserializer)?;
        match raw.version {
            1 => {
                if raw.windows.is_some() {
                    return Err(serde::de::Error::custom(
                        "v1 session must not contain a windows field",
                    ));
                }
                let tabs = raw
                    .tabs
                    .ok_or_else(|| serde::de::Error::missing_field("tabs"))?;
                let active_tab_index = raw.active_tab_index.unwrap_or(0);
                let windows = if tabs.is_empty() && active_tab_index == 0 {
                    Vec::new()
                } else {
                    vec![WindowData {
                        tabs,
                        active_tab_index,
                        size: None,
                    }]
                };
                Ok(Self {
                    version: SESSION_VERSION,
                    timestamp: raw.timestamp,
                    windows,
                })
            }
            2 => {
                if raw.tabs.is_some() {
                    return Err(serde::de::Error::custom(
                        "v2 session must not contain top-level tabs",
                    ));
                }
                let windows = raw
                    .windows
                    .ok_or_else(|| serde::de::Error::missing_field("windows"))?;
                Ok(Self {
                    version: SESSION_VERSION,
                    timestamp: raw.timestamp,
                    windows,
                })
            }
            other => {
                if raw.windows.is_none() && raw.tabs.is_none() {
                    return Err(serde::de::Error::missing_field("windows"));
                }
                let windows = raw.windows.unwrap_or_else(|| {
                    raw.tabs
                        .map(|tabs| {
                            vec![WindowData {
                                tabs,
                                active_tab_index: raw.active_tab_index.unwrap_or(0),
                                size: None,
                            }]
                        })
                        .unwrap_or_default()
                });
                Ok(Self {
                    version: other,
                    timestamp: raw.timestamp,
                    windows,
                })
            }
        }
    }
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
    /// Create a new empty v2 session.
    pub fn new() -> Self {
        Self {
            version: SESSION_VERSION,
            timestamp: current_timestamp(),
            windows: Vec::new(),
        }
    }

    /// Total number of tabs across all windows in the session.
    pub fn total_tab_count(&self) -> usize {
        self.windows.iter().map(|window| window.tabs.len()).sum()
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
        if self.version != SESSION_VERSION {
            return Err(invalid("unsupported workspace version"));
        }
        if self.windows.is_empty() || self.windows.len() > MAX_SESSION_WINDOWS {
            return Err(invalid("invalid workspace window count"));
        }
        let mut total_tabs = 0usize;
        let mut total_panes = 0usize;
        for window in &self.windows {
            if let Some(size) = window.size {
                if !size.width.is_finite()
                    || !size.height.is_finite()
                    || size.width < MIN_WINDOW_LOGICAL_WIDTH
                    || size.height < MIN_WINDOW_LOGICAL_HEIGHT
                    || size.width > MAX_WINDOW_LOGICAL_EXTENT
                    || size.height > MAX_WINDOW_LOGICAL_EXTENT
                {
                    return Err(invalid("invalid workspace window size"));
                }
            }
            total_tabs = total_tabs.saturating_add(window.tabs.len());
            if window.tabs.is_empty()
                || total_tabs > MAX_SESSION_TABS
                || window.active_tab_index >= window.tabs.len()
            {
                return Err(invalid("invalid workspace tab count or active tab"));
            }
            for tab in &window.tabs {
                total_panes = total_panes.saturating_add(tab.terminals.len());
                if tab.terminals.is_empty() || total_panes > MAX_SESSION_PANES {
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

/// Return the current UTC timestamp in ISO 8601 format (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn current_timestamp() -> String {
    iso_timestamp(SystemTime::now())
}

/// Formats `time` as an ISO 8601 UTC timestamp, `YYYY-MM-DDTHH:MM:SSZ`.
fn iso_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_from_days(secs / 86_400);
    let secs_today = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_today / 3600,
        (secs_today % 3600) / 60,
        secs_today % 60,
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, shifted so years start on 1 March and
/// the leap day falls at the end of the year.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Days since 0000-03-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

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
        session.windows[0].tabs[0].layout = Some(LayoutData::Split {
            direction: "Horizontal".to_owned(),
            children: vec![
                (LayoutData::Pane { index: 0 }, 0.5),
                (LayoutData::Pane { index: 0 }, 0.5),
            ],
        });
        assert!(session.validate().is_err());
        session.windows[0].tabs[0].layout = Some(LayoutData::Pane { index: 0 });
        assert!(session.validate().is_err());
        session.windows[0].tabs[0].layout = Some(LayoutData::Split {
            direction: "Horizontal".to_owned(),
            children: vec![
                (LayoutData::Pane { index: 0 }, 1.0),
                (LayoutData::Pane { index: 1 }, f32::INFINITY),
            ],
        });
        assert!(session.validate().is_err());
        session.windows[0].tabs[0].layout = None;
        session.version = 3;
        assert!(session.validate().is_err());
    }

    #[test]
    fn test_v2_validation_rejects_empty_windows_and_invalid_sizes() {
        let mut empty = SessionData::new();
        assert!(empty.validate().is_err());

        empty.windows.push(WindowData {
            tabs: vec![],
            active_tab_index: 0,
            size: None,
        });
        assert!(empty.validate().is_err());

        for bad_size in [
            WindowSize {
                width: 100.0,
                height: 600.0,
            },
            WindowSize {
                width: 800.0,
                height: 50.0,
            },
            WindowSize {
                width: f64::NAN,
                height: 600.0,
            },
            WindowSize {
                width: 800.0,
                height: f64::INFINITY,
            },
            WindowSize {
                width: 40_000.0,
                height: 600.0,
            },
        ] {
            let mut session = sample_session();
            session.windows[0].size = Some(bad_size);
            assert!(session.validate().is_err(), "{bad_size:?}");
        }
    }

    #[test]
    fn test_v1_file_loads_as_one_window_and_saves_as_v2() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy-v1.json");
        let v1_json = r#"{
            "version": 1,
            "timestamp": "2025-01-15T10:30:00Z",
            "tabs": [
                {
                    "title": "editor",
                    "working_directory": "/home/user/project",
                    "terminals": [
                        {"title": "nvim", "working_directory": "/home/user/project", "is_focused": true}
                    ],
                    "split_direction": "Horizontal"
                },
                {
                    "title": "shell",
                    "working_directory": "/tmp",
                    "terminals": [
                        {"title": "sh", "working_directory": "/tmp", "is_focused": true}
                    ],
                    "split_direction": "Vertical"
                }
            ],
            "active_tab_index": 1
        }"#;
        fs::write(&path, v1_json).unwrap();

        let loaded = SessionData::load(&path).unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.version, SESSION_VERSION);
        assert_eq!(loaded.windows.len(), 1);
        assert_eq!(loaded.windows[0].active_tab_index, 1);
        assert_eq!(loaded.windows[0].size, None);
        assert_eq!(loaded.windows[0].tabs.len(), 2);
        assert_eq!(loaded.windows[0].tabs[0].title, "editor");
        assert_eq!(loaded.windows[0].tabs[1].title, "shell");

        let v2_path = dir.path().join("rewritten-v2.json");
        loaded.save(&v2_path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&v2_path).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        assert!(raw.get("tabs").is_none());
        assert_eq!(raw["windows"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_v2_multi_window_session_round_trips_with_logical_sizes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("multi-window.json");
        let original = SessionData {
            version: SESSION_VERSION,
            timestamp: "2026-11-24T00:00:00Z".to_string(),
            windows: vec![
                WindowData {
                    tabs: sample_tabs(),
                    active_tab_index: 1,
                    size: Some(WindowSize {
                        width: 1024.0,
                        height: 768.0,
                    }),
                },
                WindowData {
                    tabs: vec![TabData {
                        title: "logs".to_string(),
                        working_directory: Some("/var/log".to_string()),
                        terminals: vec![TerminalData {
                            title: "tail".to_string(),
                            working_directory: Some("/var/log".to_string()),
                            is_focused: true,
                        }],
                        split_direction: "Horizontal".to_string(),
                        layout: Some(LayoutData::Pane { index: 0 }),
                    }],
                    active_tab_index: 0,
                    size: Some(WindowSize {
                        width: 900.0,
                        height: 540.0,
                    }),
                },
            ],
        };
        original.validate().unwrap();
        original.save(&path).unwrap();

        let loaded = SessionData::load(&path).unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.windows.len(), 2);
        assert_eq!(loaded.total_tab_count(), 3);
        assert_eq!(loaded.windows[0].active_tab_index, 1);
        assert_eq!(
            loaded.windows[0].size,
            Some(WindowSize {
                width: 1024.0,
                height: 768.0
            })
        );
        assert_eq!(loaded.windows[1].active_tab_index, 0);
        assert_eq!(
            loaded.windows[1].size,
            Some(WindowSize {
                width: 900.0,
                height: 540.0
            })
        );
        assert_eq!(loaded.windows[1].tabs[0].title, "logs");
    }

    #[test]
    fn test_malformed_version_schema_combinations_error() {
        assert!(serde_json::from_str::<SessionData>(
            r#"{"version":1,"timestamp":"ts","windows":[]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SessionData>(
            r#"{"version":2,"timestamp":"ts","tabs":[],"active_tab_index":0}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SessionData>(r#"{"version":2,"timestamp":"ts"}"#).is_err());
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
            SessionData::load(&path).unwrap().total_tab_count(),
            session.total_tab_count()
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn sample_tabs() -> Vec<TabData> {
        vec![
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
        ]
    }

    /// Build a minimal but complete SessionData for testing.
    fn sample_session() -> SessionData {
        SessionData {
            version: SESSION_VERSION,
            timestamp: "2025-01-15T10:30:00Z".to_string(),
            windows: vec![WindowData {
                tabs: sample_tabs(),
                active_tab_index: 0,
                size: Some(WindowSize {
                    width: 800.0,
                    height: 600.0,
                }),
            }],
        }
    }

    // ── SessionData::new / Default ───────────────────────────────────

    #[test]
    fn new_session_has_version_2() {
        let session = SessionData::new();
        assert_eq!(session.version, SESSION_VERSION);
    }

    #[test]
    fn new_session_starts_with_no_windows() {
        let session = SessionData::new();
        assert!(session.windows.is_empty());
        assert_eq!(session.total_tab_count(), 0);
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
        assert_eq!(from_new.windows.len(), from_default.windows.len());
        // timestamps may differ by a few nanoseconds but should both be non-empty
        assert!(!from_default.timestamp.is_empty());
    }

    // ── Timestamp format ─────────────────────────────────────────────

    #[test]
    fn iso_timestamp_uses_real_calendar_dates() {
        let at = |secs: u64| iso_timestamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        // Leap day, and the days around it.
        assert_eq!(at(1_709_164_799), "2024-02-28T23:59:59Z");
        assert_eq!(at(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(at(1_709_251_200), "2024-03-01T00:00:00Z");
        // 1900 and 2100 are not leap years; 2000 is.
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(4_107_542_400), "2100-03-01T00:00:00Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(at(1_767_225_599), "2025-12-31T23:59:59Z");
    }

    // ── Clone / Debug derive ─────────────────────────────────────────

    #[test]
    fn session_data_is_cloneable() {
        let session = sample_session();
        let cloned = session.clone();
        assert_eq!(cloned.version, session.version);
        assert_eq!(cloned.windows.len(), session.windows.len());
        assert_eq!(cloned.total_tab_count(), session.total_tab_count());
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
            version: SESSION_VERSION,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            windows: vec![],
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"version\":2"));
        assert!(json.contains("\"windows\":[]"));
    }

    #[test]
    fn roundtrip_sample_session() {
        let original = sample_session();
        let json = serde_json::to_string_pretty(&original).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.timestamp, original.timestamp);
        assert_eq!(loaded.windows.len(), 1);
        assert_eq!(
            loaded.windows[0].active_tab_index,
            original.windows[0].active_tab_index
        );
        assert_eq!(loaded.windows[0].size, original.windows[0].size);
        assert_eq!(loaded.windows[0].tabs.len(), 2);

        // First tab
        assert_eq!(loaded.windows[0].tabs[0].title, "editor");
        assert_eq!(
            loaded.windows[0].tabs[0].working_directory,
            Some("/home/user/project".to_string())
        );
        assert_eq!(loaded.windows[0].tabs[0].terminals.len(), 2);
        assert_eq!(loaded.windows[0].tabs[0].split_direction, "Horizontal");
        assert!(loaded.windows[0].tabs[0].terminals[0].is_focused);
        assert!(!loaded.windows[0].tabs[0].terminals[1].is_focused);

        // Second tab
        assert_eq!(loaded.windows[0].tabs[1].title, "shell");
        assert_eq!(loaded.windows[0].tabs[1].terminals.len(), 1);
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
        assert!(session.windows[0].tabs[0].working_directory.is_none());
        assert!(session.windows[0].tabs[0].terminals[0]
            .working_directory
            .is_none());
    }

    #[test]
    fn deserialize_future_version_preserves_version_for_validation() {
        let json = r#"{
            "version": 3,
            "timestamp": "ts",
            "windows": []
        }"#;
        let session: SessionData = serde_json::from_str(json).unwrap();
        assert_eq!(session.version, 3);
        assert!(session.validate().is_err());
    }

    #[test]
    fn deserialize_invalid_json_errors() {
        let result = serde_json::from_str::<SessionData>("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_wrong_types_errors() {
        let json = r#"{"version": "not_a_number", "timestamp": 123, "windows": []}"#;
        let result = serde_json::from_str::<SessionData>(json);
        assert!(result.is_err());
    }

    #[test]
    fn serialize_preserves_unicode() {
        let session = SessionData {
            version: SESSION_VERSION,
            timestamp: "ts".to_string(),
            windows: vec![WindowData {
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
                size: None,
            }],
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.windows[0].tabs[0].title, "tab with unicode");
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
            version: SESSION_VERSION,
            timestamp: "ts".to_string(),
            windows: vec![WindowData {
                tabs,
                active_tab_index: 50,
                size: None,
            }],
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.windows[0].tabs.len(), 100);
        assert_eq!(loaded.windows[0].active_tab_index, 50);
        assert_eq!(loaded.windows[0].tabs[99].title, "Tab 99");
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
        assert_eq!(parsed["version"], 2);
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
        assert_eq!(loaded.windows.len(), session.windows.len());
        assert_eq!(
            loaded.windows[0].active_tab_index,
            session.windows[0].active_tab_index
        );
        assert_eq!(loaded.windows[0].size, session.windows[0].size);

        for (orig_tab, loaded_tab) in session.windows[0]
            .tabs
            .iter()
            .zip(loaded.windows[0].tabs.iter())
        {
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
        session1.windows.push(WindowData {
            tabs: vec![TabData {
                title: "first".to_string(),
                working_directory: None,
                terminals: vec![],
                split_direction: "Horizontal".to_string(),
                layout: None,
            }],
            active_tab_index: 0,
            size: None,
        });
        session1.save(&path).unwrap();

        // Save second session with 2 tabs
        let mut session2 = SessionData::new();
        session2.windows.push(WindowData {
            tabs: vec![
                TabData {
                    title: "second-a".to_string(),
                    working_directory: None,
                    terminals: vec![],
                    split_direction: "Vertical".to_string(),
                    layout: None,
                },
                TabData {
                    title: "second-b".to_string(),
                    working_directory: None,
                    terminals: vec![],
                    split_direction: "Vertical".to_string(),
                    layout: None,
                },
            ],
            active_tab_index: 0,
            size: None,
        });
        session2.save(&path).unwrap();

        let loaded = SessionData::load(&path).unwrap();
        assert_eq!(loaded.windows[0].tabs.len(), 2);
        assert_eq!(loaded.windows[0].tabs[0].title, "second-a");
        assert_eq!(loaded.windows[0].tabs[1].title, "second-b");
    }

    #[test]
    fn save_empty_session() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.json");
        let session = SessionData::new();

        session.save(&path).unwrap();
        let loaded = SessionData::load(&path).unwrap();
        assert!(loaded.windows.is_empty());
        assert_eq!(loaded.version, SESSION_VERSION);
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
            windows: vec![WindowData {
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
                size: None,
            }],
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.windows[0].tabs[0].title, "");
        assert_eq!(
            loaded.windows[0].tabs[0].working_directory,
            Some("".to_string())
        );
        assert_eq!(loaded.windows[0].tabs[0].terminals[0].title, "");
    }

    #[test]
    fn large_active_tab_index() {
        let session = SessionData {
            version: SESSION_VERSION,
            timestamp: "ts".to_string(),
            windows: vec![WindowData {
                tabs: vec![],
                active_tab_index: usize::MAX,
                size: None,
            }],
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.windows[0].active_tab_index, usize::MAX);
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
        assert_eq!(loaded.windows.len(), original.windows.len());
        assert_eq!(
            loaded.windows[0].active_tab_index,
            original.windows[0].active_tab_index
        );
        assert_eq!(loaded.windows[0].tabs.len(), original.windows[0].tabs.len());
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
        s1.windows.push(WindowData {
            tabs: vec![TabData {
                title: "A".to_string(),
                working_directory: None,
                terminals: vec![],
                split_direction: "Horizontal".to_string(),
                layout: None,
            }],
            active_tab_index: 0,
            size: None,
        });

        let mut s2 = SessionData::new();
        s2.windows.push(WindowData {
            tabs: vec![TabData {
                title: "B".to_string(),
                working_directory: None,
                terminals: vec![],
                split_direction: "Vertical".to_string(),
                layout: None,
            }],
            active_tab_index: 0,
            size: None,
        });

        let p1 = tmp.path().join("s1.json");
        let p2 = tmp.path().join("s2.json");

        s1.save(&p1).unwrap();
        s2.save(&p2).unwrap();

        let l1 = SessionData::load(&p1).unwrap();
        let l2 = SessionData::load(&p2).unwrap();

        assert_eq!(l1.windows[0].tabs[0].title, "A");
        assert_eq!(l2.windows[0].tabs[0].title, "B");
    }
}
