//! Pseudo-terminal (PTY) handling for shell processes.
//!
//! This module provides functionality to spawn and manage shell processes
//! through a pseudo-terminal, enabling full terminal emulation.

mod child;
mod master;
pub mod shell_integration;

pub use child::Child;
pub use master::{Pty, PtySize, SlavePty};

use std::io;
use thiserror::Error;

/// PTY-related errors.
#[derive(Debug, Error)]
pub enum PtyError {
    /// Child setup or exec failed before the requested program began.
    #[error("child {stage} failed: {source}")]
    StartupFailed {
        /// Setup operation which failed.
        stage: &'static str,
        /// OS error captured in the forked child.
        source: io::Error,
    },
    #[error("failed to open PTY: {0}")]
    OpenFailed(#[source] nix::Error),

    #[error("failed to fork process: {0}")]
    ForkFailed(#[source] nix::Error),

    #[error("failed to execute shell: {0}")]
    ExecFailed(#[source] nix::Error),

    #[error("failed to set terminal size: {0}")]
    ResizeFailed(#[source] nix::Error),

    #[error("failed to read from PTY: {0}")]
    ReadFailed(#[source] io::Error),

    #[error("failed to write to PTY: {0}")]
    WriteFailed(#[source] io::Error),

    #[error("child process exited with status {0}")]
    ChildExited(i32),

    #[error("child process killed by signal {0}")]
    ChildSignaled(i32),

    #[error("PTY closed")]
    Closed,
}

/// Result type for PTY operations.
pub type Result<T> = std::result::Result<T, PtyError>;

/// Get the user's default shell.
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Get the current working directory.
pub fn current_dir() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"))
}
