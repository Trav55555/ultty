//! Where Ultty's tracing output goes: stderr, and a persistent log file.
//!
//! The file is `$XDG_STATE_HOME/ultty/ultty.log` (or
//! `~/.local/state/ultty/ultty.log`), written in the background by
//! [`background::BackgroundWriter`] and rotated by size by
//! [`rotating::RotatingFile`]. Several Ultty processes may share it; each
//! line starts with `pid=<pid>`. The `[log]` config section, `RUST_LOG` and
//! `ULTTY_LOG_FILE` choose the filter and the file; see [`LogSettings`].

pub mod background;
pub mod rotating;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use crate::config::LogConfig;
use background::{BackgroundWriter, LogGuard};
use rotating::RotatingFile;

/// The log filter when neither `RUST_LOG` nor `[log] level` sets one.
pub const DEFAULT_LOG_FILTER: &str = "ultty=info,wgpu=warn";

/// Rotate the log file once it reaches this size.
pub const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Rotated log files kept (`ultty.log.1` to `ultty.log.3`).
pub const KEPT_LOG_FILES: usize = 3;

/// The log filter and file in effect, resolved from config and environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSettings {
    /// Tracing filter directives, as in `RUST_LOG`.
    pub filter: String,
    /// The log file, or `None` when file logging is off.
    pub file: Option<PathBuf>,
}

/// The environment variables [`LogSettings::resolve`] reads.
#[derive(Debug, Clone, Default)]
pub struct LogEnvironment {
    /// `RUST_LOG`: overrides `[log] level` when set and not empty.
    pub rust_log: Option<OsString>,
    /// `ULTTY_LOG_FILE`: `0` turns the file off, `1` turns it on at the
    /// default path, anything else is the file's path.
    pub ultty_log_file: Option<OsString>,
    /// `XDG_STATE_HOME`, used when it is an absolute path.
    pub xdg_state_home: Option<OsString>,
    /// The home directory, for the `~/.local/state` fallback.
    pub home: Option<PathBuf>,
}

impl LogEnvironment {
    /// This process's environment.
    pub fn current() -> Self {
        Self {
            rust_log: std::env::var_os("RUST_LOG"),
            ultty_log_file: std::env::var_os("ULTTY_LOG_FILE"),
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            home: dirs::home_dir(),
        }
    }

    /// `$XDG_STATE_HOME/ultty/ultty.log`, or `~/.local/state/ultty/ultty.log`
    /// when `XDG_STATE_HOME` is unset or not absolute (the XDG rule).
    pub fn default_log_path(&self) -> Option<PathBuf> {
        let state = self
            .xdg_state_home
            .as_ref()
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| self.home.as_ref().map(|home| home.join(".local/state")))?;
        Some(state.join("ultty").join("ultty.log"))
    }
}

impl LogSettings {
    /// Combine `[log]` with the environment, which wins.
    pub fn resolve(config: &LogConfig, env: &LogEnvironment) -> Self {
        let filter = env
            .rust_log
            .as_ref()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| config.level.clone())
            .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_owned());
        let file = match env.ultty_log_file.as_deref() {
            Some(value) if value == "0" => None,
            Some(value) if value == "1" => env.default_log_path(),
            Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
            _ if !config.file => None,
            _ => config.path.clone().or_else(|| env.default_log_path()),
        };
        Self { filter, file }
    }
}

/// Install the global tracing subscriber: stderr, plus the log file when
/// `[log]` and the environment leave it on. Logs where the file is, or why
/// it could not be opened.
///
/// Keep the returned guard alive for the life of the process: dropping it
/// flushes lines still queued for the file.
pub fn init(config: &LogConfig) -> Option<LogGuard> {
    let settings = LogSettings::resolve(config, &LogEnvironment::current());
    let (filter, bad_filter) = match EnvFilter::try_new(&settings.filter) {
        Ok(filter) => (filter, None),
        Err(error) => (EnvFilter::new(DEFAULT_LOG_FILTER), Some(error)),
    };
    let mut guard = None;
    let mut file_layer = None;
    let mut file_error = None;
    if let Some(path) = &settings.file {
        match open_file(path) {
            Ok((writer, opened)) => {
                file_layer = Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer),
                );
                guard = Some(opened);
            }
            Err(error) => file_error = Some(error),
        }
    }
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .and_then(file_layer)
                .with_filter(filter),
        )
        .init();

    if let Some(error) = bad_filter {
        tracing::warn!(
            filter = %settings.filter,
            %error,
            "Ignoring the log filter; using {DEFAULT_LOG_FILTER}"
        );
    }
    match (&settings.file, file_error) {
        (Some(path), None) => tracing::info!(path = %path.display(), "Logging to the log file"),
        (Some(path), Some(error)) => tracing::warn!(
            path = %path.display(),
            %error,
            "Could not open the log file; logging to stderr only"
        ),
        (None, _) => tracing::info!("File logging is off"),
    }
    guard
}

