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

    #[test]
    fn config_watcher_creation() {
        // This may or may not succeed depending on whether config exists
        let _watcher = ConfigWatcher::new();
    }
}
