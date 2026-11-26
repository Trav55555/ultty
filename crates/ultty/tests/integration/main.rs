//! Consolidated integration test binary for `ultty`.

#[path = "../support/tmux_server.rs"]
mod tmux_server;

mod app_compat;
mod app_tests;
mod config_reload;
mod prompt_sp;
mod proptest_ansi;
mod self_check;
mod shell_integration;
mod shutdown_signals;
mod ulttui_layout;
mod ulttyctl_targets;
mod ulttyctl_verbose;
