//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod app;

fn main() -> Result<()> {
    if should_print_version(std::env::args().skip(1)) {
        println!("{}", version_output());
        return Ok(());
    }

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ultty=info,wgpu=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Ultty");

    // Run the application
    app::App::run()
}

fn should_print_version(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter()
        .any(|arg| arg == "--version" || arg == "-V")
}

fn version_output() -> String {
    format!("ultty {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{should_print_version, version_output};

    #[test]
    fn version_flag_is_detected() {
        assert!(should_print_version(["--version".to_string()]));
        assert!(should_print_version(["-V".to_string()]));
        assert!(should_print_version([
            "--config".to_string(),
            "path.toml".to_string(),
            "--version".to_string()
        ]));
    }

    #[test]
    fn unrelated_args_do_not_request_version() {
        assert!(!should_print_version(["--help".to_string()]));
        assert!(!should_print_version([
            "-e".to_string(),
            "bash".to_string()
        ]));
    }

    #[test]
    fn version_output_includes_binary_name_and_package_version() {
        assert_eq!(
            version_output(),
            format!("ultty {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