fn open_file(path: &Path) -> std::io::Result<(BackgroundWriter, LogGuard)> {
    let file = RotatingFile::open(path, MAX_LOG_BYTES, KEPT_LOG_FILES)?;
    BackgroundWriter::spawn(file, std::process::id(), background::DEFAULT_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(rust_log: Option<&str>, log_file: Option<&str>) -> LogEnvironment {
        LogEnvironment {
            rust_log: rust_log.map(OsString::from),
            ultty_log_file: log_file.map(OsString::from),
            xdg_state_home: Some("/state".into()),
            home: Some("/home/ana".into()),
        }
    }

    fn config(toml: &str) -> LogConfig {
        toml::from_str::<crate::config::Config>(toml).unwrap().log
    }

    #[test]
    fn test_log_settings_default_to_the_state_dir_file_at_info() {
        let settings = LogSettings::resolve(&LogConfig::default(), &env(None, None));
        assert_eq!(settings.filter, DEFAULT_LOG_FILTER);
        assert_eq!(
            settings.file.as_deref(),
            Some(Path::new("/state/ultty/ultty.log"))
        );
    }

    #[test]
    fn test_log_settings_follow_the_xdg_fallback_rule() {
        let mut env = env(None, None);
        env.xdg_state_home = Some("relative/state".into());
        assert_eq!(
            env.default_log_path().as_deref(),
            Some(Path::new("/home/ana/.local/state/ultty/ultty.log"))
        );
        env.xdg_state_home = None;
        assert_eq!(
            env.default_log_path().as_deref(),
            Some(Path::new("/home/ana/.local/state/ultty/ultty.log"))
        );
        env.home = None;
        assert_eq!(env.default_log_path(), None);
    }

    #[test]
    fn test_log_section_parses_file_path_and_level() {
        let log = config("[log]\nfile = false\npath = \"/tmp/u.log\"\nlevel = \"ultty=debug\"\n");
        assert!(!log.file);
        assert_eq!(log.path.as_deref(), Some(Path::new("/tmp/u.log")));
        assert_eq!(log.level.as_deref(), Some("ultty=debug"));
        let absent = config("[behavior]\nopacity = 1.0\n");
        assert!(absent.file, "file logging is on by default");
        assert_eq!(absent.path, None);
        assert_eq!(absent.level, None);
    }

    #[test]
    fn test_log_settings_use_the_configured_path_and_level() {
        let log = config("[log]\npath = \"/var/tmp/u.log\"\nlevel = \"debug\"\n");
        let settings = LogSettings::resolve(&log, &env(None, None));
        assert_eq!(settings.filter, "debug");
        assert_eq!(settings.file.as_deref(), Some(Path::new("/var/tmp/u.log")));
        let off = config("[log]\nfile = false\npath = \"/var/tmp/u.log\"\n");
        assert_eq!(LogSettings::resolve(&off, &env(None, None)).file, None);
    }

    #[test]
    fn test_rust_log_overrides_the_configured_level_unless_empty() {
        let log = config("[log]\nlevel = \"warn\"\n");
        let set = LogSettings::resolve(&log, &env(Some("ultty=trace"), None));
        assert_eq!(set.filter, "ultty=trace");
        let empty = LogSettings::resolve(&log, &env(Some("  "), None));
        assert_eq!(empty.filter, "warn");
    }

    #[test]
    fn test_ultty_log_file_disables_redirects_or_forces_the_file() {
        let log = config("[log]\npath = \"/var/tmp/u.log\"\n");
        assert_eq!(LogSettings::resolve(&log, &env(None, Some("0"))).file, None);
        assert_eq!(
            LogSettings::resolve(&log, &env(None, Some("/run/test.log"))).file,
            Some(PathBuf::from("/run/test.log"))
        );
        let off = config("[log]\nfile = false\n");
        assert_eq!(
            LogSettings::resolve(&off, &env(None, Some("1"))).file,
            Some(PathBuf::from("/state/ultty/ultty.log"))
        );
        assert_eq!(
            LogSettings::resolve(&log, &env(None, Some(""))).file,
            Some(PathBuf::from("/var/tmp/u.log")),
            "an empty ULTTY_LOG_FILE counts as unset"
        );
    }
}
