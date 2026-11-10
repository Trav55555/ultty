//! `ulttui-lua [--allow-exec] [--log FILE] SCRIPT.luau`

use std::path::PathBuf;

use anyhow::Context;
use ulttui::app::App;
use ulttui_lua::runner::watch_file;
use ulttui_lua::{LuaApp, ScriptOptions};

const USAGE: &str = "usage: ulttui-lua [--allow-exec] [--log FILE] SCRIPT.luau

Runs a Luau ulttui app and reloads it whenever the file is saved.

    --allow-exec   let the script run processes with run{}
    --log FILE     write logs (including reload timings) to FILE";

#[derive(Debug, PartialEq)]
struct Args {
    script: PathBuf,
    allow_exec: bool,
    log: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut script = None;
    let mut allow_exec = false;
    let mut log = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--allow-exec" => allow_exec = true,
            "--log" => log = Some(PathBuf::from(args.next().ok_or("--log needs a file")?)),
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            path if script.is_none() => script = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument {extra}")),
        }
    }
    Ok(Args {
        script: script.ok_or("no script given")?,
        allow_exec,
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
    let app = LuaApp::new(
        &args.script,
        ScriptOptions {
            allow_exec: args.allow_exec,
            ..ScriptOptions::default()
        },
    )?;
    let runner = App::new(app);
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

    #[test]
    fn exec_is_off_unless_asked_for() {
        assert!(!parse("app.luau").unwrap().allow_exec);
        let args = parse("--allow-exec --log out.log app.luau").unwrap();
        assert_eq!(
            args,
            Args {
                script: "app.luau".into(),
                allow_exec: true,
                log: Some("out.log".into()),
            }
        );
    }

    #[test]
    fn bad_command_lines_are_rejected() {
        assert!(parse("").is_err());
        assert!(parse("--exec app.luau").is_err());
        assert!(parse("a.luau b.luau").is_err());
        assert!(parse("app.luau --log").is_err());
    }
}
