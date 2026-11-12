//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod app;

/// The log filter when `RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "ultty=info,wgpu=warn";

fn main() -> Result<()> {
    match invocation(std::env::args().skip(1)) {
        Invocation::Run => {}
        Invocation::Version => {
            println!("{}", version_output());
            return Ok(());
        }
        Invocation::Help => {
            print!("{}", usage());
            return Ok(());
        }
    }

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Ultty");

    // Run the application
    app::App::run()
}

/// What the command line asks for. Ultty takes no arguments besides
/// `--help` and `--version`; anything else still starts the terminal.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Run,
    Version,
    Help,
}

/// `--help` wins over `--version` wherever either appears, so neither ever
/// starts a window.
fn invocation(args: impl IntoIterator<Item = String>) -> Invocation {
    let mut found = Invocation::Run;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Invocation::Help,
            "--version" | "-V" => found = Invocation::Version,
            _ => {}
        }
    }
    found
}

fn usage() -> String {
    format!(
        "\
GPU-accelerated terminal emulator. Opens a window running your shell.

Usage: ultty [OPTIONS]

Options:
  -h, --help     Print this help and exit
  -V, --version  Print the version and exit

Configuration: ~/.config/ultty/config.toml
Logging:       RUST_LOG (default \"{DEFAULT_LOG_FILTER}\")
Control a running Ultty with ulttyctl (ulttyctl --help).

{version}
",
        version = version_output()
    )
}

fn version_output() -> String {
    format!("ultty {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{invocation, usage, version_output, Invocation};

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn no_arguments_run_the_terminal() {
        assert_eq!(invocation(args(&[])), Invocation::Run);
    }

    #[test]
    fn version_flag_is_detected() {
        assert_eq!(invocation(args(&["--version"])), Invocation::Version);
        assert_eq!(invocation(args(&["-V"])), Invocation::Version);
        assert_eq!(
            invocation(args(&["--config", "path.toml", "--version"])),
            Invocation::Version
        );
    }

    #[test]
    fn help_flag_is_detected_and_wins_over_version() {
        assert_eq!(invocation(args(&["--help"])), Invocation::Help);
        assert_eq!(invocation(args(&["-h"])), Invocation::Help);
        assert_eq!(invocation(args(&["-V", "--help"])), Invocation::Help);
        assert_eq!(invocation(args(&["--version", "-h"])), Invocation::Help);
    }

    #[test]
    fn unrelated_args_run_the_terminal() {
        assert_eq!(invocation(args(&["-e", "bash"])), Invocation::Run);
    }

    #[test]
    fn usage_names_every_flag() {
        let usage = usage();
        for flag in ["-h, --help", "-V, --version"] {
            assert!(usage.contains(flag), "{usage}");
        }
        assert!(usage.contains("Usage: ultty"), "{usage}");
    }

    #[test]
    fn version_output_includes_binary_name_and_package_version() {
        assert_eq!(
            version_output(),
            format!("ultty {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
