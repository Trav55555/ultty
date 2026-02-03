//! Config file watcher for hot-reload functionality.

use crate::config::Config;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Config change event.
#[derive(Debug)]
pub enum ConfigEvent {
    /// Config file was modified and should be reloaded.
    Reload,
    /// Watcher encountered an error.
    Error(String),
}

/// Watches the config file for changes.
pub struct ConfigWatcher {
    /// File watcher
    _watcher: RecommendedWatcher,
    /// Receiver for config events
    receiver: Receiver<ConfigEvent>,
    /// Last reload time (for debouncing)
    last_reload: Instant,
    /// Debounce duration
    debounce: Duration,
}

impl ConfigWatcher {
    /// Create a new config watcher.
    ///
    /// Returns None if the config file doesn't exist or watching fails.
    pub fn new() -> Option<Self> {
        let config_path = Config::config_path()?;

        // Ensure config file exists
        if !config_path.exists() {
            tracing::debug!("Config file does not exist, skipping watcher");
            return None;
        }

        let (tx, rx) = mpsc::channel();

        // Watch the parent directory (more reliable than watching the file directly)
        let watch_path = config_path.parent()?.to_path_buf();
        let config_filename = config_path.file_name()?.to_os_string();

        let sender = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Only react to modify/create events for our config file
                    let dominated_our_file = event.paths.iter().any(|p| {
                        p.file_name().map(|n| n == config_filename).unwrap_or(false)
                    });

                    if dominated_our_file {
                        use notify::EventKind;
                        match event.kind {
                            EventKind::Modify(_) | EventKind::Create(_) => {
                                let _ = sender.send(ConfigEvent::Reload);
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    let _ = sender.send(ConfigEvent::Error(e.to_string()));
                }
            }
        }).ok()?;

        watcher.watch(&watch_path, RecursiveMode::NonRecursive).ok()?;

        tracing::info!("Watching config file: {:?}", config_path);

        Some(Self {
            _watcher: watcher,
            receiver: rx,
            last_reload: Instant::now() - Duration::from_secs(10), // Allow immediate first reload
            debounce: Duration::from_millis(500),
        })
    }

    /// Check for pending config events (non-blocking).
    ///
    /// Returns true if config should be reloaded.
    pub fn poll(&mut self) -> bool {
        let mut should_reload = false;

        // Drain all pending events
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ConfigEvent::Reload => {
                    should_reload = true;
                }
                ConfigEvent::Error(e) => {
                    tracing::warn!("Config watcher error: {}", e);
                }
            }
        }

        // Debounce: only reload if enough time has passed
        if should_reload {
            let now = Instant::now();
            if now.duration_since(self.last_reload) >= self.debounce {
                self.last_reload = now;
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

/// Get the config file path.
pub fn config_path() -> Option<PathBuf> {
    Config::config_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn config_watcher_creation() {
        // This may or may not succeed depending on whether config exists
        let _watcher = ConfigWatcher::new();
    }

    #[test]
    fn config_path_returns_some() {
        // config_path should return a valid path (even if file doesn't exist)
        let path = config_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains("ultty"));
        assert!(p.to_string_lossy().ends_with("config.toml"));
    }

    #[test]
    fn debounce_prevents_rapid_reloads() {
        // Create a temp directory with a config file to get a working watcher
        let tmpdir = tempfile::tempdir().unwrap();
        let config_file = tmpdir.path().join("config.toml");
        fs::write(&config_file, "[font]\nsize = 14.0\n").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();

        // Simulate a watcher with manual event injection
        let mut watcher_state = ConfigWatcher {
            _watcher: notify::recommended_watcher(|_: Result<Event, notify::Error>| {})
                .unwrap(),
            receiver: rx,
            last_reload: Instant::now(),
            debounce: Duration::from_millis(500),
        };

        // Send a reload event — should be debounced (just set last_reload to now)
        tx.send(ConfigEvent::Reload).unwrap();
        assert!(!watcher_state.poll(), "should be debounced");

        // Advance past debounce by setting last_reload to the past
        watcher_state.last_reload = Instant::now() - Duration::from_secs(1);
        tx.send(ConfigEvent::Reload).unwrap();
        assert!(watcher_state.poll(), "should allow reload after debounce");
    }

    #[test]
    fn poll_drains_errors_without_reload() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher_state = ConfigWatcher {
            _watcher: notify::recommended_watcher(|_: Result<Event, notify::Error>| {})
                .unwrap(),
            receiver: rx,
            last_reload: Instant::now() - Duration::from_secs(10),
            debounce: Duration::from_millis(500),
        };

        tx.send(ConfigEvent::Error("test error".to_string())).unwrap();
        assert!(!watcher_state.poll(), "errors should not trigger reload");
    }

    #[test]
    fn config_reload_roundtrip() {
        let tmpdir = tempfile::tempdir().unwrap();
        let config_file = tmpdir.path().join("config.toml");

        // Write a config and load it
        let mut f = fs::File::create(&config_file).unwrap();
        writeln!(f, "[font]\nsize = 20.0\nfamily = \"Test Font\"").unwrap();
        drop(f);

        let config = Config::load_from_path(&config_file.to_path_buf()).unwrap();
        assert_eq!(config.font.size, 20.0);
        assert_eq!(config.font.family, "Test Font");

        // Overwrite with different values
        fs::write(&config_file, "[font]\nsize = 12.0\nfamily = \"Other\"").unwrap();
        let config2 = Config::load_from_path(&config_file.to_path_buf()).unwrap();
        assert_eq!(config2.font.size, 12.0);
        assert_eq!(config2.font.family, "Other");
    }
}
