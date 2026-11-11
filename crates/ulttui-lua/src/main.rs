//! `ulttui-lua [--exec CMD]... [--allow-exec] [--ansi] [--log FILE] SCRIPT.luau`

use std::path::PathBuf;

use anyhow::Context;
use ulttui::app::App;
use ulttui::backend::Backend;
use ulttui_lua::runner::watch_file;
use ulttui_lua::{ExecGrant, LuaApp, ScriptOptions};

const USAGE: &str =
    "usage: ulttui-lua [--exec CMD]... [--allow-exec] [--ansi] [--log FILE] SCRIPT.luau

Runs a Luau ulttui app and reloads it whenever the file is saved.

    --exec CMD     let run{} start command lines beginning with the words of
                   CMD, such as 'ulttyctl --json panes' (repeatable); a bare
                   program name allows any arguments
    --allow-exec   let the script run anything
    --ansi         draw with ANSI even inside Ultty (a split pane)
    --log FILE     write logs (including reload timings) to FILE";

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

fn main() -> anyhow::Result<()> {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("error: {message}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    // The terminal is the UI, so logs only ever go to a file.
    if let Some(log) = &args.log {
        let file = std::fs::File::create(log)
            .with_context(|| format!("cannot create log file {}", log.display()))?;
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "ulttui_lua=info".into()),
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
}
