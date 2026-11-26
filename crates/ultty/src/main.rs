//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use std::process::ExitCode;

use anyhow::Result;

mod app;

use app::StartupCommand;

/// Counts Rust heap for soaks; see `ultty::heap_stats`.
#[cfg(feature = "heap-stats")]
#[global_allocator]
static GLOBAL: ultty::heap_stats::Counting = ultty::heap_stats::Counting;

fn main() -> Result<ExitCode> {
    let (command, new_process, quick) = match invocation(std::env::args().skip(1)) {
        Ok(Invocation::Attach { args }) => {
            let _log = ultty::logging::init(&ultty::config::LogConfig::load());
            ultty::viewer::cli::run(args)?;
            return Ok(ExitCode::SUCCESS);
        }
        Ok(Invocation::Run {
            command,
            new_process,
            quick,
        }) => (command, new_process, quick),
        Ok(Invocation::Version) => {
            println!("{}", version_output());
            return Ok(ExitCode::SUCCESS);
        }
        Ok(Invocation::ListThemes) => {
            use std::io::IsTerminal;
            let config = ultty::config::Config::load();
            let themes: Vec<_> = config
                .colors
                .theme_names()
                .into_iter()
                .filter_map(|name| config.colors.get_theme(&name).cloned().map(|t| (name, t)))
                .collect();
            print!(
                "{}",
                ultty::themes::format_listing(&themes, std::io::stdout().is_terminal())
            );
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

    let _log = ultty::logging::init(&ultty::config::LogConfig::load());

    let config = ultty::config::Config::load();
    if quick {
        if !new_process && command.is_none() {
            let token = ultty::control::single_instance::current_activation_token();
            if let ultty::control::single_instance::QuickHandoffOutcome::Toggled {
                pid,
                window,
                visible,
            } = ultty::control::single_instance::try_toggle_quick_in_existing_instance(token)
            {
                tracing::info!(
                    pid,
                    ?window,
                    visible,
                    "Toggled quick terminal in running Ultty instance"
                );
                return Ok(ExitCode::SUCCESS);
            }
        }
    } else if config.behavior.single_instance && !new_process {
        if let Some(launch) = single_instance_launch(&command) {
            if let ultty::control::single_instance::HandoffOutcome::Opened { pid, window } =
                ultty::control::single_instance::try_open_in_existing_instance(&launch)
            {
                tracing::info!(pid, window, "Opened new window in running Ultty instance");
                return Ok(ExitCode::SUCCESS);
            }
        }
    }

    tracing::info!("Starting Ultty");

    if config.behavior.narrow_vulkan_icd {
        ultty::platform::vulkan_icd::narrow_to_detected_gpu();
    }

    app::App::run(command, config, quick)?;
    Ok(ExitCode::SUCCESS)
}

fn single_instance_launch(
    command: &Option<StartupCommand>,
) -> Option<ultty::control::single_instance::NewWindowLaunch> {
    let working_dir = std::env::current_dir()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok());
    match command {
        None => Some(ultty::control::single_instance::NewWindowLaunch {
            command: None,
            args: Vec::new(),
            working_dir,
        }),
        Some(StartupCommand::Exec { program, args }) => {
            Some(ultty::control::single_instance::NewWindowLaunch {
                command: Some(program.clone()),
                args: args.clone(),
                working_dir,
            })
        }
        Some(StartupCommand::Profile(_) | StartupCommand::Workspace(_)) => None,
    }
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// Attach a viewer to a persistent local or SSH owner.
    Attach {
        args: Vec<String>,
    },
    /// Start the terminal. The first tab runs `command`, or the login shell
    /// when there is none.
    Run {
        command: Option<StartupCommand>,
        new_process: bool,
        quick: bool,
    },
    Version,
    /// Print the theme names (with colour swatches on a terminal).
    ListThemes,
    Help,
}

/// Parse the arguments after the program name.
///
/// `-e` (or `--`) ends option parsing: everything after it is the first
/// tab's program and its arguments, however much they look like flags.
/// `--help` wins over `--version`, and both win over a bad argument, so
/// neither ever starts a window. Any other argument is an error.
fn invocation(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let mut version = false;
    let mut list_themes = false;
    let mut new_process = false;
    let mut quick = false;
    let mut unknown = None;
    let mut command = None;
    let mut selection_error = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--attach" if command.is_none() && unknown.is_none() && !version => {
                return Ok(Invocation::Attach {
                    args: args.collect(),
                });
            }
            "--help" | "-h" => return Ok(Invocation::Help),
            "--version" | "-V" => version = true,
            "--list-themes" => list_themes = true,
            "--new-process" => new_process = true,
            "--quick" => quick = true,
            "--profile" | "--workspace" => {
                if command.is_some() {
                    selection_error =
                        Some("choose one profile, workspace or -e command".to_owned());
                }
                command = Some(
                    if args
                        .peek()
                        .is_some_and(|name| !name.is_empty() && !name.starts_with('-'))
                    {
                        args.next()
                            .map(|name| {
                                if arg == "--profile" {
                                    StartupCommand::Profile(name)
                                } else {
                                    StartupCommand::Workspace(name)
                                }
                            })
                            .ok_or_else(|| format!("'{arg}' needs a name"))
                    } else {
                        Err(format!("'{arg}' needs a name"))
                    },
                );
            }
            "-e" | "--" => {
                if command.is_some() {
                    selection_error =
                        Some("choose one profile, workspace or -e command".to_owned());
                }
                command = Some(match args.next() {
                    Some(program) => Ok(StartupCommand::Exec {
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
    if list_themes {
        return Ok(Invocation::ListThemes);
    }
    if let Some(arg) = unknown {
        return Err(format!("unknown argument '{arg}'"));
    }
    if let Some(error) = selection_error {
        return Err(error);
    }
    let command = command.transpose()?;
    if quick
        && matches!(
            command,
            Some(StartupCommand::Profile(_) | StartupCommand::Workspace(_))
        )
    {
        return Err("'--quick' cannot be combined with '--profile' or '--workspace'".to_owned());
    }
    Ok(Invocation::Run {
        command,
        new_process,
        quick,
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
                        Does not restore or save the session.
                        -- does the same.
  -h, --help            Print this help and exit
  -V, --version         Print the version and exit
  --list-themes         Print the available colour themes and exit
  --new-process         Always start a new process even with behavior.single_instance.
  --quick               Toggle the running instance's quick terminal, or start one.
  --profile NAME        Launch configured tabs and panes instead of restoring.
  --workspace NAME      Open a saved layout in fresh shells; does not resume jobs.
  --attach NAME         Attach to a live session host; close the window to detach.

Configuration: ~/.config/ultty/config.toml
Your themes:   ~/.config/ultty/themes/NAME.toml
Logging:       RUST_LOG or [log] level (default \"{filter}\")
Log file:      ~/.local/state/ultty/ultty.log; ULTTY_LOG_FILE=0 turns it off
Control a running Ultty with ulttyctl (ulttyctl --help).

{version}
",
        filter = ultty::logging::DEFAULT_LOG_FILTER,
        version = version_output()
    )
}

fn version_output() -> String {
    format!("ultty {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{
        invocation, single_instance_launch, usage, version_output, Invocation, StartupCommand,
    };

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn run_command(program: &str, command_args: &[&str]) -> Result<Invocation, String> {
        Ok(Invocation::Run {
            command: Some(StartupCommand::Exec {
                program: program.to_owned(),
                args: args(command_args),
            }),
            new_process: false,
            quick: false,
        })
    }

    #[test]
    fn no_arguments_run_the_shell() {
        assert_eq!(
            invocation(args(&[])),
            Ok(Invocation::Run {
                command: None,
                new_process: false,
                quick: false,
            })
        );
    }

    #[test]
    fn test_new_process_flag_parses_and_combines_with_exec() {
        assert_eq!(
            invocation(args(&["--new-process"])),
            Ok(Invocation::Run {
                command: None,
                new_process: true,
                quick: false,
            })
        );
        assert_eq!(
            invocation(args(&["--new-process", "-e", "sh", "-c", "true"])),
            Ok(Invocation::Run {
                command: Some(StartupCommand::Exec {
                    program: "sh".to_owned(),
                    args: args(&["-c", "true"]),
                }),
                new_process: true,
                quick: false,
            })
        );
        assert_eq!(
            invocation(args(&["-e", "echo", "--new-process"])),
            run_command("echo", &["--new-process"])
        );
    }

    #[test]
    fn test_quick_flag_parses_and_rejects_profile_or_workspace() {
        assert_eq!(
            invocation(args(&["--quick"])),
            Ok(Invocation::Run {
                command: None,
                new_process: false,
                quick: true,
            })
        );
        assert_eq!(
            invocation(args(&["--quick", "--new-process"])),
            Ok(Invocation::Run {
                command: None,
                new_process: true,
                quick: true,
            })
        );
        assert!(invocation(args(&["--quick", "--profile", "dev"])).is_err());
        assert!(invocation(args(&["--quick", "--workspace", "work"])).is_err());
    }

    #[test]
    fn single_instance_launch_maps_shell_and_exec_but_skips_profile_and_workspace() {
        let shell = single_instance_launch(&None).expect("bare launch hands off");
        assert!(shell.command.is_none());
        assert!(shell.args.is_empty());
        assert!(shell.working_dir.is_some());

        let exec = single_instance_launch(&Some(StartupCommand::Exec {
            program: "htop".to_owned(),
            args: args(&["-d", "5"]),
        }))
        .expect("-e launch hands off");
        assert_eq!(exec.command.as_deref(), Some("htop"));
        assert_eq!(exec.args, args(&["-d", "5"]));

        assert!(
            single_instance_launch(&Some(StartupCommand::Profile("work".to_owned()))).is_none()
        );
        assert!(
            single_instance_launch(&Some(StartupCommand::Workspace("dev".to_owned()))).is_none()
        );
    }

    #[test]
    fn test_profile_selection_and_conflicts() {
        assert_eq!(
            invocation(args(&["--profile", "remote"])),
            Ok(Invocation::Run {
                command: Some(StartupCommand::Profile("remote".to_owned())),
                new_process: false,
                quick: false,
            })
        );
        assert!(invocation(args(&["--profile"])).is_err());
        assert_eq!(
            invocation(args(&["--profile", "--help"])),
            Ok(Invocation::Help)
        );
        assert!(invocation(args(&["--profile", "a", "--profile", "b"])).is_err());
        assert!(invocation(args(&["--profile", "remote", "-e", "sh"])).is_err());
        assert_eq!(
            invocation(args(&["-e", "echo", "--profile", "remote"])),
            run_command("echo", &["--profile", "remote"])
        );
    }

    #[test]
    fn version_flag_is_detected() {
        assert_eq!(invocation(args(&["--version"])), Ok(Invocation::Version));
        assert_eq!(invocation(args(&["-V"])), Ok(Invocation::Version));
    }

    #[test]
    fn list_themes_is_an_information_flag_like_version() {
        assert_eq!(
            invocation(args(&["--list-themes"])),
            Ok(Invocation::ListThemes)
        );
        assert_eq!(
            invocation(args(&["--list-themes", "--bogus"])),
            Ok(Invocation::ListThemes)
        );
        assert_eq!(
            invocation(args(&["--list-themes", "-h"])),
            Ok(Invocation::Help)
        );
        assert_eq!(
            invocation(args(&["--list-themes", "-V"])),
            Ok(Invocation::Version)
        );
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
        for flag in [
            "-e COMMAND [ARGS]...",
            "-h, --help",
            "-V, --version",
            "--list-themes",
            "--new-process",
            "--quick",
        ] {
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
