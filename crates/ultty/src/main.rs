//! Ultty - GPU-accelerated terminal emulator
//!
//! This is the entry point for the terminal emulator binary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;

mod app;

use app::{LaunchOptions, StartupCommand};

/// Counts Rust heap for soaks; see `ultty::heap_stats`.
#[cfg(feature = "heap-stats")]
#[global_allocator]
static GLOBAL: ultty::heap_stats::Counting = ultty::heap_stats::Counting;

fn main() -> Result<ExitCode> {
    let (command, new_process, quick, config_path, config_overrides, working_dir) =
        match invocation(std::env::args().skip(1)) {
            Ok(Invocation::Attach { args }) => {
                let _log = ultty::logging::init(&ultty::config::LogConfig::load());
                ultty::viewer::cli::run(args)?;
                return Ok(ExitCode::SUCCESS);
            }
            Ok(Invocation::Run {
                command,
                new_process,
                quick,
                config_path,
                config_overrides,
                working_dir,
            }) => (
                command,
                new_process,
                quick,
                config_path,
                config_overrides,
                working_dir,
            ),
            Ok(Invocation::Version) => {
                println!("{}", version_output());
                return Ok(ExitCode::SUCCESS);
            }
            Ok(Invocation::ListThemes {
                config_path,
                config_overrides,
            }) => {
                use std::io::IsTerminal;
                let config_path = match config_path.map(resolve_path).transpose() {
                    Ok(path) => path,
                    Err(err) => {
                        eprintln!("ultty: {err}");
                        return Ok(ExitCode::from(2));
                    }
                };
                let config = match ultty::config::Config::load_with_options(
                    config_path.as_deref(),
                    &config_overrides,
                ) {
                    Ok(config) => config,
                    Err(err) => {
                        eprintln!("ultty: {err}");
                        return Ok(ExitCode::from(2));
                    }
                };
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

    let config_path = match config_path.map(resolve_path).transpose() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("ultty: {err}");
            return Ok(ExitCode::from(2));
        }
    };
    let working_dir = match working_dir.map(resolve_working_directory).transpose() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("ultty: {err}");
            return Ok(ExitCode::from(2));
        }
    };
    let config =
        match ultty::config::Config::load_with_options(config_path.as_deref(), &config_overrides) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("ultty: {err}");
                return Ok(ExitCode::from(2));
            }
        };

    let _log = ultty::logging::init(&config.log);

    let custom_config = config_path.is_some() || !config_overrides.is_empty();
    if quick {
        if !new_process && !custom_config && command.is_none() && working_dir.is_none() {
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
    } else if config.behavior.single_instance && !new_process && !custom_config {
        if let Some(launch) = single_instance_launch(&command, working_dir.as_deref()) {
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

    let options = LaunchOptions {
        config_path,
        config_overrides,
        working_dir,
        start_quick: quick,
    };
    app::App::run(command, config, options)?;
    Ok(ExitCode::SUCCESS)
}

fn resolve_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .map_err(|err| format!("cannot resolve '{}': {err}", path.display()))
}

fn resolve_working_directory(dir: PathBuf) -> Result<PathBuf, String> {
    let resolved = resolve_path(dir)?;
    match std::fs::metadata(&resolved) {
        Ok(meta) if meta.is_dir() => Ok(resolved),
        Ok(_) => Err(format!(
            "working directory '{}' is not a directory",
            resolved.display()
        )),
        Err(err) => Err(format!("working directory '{}': {err}", resolved.display())),
    }
}

fn single_instance_launch(
    command: &Option<StartupCommand>,
    explicit_working_dir: Option<&Path>,
) -> Option<ultty::control::single_instance::NewWindowLaunch> {
    let working_dir = explicit_working_dir
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| path.into_os_string().into_string().ok())
        });
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
        config_path: Option<PathBuf>,
        config_overrides: Vec<String>,
        working_dir: Option<PathBuf>,
    },
    Version,
    /// Print the theme names (with colour swatches on a terminal).
    ListThemes {
        config_path: Option<PathBuf>,
        config_overrides: Vec<String>,
    },
    Help,
}

