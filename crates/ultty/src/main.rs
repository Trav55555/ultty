//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod app;
mod platform;
mod protocol;
mod renderer;
mod scene;
pub mod vim;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ultty=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Ultty");

    // TODO: Initialize winit event loop
    // TODO: Create wgpu surface
    // TODO: Start protocol listener
    // TODO: Enter main loop

    tracing::warn!("Terminal emulator not yet implemented");
    Ok(())
}
