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

/// How long the config file must stay unchanged before it is reloaded. An
/// editor's save is often several events (truncate, write, rename); reading
/// after the last of them avoids loading a half-written file.
const SETTLE: Duration = Duration::from_millis(100);

/// Watches the config file for changes.
///
/// Changes arrive on the watcher's own thread, which calls the waker given
/// to [`ConfigWatcher::new`]; the owner then calls [`ConfigWatcher::poll`]
/// and, while a reload is settling, wakes again at
/// [`ConfigWatcher::next_deadline`]. Nothing is polled while the file is
/// unchanged.
pub struct ConfigWatcher {
    /// File watcher
    _watcher: RecommendedWatcher,
    /// Receiver for config events
    receiver: Receiver<ConfigEvent>,
    /// When the latest change not yet reloaded was seen.
    changed_at: Option<Instant>,
}

impl ConfigWatcher {
    /// Create a new config watcher.
    ///
    /// `wake` is called from the watcher's thread after each change.
    ///
    /// Returns None if the config file doesn't exist or watching fails.
    pub fn new(wake: impl Fn() + Send + 'static) -> Option<Self> {
        let config_path = Config::config_path()?;

        // Ensure config file exists
        if !config_path.exists() {
            tracing::debug!("Config file does not exist, skipping watcher");
            return None;
        }
        Self::watch(&config_path, wake)
    }

    /// Watch the config file at `config_path`; see [`ConfigWatcher::new`].
    fn watch(config_path: &std::path::Path, wake: impl Fn() + Send + 'static) -> Option<Self> {
        let (tx, rx) = mpsc::channel();

        // Watch the parent directory (more reliable than watching the file directly)
        let watch_path = config_path.parent()?.to_path_buf();
        let config_filename = config_path.file_name()?.to_os_string();

        let sender = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Only react to modify/create events for our config file
                    let dominated_our_file = event
                        .paths
                        .iter()
                        .any(|p| p.file_name().map(|n| n == config_filename).unwrap_or(false));

                    if dominated_our_file {
                        use notify::EventKind;
                        match event.kind {
                            EventKind::Modify(_) | EventKind::Create(_) => {
                                let _ = sender.send(ConfigEvent::Reload);
                                wake();
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    let _ = sender.send(ConfigEvent::Error(e.to_string()));
                    wake();
                }
            }
        })
        .ok()?;

        watcher
            .watch(&watch_path, RecursiveMode::NonRecursive)
            .ok()?;

        tracing::info!("Watching config file: {:?}", config_path);

        Some(Self {
            _watcher: watcher,
            receiver: rx,
            changed_at: None,
        })
    }

    /// Collect pending change events (non-blocking).
    ///
    /// Returns true once the file has been unchanged for [`SETTLE`] after a
    /// change, which is when it should be reloaded.
    pub fn poll(&mut self, now: Instant) -> bool {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ConfigEvent::Reload => self.changed_at = Some(now),
                ConfigEvent::Error(e) => tracing::warn!("Config watcher error: {}", e),
            }
        }
        match self.changed_at {
            Some(at) if now.saturating_duration_since(at) >= SETTLE => {
                self.changed_at = None;
                true
            }
            _ => false,
        }
    }

    /// When a settling change becomes due for reload, if one is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.changed_at.map(|at| at + SETTLE)
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
        let _watcher = ConfigWatcher::new(|| {});
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

    fn manual_watcher() -> (ConfigWatcher, std::sync::mpsc::Sender<ConfigEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = ConfigWatcher {
            _watcher: notify::recommended_watcher(|_: Result<Event, notify::Error>| {}).unwrap(),
            receiver: rx,
            changed_at: None,
        };
        (watcher, tx)
    }

    #[test]
    fn reload_waits_for_the_file_to_settle() {
        let (mut watcher, tx) = manual_watcher();
        let now = Instant::now();
        assert_eq!(watcher.next_deadline(), None, "no change, no wakeup");

        tx.send(ConfigEvent::Reload).unwrap();
        assert!(!watcher.poll(now), "a fresh change is still settling");
        assert_eq!(watcher.next_deadline(), Some(now + SETTLE));

        // A second event of the same save restarts the wait.
        let later = now + SETTLE / 2;
        tx.send(ConfigEvent::Reload).unwrap();
        assert!(!watcher.poll(later));
        assert_eq!(watcher.next_deadline(), Some(later + SETTLE));

        assert!(watcher.poll(later + SETTLE), "settled: reload");
        assert_eq!(watcher.next_deadline(), None);
        assert!(!watcher.poll(later + SETTLE * 2), "reloads once per change");
    }

    #[test]
    fn poll_drains_errors_without_reload() {
        let (mut watcher, tx) = manual_watcher();
        tx.send(ConfigEvent::Error("test error".to_string()))
            .unwrap();
        assert!(
            !watcher.poll(Instant::now()),
            "errors should not trigger reload"
        );
        assert_eq!(watcher.next_deadline(), None);
    }

    #[test]
    fn a_change_on_disk_calls_the_waker() {
        let tmpdir = tempfile::tempdir().unwrap();
        let config_file = tmpdir.path().join("config.toml");
        fs::write(&config_file, "[font]\nsize = 14.0\n").unwrap();
        let (woke_tx, woke_rx) = std::sync::mpsc::channel();
        let woke_tx = std::sync::Mutex::new(woke_tx);
        let _watcher = ConfigWatcher::watch(&config_file, move || {
            let _ = woke_tx.lock().unwrap().send(());
        })
        .expect("watch the temp config");

        fs::write(&config_file, "[font]\nsize = 15.0\n").unwrap();
        assert!(
            woke_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "a write to the watched file wakes the owner"
        );
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