fn take_flag_value<I>(
    args: &mut std::iter::Peekable<I>,
    flag: &str,
    expected: &str,
) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    if args
        .peek()
        .is_some_and(|value| !value.is_empty() && !value.starts_with('-'))
    {
        args.next()
            .ok_or_else(|| format!("'{flag}' needs {expected}"))
    } else {
        Err(format!("'{flag}' needs {expected}"))
    }
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
    let mut config_path = None;
    let mut config_overrides = Vec::new();
    let mut working_dir = None;
    let mut unknown = None;
    let mut command = None;
    let mut option_error = None;
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
            "--config" => match take_flag_value(&mut args, "--config", "a file path") {
                Ok(path) => config_path = Some(PathBuf::from(path)),
                Err(err) => {
                    option_error.get_or_insert(err);
                }
            },
            "--working-directory" => {
                match take_flag_value(&mut args, "--working-directory", "a directory") {
                    Ok(dir) => working_dir = Some(PathBuf::from(dir)),
                    Err(err) => {
                        option_error.get_or_insert(err);
                    }
                }
            }
            "-o" | "--option" => match take_flag_value(&mut args, &arg, "KEY=VALUE") {
                Ok(spec) => config_overrides.push(spec),
                Err(err) => {
                    option_error.get_or_insert(err);
                }
            },
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
                if let Some(value) = arg.strip_prefix("--config=") {
                    if value.is_empty() {
                        option_error
                            .get_or_insert_with(|| "'--config' needs a file path".to_owned());
                    } else {
                        config_path = Some(PathBuf::from(value));
                    }
                } else if let Some(value) = arg.strip_prefix("--working-directory=") {
                    if value.is_empty() {
                        option_error.get_or_insert_with(|| {
                            "'--working-directory' needs a directory".to_owned()
                        });
                    } else {
                        working_dir = Some(PathBuf::from(value));
                    }
                } else if let Some(value) = arg.strip_prefix("--option=") {
                    if value.is_empty() {
                        option_error.get_or_insert_with(|| "'--option' needs KEY=VALUE".to_owned());
                    } else {
                        config_overrides.push(value.to_owned());
                    }
                } else {
                    unknown.get_or_insert(arg);
                }
            }
        }
    }
    if version {
        return Ok(Invocation::Version);
    }
    if list_themes {
        return Ok(Invocation::ListThemes {
            config_path,
            config_overrides,
        });
    }
    if let Some(arg) = unknown {
        return Err(format!("unknown argument '{arg}'"));
    }
    if let Some(error) = option_error {
        return Err(error);
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
        config_path,
        config_overrides,
        working_dir,
    })
}

