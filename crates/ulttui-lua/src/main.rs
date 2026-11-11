//! `ulttui-lua [--exec CMD]... [--allow-exec] [--ansi] [--log FILE]
//! [--wait-on-error] SCRIPT.luau`

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use ulttui::app::App;
use ulttui::backend::Backend;
use ulttui_lua::runner::watch_file;
use ulttui_lua::{ExecGrant, LuaApp, ScriptOptions};

const USAGE: &str = "usage: ulttui-lua [--exec CMD]... [--allow-exec] [--ansi] [--log FILE]
                  [--wait-on-error] SCRIPT.luau

Runs a Luau ulttui app and reloads it whenever the file is saved.

    --exec CMD     let run{} start command lines beginning with the words of
                   CMD, such as 'ulttyctl --json panes' (repeatable); a bare
                   program name allows any arguments
    --allow-exec   let the script run anything
    --ansi         draw with ANSI even inside Ultty (a split pane)
    --log FILE     write logs (including reload timings) to FILE
    --wait-on-error
                   if it cannot start, show why and wait for a key before
                   exiting (ulttyctl lua open passes this, as its pane
                   closes when ulttui-lua exits)";

#[derive(Debug, PartialEq)]
struct Args {
    script: PathBuf,
    exec: ExecGrant,
    ansi: bool,
    log: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut script = None;
    let mut prefixes = Vec::new();
    let mut allow_any = false;
    let mut ansi = false;
    let mut log = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exec" => prefixes.push(ExecGrant::parse_grant(
                &args.next().ok_or("--exec needs a command")?,
            )?),
            "--allow-exec" => allow_any = true,
            "--ansi" => ansi = true,
            // Read by `main` before parsing.
            "--wait-on-error" => {}
            "--log" => log = Some(PathBuf::from(args.next().ok_or("--log needs a file")?)),
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            path if script.is_none() => script = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument {extra}")),
        }
    }
    let exec = if allow_any {
        ExecGrant::Any
    } else if prefixes.is_empty() {
        ExecGrant::Deny
    } else {
        ExecGrant::Only(prefixes)
    };
    Ok(Args {
        script: script.ok_or("no script given")?,
        exec,
        ansi,
        log,
    })
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Looked for before parsing, so a bad command line is held on screen too.
    let wait_on_error = argv.iter().any(|arg| arg == "--wait-on-error");
    let (message, code) = match parse_args(argv.into_iter()) {
        Ok(args) => match run(args) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(error) => (format!("ulttui-lua: error: {error:#}"), 1),
        },
        Err(message) if message.is_empty() => (USAGE.to_string(), 2),
        Err(message) => (format!("ulttui-lua: error: {message}\n{USAGE}"), 2),
    };
    eprintln!("{message}");
    if wait_on_error {
        wait_for_key();
    }
    ExitCode::from(code)
}

/// Keep the error above on screen until a key is pressed. A pane opened by
/// `ulttyctl lua open` closes when ulttui-lua exits, so without this its
/// error would vanish unread. Returns at once when stdin is not a terminal.
fn wait_for_key() {
    use crossterm::event::{self, Event, KeyEventKind};
    use crossterm::terminal;

    if terminal::enable_raw_mode().is_err() {
        return;
    }
    eprint!("\r\npress any key to close");
    loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    // Best effort: the process exits next either way.
    let _ = terminal::disable_raw_mode();
}

fn run(args: Args) -> anyhow::Result<()> {
    // The terminal is the UI, so logs only ever go to a file.
    if let Some(log) = &args.log {
        let file = std::fs::File::create(log)
            .with_context(|| format!("cannot create log file {}", log.display()))?;
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "ulttui_lua=info,ulttui::overrides=info".into()),
            )
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    }
    tracing::info!(script = %args.script.display(), exec = ?args.exec, ansi = args.ansi, "starting");
    let app = LuaApp::new(
        &args.script,
        ScriptOptions {
            exec: args.exec,
            ..ScriptOptions::default()
        },
    )?;
    let runner = if args.ansi {
        App::new_with_backend(app, Backend::ansi())
    } else {
        App::new(app)
    };
    // Each script has its own overrides file, named after it.
    let name = args
        .script
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());
    let runner = runner.app_name(name.as_deref().unwrap_or_default());
    let proxy = runner.proxy();
    // Each change goes straight to the event loop, waking it.
    let _watch = watch_file(&args.script, move |msg| {
        // Fails only once the app has quit.
        let _ = proxy.send(msg);
    })?;
    runner.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, String> {
        parse_args(line.split_whitespace().map(str::to_string))
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn exec_is_denied_unless_granted() {
        assert_eq!(parse("app.luau").unwrap().exec, ExecGrant::Deny);
        // A grant with spaces arrives as one argument from the shell.
        let args = parse_args(
            ["--exec", "git status", "--exec", "ls", "app.luau"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert_eq!(
            args,
            Args {
                script: "app.luau".into(),
                exec: ExecGrant::Only(vec![words("git status"), words("ls")]),
                ansi: false,
                log: None,
            }
        );
        let args = parse("--allow-exec --ansi --log out.log app.luau").unwrap();
        assert_eq!(
            args,
            Args {
                script: "app.luau".into(),
                exec: ExecGrant::Any,
                ansi: true,
                log: Some("out.log".into()),
            }
        );
        // The broad flag wins over named commands.
        assert_eq!(
            parse("--exec git --allow-exec app.luau").unwrap().exec,
            ExecGrant::Any
        );
    }

    #[test]
    fn bad_command_lines_are_rejected() {
        assert!(parse("").is_err());
        assert!(parse("--exec app.luau").is_err());
        // A grant that would allow anything must be spelled --allow-exec.
        let refused = parse("--exec sh app.luau").unwrap_err();
        assert!(refused.contains("--allow-exec"), "{refused}");
        assert!(parse("--exec ulttyctl app.luau").is_err());
        assert!(parse("--allow-exec app.luau").is_ok());
        assert!(parse("--allow app.luau").is_err());
        assert!(parse("a.luau b.luau").is_err());
        assert!(parse("app.luau --log").is_err());
        assert!(parse_args(
            ["--exec", "'git", "app.luau"]
                .into_iter()
                .map(str::to_string)
        )
        .is_err());
    }

    #[test]
    fn grants_are_refused_by_the_check_ulttyctl_also_makes() {
        // `ulttyctl lua open` refuses a grant before opening a pane with
        // `ultty_protocol::exec_grant::parse`; ulttui-lua must refuse
        // exactly the same ones, with the same words.
        use ultty_protocol::exec_grant::{parse as shared, NEED_SUBCOMMAND, PROGRAM_RUNNERS};
        for &program in PROGRAM_RUNNERS.iter().chain(NEED_SUBCOMMAND) {
            let ours = parse_args(
                ["--exec", program, "app.luau"]
                    .into_iter()
                    .map(str::to_string),
            )
            .unwrap_err();
            assert_eq!(ours, shared(program).unwrap_err(), "{program}");
        }
        assert!(shared("ulttyctl --json panes").is_ok());
        assert!(parse_args(
            ["--exec", "ulttyctl --json panes", "app.luau"]
                .into_iter()
                .map(str::to_string)
        )
        .is_ok());
    }

    #[test]
    fn wait_on_error_is_accepted_anywhere_in_the_command_line() {
        // `main` acts on it; parsing only has to let it through.
        assert_eq!(
            parse("--wait-on-error app.luau").unwrap(),
            parse("app.luau").unwrap()
        );
        assert_eq!(
            parse("app.luau --wait-on-error").unwrap(),
            parse("app.luau").unwrap()
        );
    }
}
