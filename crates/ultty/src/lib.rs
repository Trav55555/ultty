//! Ultty - GPU-accelerated terminal emulator
//!
//! This library provides the core terminal emulation functionality
//! including ANSI parsing, scene graph management, and rendering.

// Core modules (must be first as others depend on them)
pub mod config;
pub mod config_watcher;
pub mod grid;
pub mod protocol;
pub mod pty;

// Higher-level modules
pub mod event_loop;
pub mod renderer;
pub mod scene;
pub mod tabs;
pub mod terminal_manager;
pub mod tmux;
pub mod url;
pub mod vim;