fn usage() -> String {
    format!(
        "\
GPU-accelerated terminal emulator. Opens a window running your shell.

Usage: ultty [OPTIONS] [-e COMMAND [ARGS]...]

Options:
  -e COMMAND [ARGS]...      Run COMMAND in the first tab instead of your shell.
                            Everything after -e is passed to COMMAND.
                            Does not restore or save the session.
                            -- does the same.
  --config PATH             Load configuration from PATH instead of the default file.
  -o, --option KEY=VALUE    Override a config key (repeatable, e.g. -o font.size=16).
  --working-directory DIR   Start the initial shell or command in DIR.
  -h, --help                Print this help and exit
  -V, --version             Print the version and exit
  --list-themes             Print the available colour themes and exit
  --new-process             Always start a new process even with behavior.single_instance.
  --quick                   Toggle the running instance's quick terminal, or start one.
  --profile NAME            Launch configured tabs and panes instead of restoring.
  --workspace NAME          Open a saved layout in fresh shells; does not resume jobs.
  --attach NAME             Attach to a live session host; close the window to detach.

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
    use std::path::{Path, PathBuf};

    use super::{
        invocation, resolve_working_directory, single_instance_launch, usage, version_output,
        Invocation, StartupCommand,
    };

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn run_default() -> Invocation {
        Invocation::Run {
            command: None,
            new_process: false,
            quick: false,
            config_path: None,
            config_overrides: Vec::new(),
            working_dir: None,
        }
    }

    fn run_command(program: &str, command_args: &[&str]) -> Result<Invocation, String> {
        Ok(Invocation::Run {
            command: Some(StartupCommand::Exec {
                program: program.to_owned(),
                args: args(command_args),
            }),
            new_process: false,
            quick: false,
            config_path: None,
            config_overrides: Vec::new(),
            working_dir: None,
        })
    }

    #[test]
    fn no_arguments_run_the_shell() {
        assert_eq!(invocation(args(&[])), Ok(run_default()));
    }

    #[test]
    fn test_config_option_and_working_directory_flags_parse() {
        assert_eq!(
            invocation(args(&[
                "--config",
                "/tmp/alt.toml",
                "-o",
                "font.size=18",
                "--option=behavior.cursor_blink=false",
                "--option",
                "colors.theme=dracula",
                "--working-directory",
                "/tmp",
            ])),
            Ok(Invocation::Run {
                command: None,
                new_process: false,
                quick: false,
                config_path: Some(PathBuf::from("/tmp/alt.toml")),
                config_overrides: args(&[
                    "font.size=18",
                    "behavior.cursor_blink=false",
                    "colors.theme=dracula",
                ]),
                working_dir: Some(PathBuf::from("/tmp")),
            })
        );
        assert_eq!(
            invocation(args(&[
                "--config=/tmp/one.toml",
                "--working-directory=/var/tmp",
                "-e",
                "echo",
                "--config",
                "-o",
            ])),
            Ok(Invocation::Run {
                command: Some(StartupCommand::Exec {
                    program: "echo".to_owned(),
                    args: args(&["--config", "-o"]),
                }),
                new_process: false,
                quick: false,
                config_path: Some(PathBuf::from("/tmp/one.toml")),
                config_overrides: Vec::new(),
                working_dir: Some(PathBuf::from("/var/tmp")),
            })
        );
    }

    #[test]
    fn test_config_option_and_working_directory_missing_values_error() {
        for bad in [
            vec!["--config"],
            vec!["--config="],
            vec!["--working-directory"],
            vec!["--working-directory="],
            vec!["-o"],
            vec!["--option"],
            vec!["--option="],
        ] {
            assert!(
                invocation(args(&bad)).is_err(),
                "expected error for {bad:?}"
            );
        }
        assert_eq!(
            invocation(args(&["--config", "--help"])),
            Ok(Invocation::Help)
        );
        assert_eq!(
            invocation(args(&["--working-directory", "--version"])),
            Ok(Invocation::Version)
        );
    }

    #[test]
    fn test_resolve_working_directory_validates_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_working_directory(dir.path().to_path_buf()).unwrap();
        assert_eq!(resolved, dir.path());

        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, "hi").unwrap();
        assert!(resolve_working_directory(file).is_err());
        assert!(resolve_working_directory(dir.path().join("missing")).is_err());
    }

    #[test]
    fn test_new_process_flag_parses_and_combines_with_exec() {
        assert_eq!(
            invocation(args(&["--new-process"])),
            Ok(Invocation::Run {
                command: None,
                new_process: true,
                quick: false,
                config_path: None,
                config_overrides: Vec::new(),
                working_dir: None,
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
                config_path: None,
                config_overrides: Vec::new(),
                working_dir: None,
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
                config_path: None,
                config_overrides: Vec::new(),
                working_dir: None,
            })
        );
        assert_eq!(
            invocation(args(&["--quick", "--new-process"])),
            Ok(Invocation::Run {
                command: None,
                new_process: true,
                quick: true,
                config_path: None,
                config_overrides: Vec::new(),
                working_dir: None,
            })
        );
        assert!(invocation(args(&["--quick", "--profile", "dev"])).is_err());
        assert!(invocation(args(&["--quick", "--workspace", "work"])).is_err());
    }

    #[test]
    fn single_instance_launch_maps_shell_and_exec_but_skips_profile_and_workspace() {
        let shell = single_instance_launch(&None, None).expect("bare launch hands off");
        assert!(shell.command.is_none());
        assert!(shell.args.is_empty());
        assert!(shell.working_dir.is_some());

        let with_dir = single_instance_launch(&None, Some(Path::new("/tmp/proj")))
            .expect("bare launch with dir hands off");
        assert_eq!(with_dir.working_dir.as_deref(), Some("/tmp/proj"));

        let exec = single_instance_launch(
            &Some(StartupCommand::Exec {
                program: "htop".to_owned(),
                args: args(&["-d", "5"]),
            }),
            Some(Path::new("/tmp/work")),
        )
        .expect("-e launch hands off");
        assert_eq!(exec.command.as_deref(), Some("htop"));
        assert_eq!(exec.args, args(&["-d", "5"]));
        assert_eq!(exec.working_dir.as_deref(), Some("/tmp/work"));

        assert!(
            single_instance_launch(&Some(StartupCommand::Profile("work".to_owned())), None)
                .is_none()
        );
        assert!(
            single_instance_launch(&Some(StartupCommand::Workspace("dev".to_owned())), None)
                .is_none()
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
                config_path: None,
                config_overrides: Vec::new(),
                working_dir: None,
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
            Ok(Invocation::ListThemes {
                config_path: None,
                config_overrides: Vec::new(),
            })
        );
        assert_eq!(
            invocation(args(&["--config", "/tmp/c.toml", "--list-themes"])),
            Ok(Invocation::ListThemes {
                config_path: Some(PathBuf::from("/tmp/c.toml")),
                config_overrides: Vec::new(),
            })
        );
        assert_eq!(
            invocation(args(&["--list-themes", "--bogus"])),
            Ok(Invocation::ListThemes {
                config_path: None,
                config_overrides: Vec::new(),
            })
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
            "--config PATH",
            "-o, --option KEY=VALUE",
            "--working-directory DIR",
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
