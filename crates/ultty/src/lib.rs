//! # Ultty - GPU-accelerated Terminal Emulator
//!
//! Ultty is a modern terminal emulator with GPU rendering and support for
//! a native binary protocol that enables high-performance TUI applications.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Ultty Terminal                           │
//! ├─────────────────────────────────────────────────────────────┤
//! │  [EventLoop] ──► [Terminal] ──► [Renderer]                  │
//! │       │              │              │                       │
//! │       ▼              ▼              ▼                       │
//! │  PTY/Socket     Grid/Buffer    GPU (wgpu)                   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Components
//!
//! - **[`event_loop`]**: Main event loop handling PTY I/O and input
//! - **[`grid`]**: Terminal grid, cells, and ANSI parsing
//! - **[`renderer`]**: GPU-accelerated rendering via wgpu
//! - **[`protocol`]**: Native binary protocol for TUI apps
//! - **[`config`]**: Configuration and theming
//!
//! ## Crate Features
//!
//! This crate is primarily used as the ultty binary, but can also be used
//! as a library for:
//! - Embedding terminal emulation in other applications
//! - Headless terminal testing
//! - Custom terminal implementations

#[cfg(not(unix))]
compile_error!(
    "ultty is currently a Unix-first runtime. Windows support requires future ConPTY and native transport work."
);

pub mod a11y;
pub mod config;
pub mod config_watcher;
pub mod control;
/// The terminal screen model, re-exported from [`ultty_vt`].
pub use ultty_vt::grid;
pub mod platform;
pub mod protocol;
pub mod pty;

pub mod event_loop;
pub mod keys;
pub mod pane_layout;
pub mod renderer;
pub mod scene;
pub mod session;
pub mod tabs;
pub mod terminal_manager;
pub mod testing;
pub mod url;
pub mod vim;
