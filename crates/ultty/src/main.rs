//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use std::process::ExitCode;

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod app;

use app::StartupCommand;

/// The log filter when `RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "ultty=info,wgpu=warn";

fn main() -> Result<ExitCode> {
    let command = match invocation(std::env::args().skip(1)) {
        Ok(Invocation::Run { command }) => command,
        Ok(Invocation::Version) => {
            println!("{}", version_output());
            return Ok(ExitCode::SUCCESS);
        }
        Ok(Invocation::Help) => {
            print!("{}", usage());
            return Ok(ExitCode::SUCCESS);
        }
        Err(message) => {
            eprint!("ultty: {message}\n\n{}", usage());
            return Ok(ExitCode::from(2));
        }
    };

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
    app::App::run(command)?;
    Ok(ExitCode::SUCCESS)
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// Start the terminal. The first tab runs `command`, or the login shell
    /// when there is none.
    Run {
        command: Option<StartupCommand>,
    },
    Version,
    Help,
}

/// Parse the arguments after the program name.
///
/// `-e` (or `--`) ends option parsing: everything after it is the first
/// tab's program and its arguments, however much they look like flags.
/// `--help` wins over `--version`, and both win over a bad argument, so
/// neither ever starts a window. Any other argument is an error.
fn invocation(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let mut args = args.into_iter();
    let mut version = false;
    let mut unknown = None;
    let mut command = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Invocation::Help),
            "--version" | "-V" => version = true,
            "-e" | "--" => {
                command = Some(match args.next() {
                    Some(program) => Ok(StartupCommand {
                        program,
                        args: args.collect(),
                    }),
                    None => Err(format!("'{arg}' needs a command to run")),
                });
                break;
            }
            _ => {
                unknown.get_or_insert(arg);
            }
        }
    }
    if version {
        return Ok(Invocation::Version);
    }
    if let Some(arg) = unknown {
        return Err(format!("unknown argument '{arg}'"));
    }
    Ok(Invocation::Run {
        command: command.transpose()?,
    })
}

fn usage() -> String {
    format!(
        "\
GPU-accelerated terminal emulator. Opens a window running your shell.

Usage: ultty [OPTIONS] [-e COMMAND [ARGS]...]

Options:
  -e COMMAND [ARGS]...  Run COMMAND in the first tab instead of your shell.
                        Everything after -e is passed to COMMAND.
                        -- does the same.
  -h, --help            Print this help and exit
  -V, --version         Print the version and exit

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
    use super::{invocation, usage, version_output, Invocation, StartupCommand};

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn run_command(program: &str, command_args: &[&str]) -> Result<Invocation, String> {
        Ok(Invocation::Run {
            command: Some(StartupCommand {
                program: program.to_owned(),
                args: args(command_args),
            }),
        })
    }

    #[test]
    fn no_arguments_run_the_shell() {
        assert_eq!(invocation(args(&[])), Ok(Invocation::Run { command: None }));
    }

    #[test]
    fn version_flag_is_detected() {
        assert_eq!(invocation(args(&["--version"])), Ok(Invocation::Version));
        assert_eq!(invocation(args(&["-V"])), Ok(Invocation::Version));
    }

    #[test]
    fn help_flag_is_detected_and_wins_over_version() {
        assert_eq!(invocation(args(&["--help"])), Ok(Invocation::Help));
        assert_eq!(invocation(args(&["-h"])), Ok(Invocation::Help));
        assert_eq!(invocation(args(&["-V", "--help"])), Ok(Invocation::Help));
        assert_eq!(invocation(args(&["--version", "-h"])), Ok(Invocation::Help));
    }

    #[test]
    fn help_and_version_win_over_bad_arguments() {
        assert_eq!(invocation(args(&["--bogus", "-h"])), Ok(Invocation::Help));
        assert_eq!(
            invocation(args(&["--config", "path.toml", "--version"])),
            Ok(Invocation::Version)
        );
        assert_eq!(invocation(args(&["-V", "-e"])), Ok(Invocation::Version));
    }

    #[test]
    fn exec_flag_takes_the_command_and_its_arguments() {
        assert_eq!(invocation(args(&["-e", "htop"])), run_command("htop", &[]));
        assert_eq!(
            invocation(args(&["-e", "sh", "-c", "echo hi; sleep 5"])),
            run_command("sh", &["-c", "echo hi; sleep 5"])
        );
    }

    #[test]
    fn double_dash_means_the_same_as_exec_flag() {
        assert_eq!(
            invocation(args(&["--", "vim", "notes.txt"])),
            run_command("vim", &["notes.txt"])
        );
    }

    #[test]
    fn arguments_after_exec_flag_belong_to_the_command() {
        assert_eq!(
            invocation(args(&["-e", "less", "--help", "-V", "-e", "--", "--bogus"])),
            run_command("less", &["--help", "-V", "-e", "--", "--bogus"])
        );
        assert_eq!(invocation(args(&["-e", "-h"])), run_command("-h", &[]));
    }

    #[test]
    fn exec_flag_without_a_command_is_an_error() {
        assert_eq!(
            invocation(args(&["-e"])),
            Err("'-e' needs a command to run".to_owned())
        );
        assert_eq!(
            invocation(args(&["--"])),
            Err("'--' needs a command to run".to_owned())
        );
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(
            invocation(args(&["--bogus"])),
            Err("unknown argument '--bogus'".to_owned())
        );
        assert_eq!(
            invocation(args(&["bash"])),
            Err("unknown argument 'bash'".to_owned())
        );
        assert_eq!(
            invocation(args(&["stray", "-e", "bash"])),
            Err("unknown argument 'stray'".to_owned())
        );
    }

    #[test]
    fn usage_names_every_flag() {
        let usage = usage();
        for flag in ["-e COMMAND [ARGS]...", "-h, --help", "-V, --version"] {
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
