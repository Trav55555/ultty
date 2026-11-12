use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use std::path::Path;

use ultty::control::format_response;
use ultty::control::paths::{control_dir, ControlEndpoint};
use ultty_protocol::control::{
    ControlData, ControlEnvelope, ControlEvent, ControlRequest, ControlResponse, OutputFormat,
    PaneState, PaneStatus, SplitDirection, WaitCondition,
};
use ultty_protocol::{Key, Modifiers, OverrideFormat, MAX_OVERRIDE_SOURCE_LEN};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        std::process::exit(0);
    }

    if should_print_version(&args) {
        println!("{}", version_output());
        std::process::exit(0);
    }

    if args.first().map(String::as_str) == Some("mcp") {
        std::process::exit(match run_mcp() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {}", e);
                1
            }
        });
    }

    let use_json = args.iter().any(|a| a == "--json");
    let timeout_secs: u64 = args
        .windows(2)
        .find_map(|w| {
            if w[0] == "--read-timeout" {
                w[1].parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(5);

    let args: Vec<String> = args.into_iter().filter(|a| a != "--json").collect();
    let args = remove_flag_with_value(args, "--read-timeout");

    let mut request = match parse_cli_request(&args) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    };
    default_split_target(&mut request, own_pane());
    let read_timeout = read_timeout_for(&request, timeout_secs);
    let endpoint = match find_instance() {
        Ok(endpoint) => endpoint,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    };
    let subscribe = matches!(request, ControlRequest::Subscribe { .. });
    let request = with_auth_token(request, read_auth_token(&endpoint.token));
    // Always ask for JSON so the reply can be parsed and the exit status set
    // from it; text output is rendered here with the server's formatter.
    let envelope = ControlEnvelope {
        output_format: OutputFormat::Json,
        request,
    };
    let command =
        serde_json::to_string(&envelope).expect("control request envelope should serialize");

    if subscribe {
        match stream_events(&endpoint.socket, &command, &mut std::io::stdout()) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }

    let response = match send_command(&endpoint.socket, &command, read_timeout)
        .and_then(|raw| parse_response(&raw))
    {
        Ok(response) => response,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    };
    let format = if use_json {
        OutputFormat::Json
    } else {
        OutputFormat::Text
    };
    print!("{}", format_response(&response, format));
    std::process::exit(exit_status(&response));
}

/// Socket read timeout for `request`: long enough to outlast a `wait-for`,
/// which the server answers only when its own timeout runs out.
fn read_timeout_for(request: &ControlRequest, read_timeout_secs: u64) -> u64 {
    const WAIT_FOR_MARGIN_SECS: u64 = 5;
    // The server gives the app 5 s to answer an override push.
    const OVERRIDE_REPLY_SECS: u64 = 5;
    match request {
        ControlRequest::WaitFor { timeout_ms, .. } => {
            read_timeout_secs.max(timeout_ms.div_ceil(1000) + WAIT_FOR_MARGIN_SECS)
        }
        ControlRequest::PushOverride { .. } | ControlRequest::ClearOverride { .. } => {
            read_timeout_secs.max(OVERRIDE_REPLY_SECS + WAIT_FOR_MARGIN_SECS)
        }
        _ => read_timeout_secs,
    }
}

/// Parse the server's JSON reply. An empty reply means the server closed the
/// connection without answering, which is an error, not a silent success.
fn parse_response(raw: &str) -> Result<ControlResponse, String> {
    if raw.trim().is_empty() {
        return Err("ultty closed the connection without replying".to_string());
    }
    serde_json::from_str(raw.trim()).map_err(|e| format!("unreadable reply from ultty: {}", e))
}

/// Whether `response` should make ulttyctl exit non-zero: an error, or a
/// `wait-for` that timed out.
/// ulttyctl's exit status for `response`: 1 for an error or a timed-out
/// wait, the command's own status for `wait-for --command-done`, else 0.
fn exit_status(response: &ControlResponse) -> i32 {
    if response_is_failure(response) {
        return 1;
    }
    match response {
        ControlResponse::Data(ControlData::WaitEvent {
            event:
                ControlEvent::CommandFinished {
                    exit_code: Some(code),
                    ..
                },
        })
        | ControlResponse::Data(ControlData::LastCommand {
            exit_code: Some(code),
            running: false,
            ..
        }) => {
            if (0..=255).contains(code) {
                *code
            } else {
                1
            }
        }
        _ => 0,
    }
}

fn response_is_failure(response: &ControlResponse) -> bool {
    matches!(
        response,
        ControlResponse::Error { .. } | ControlResponse::Data(ControlData::WaitTimeout)
    )
}

/// The Ultty this command runs inside (`ULTTY_PID`) while it is still
/// running, else the most recently started one.
fn find_instance() -> Result<ControlEndpoint, String> {
    let dir = control_dir().map_err(|e| format!("cannot use the control directory: {}", e))?;
    let own_pid = std::env::var("ULTTY_PID")
        .ok()
        .and_then(|pid| pid.parse().ok());
    ultty::control::paths::find_instance(&dir, own_pid).ok_or_else(|| {
        format!(
            "no running ultty has a control socket in {} (is it running?)",
            dir.display()
        )
    })
}

/// Read the control auth token that instance wrote at startup, if any.
///
/// `ULTTY_AUTH_TOKEN` overrides the file so scripts can pass a token in
/// explicitly.
fn read_auth_token(token_path: &Path) -> Option<String> {
    if let Ok(token) = std::env::var("ULTTY_AUTH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    std::fs::read_to_string(token_path)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// Wrap a request so it authenticates in the same round trip.
///
/// An explicit `auth` command is sent as-is, and without a token the
/// request goes out unwrapped (the server will reject it with a clear
/// error rather than ulttyctl guessing).
fn with_auth_token(request: ControlRequest, token: Option<String>) -> ControlRequest {
    match (request, token) {
        (request @ ControlRequest::Authenticate { .. }, _) => request,
        (request, Some(token)) => ControlRequest::WithToken {
            token,
            request: Box::new(request),
        },
        (request, None) => request,
    }
}

fn remove_flag_with_value(args: Vec<String>, flag: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == flag {
            skip_next = true;
            continue;
        }
        result.push(arg);
    }
    result
}

fn parse_cli_request(args: &[String]) -> Result<ControlRequest, String> {
    // `--pane <id>` comes before the command, so it can never be mistaken
    // for part of the text that send-text or search take.
    if args.first().map(String::as_str) == Some("--pane") {
        let id = args
            .get(1)
            .ok_or_else(|| "--pane requires a pane id (see `ulttyctl panes`)".to_string())?
            .parse()
            .map_err(|_| "--pane requires a numeric pane id".to_string())?;
        let mut request = parse_cli_request(&args[2..])?;
        return set_target(&mut request, id).map(|()| request);
    }

    let command = args
        .first()
        .ok_or_else(|| "missing command".to_string())?
        .as_str();
    let rest = &args[1..];

    match command {
        "ping" => Ok(ControlRequest::Ping),
        "version" => Ok(ControlRequest::Version),
        "info" => Ok(ControlRequest::GetInfo),
        "tabs" => Ok(ControlRequest::ListTabs),
        "panes" => Ok(ControlRequest::ListPanes),
        "text" => Ok(ControlRequest::GetText {
            include_scrollback: rest.iter().any(|arg| arg == "--scrollback"),
            scrollback_lines: None,
            target: None,
        }),
        "selection" => Ok(ControlRequest::GetSelection { target: None }),
        "send-text" => {
            let text = rest.join(" ");
            if text.is_empty() {
                Err("send-text requires text".to_string())
            } else {
                Ok(ControlRequest::SendText { text, target: None })
            }
        }
        "send-key" => {
            let key = parse_key(
                rest.first()
                    .ok_or_else(|| "send-key requires a key".to_string())?,
            )
            .ok_or_else(|| "unknown key".to_string())?;
            let modifiers = parse_modifiers(rest.get(1).map(String::as_str).unwrap_or(""));
            Ok(ControlRequest::SendKey {
                key,
                modifiers,
                target: None,
            })
        }
        "send-raw" => {
            let hex = rest.join("");
            let data = hex_decode(&hex).ok_or_else(|| "send-raw requires valid hex".to_string())?;
            Ok(ControlRequest::SendRaw { data, target: None })
        }
        "new-tab" => Ok(ControlRequest::NewTab {
            title: None,
            command: rest.first().cloned(),
            args: rest.iter().skip(1).cloned().collect(),
            working_dir: None,
        }),
        "split" => parse_split(rest),
        "lua" => match rest.first().map(String::as_str) {
            Some("open") => {
                let open = LuaOpen::parse(&rest[1..])?;
                let cwd = std::env::current_dir()
                    .map_err(|e| format!("cannot find the current directory: {e}"))?;
                open.check(&cwd)?;
                if open.split.is_some() {
                    eprintln!(
                        "note: a split pane draws with ANSI until native sessions map to panes"
                    );
                }
                Ok(open.request(&lua_runner(), &cwd))
            }
            _ => Err(LuaOpen::USAGE.to_string()),
        },
        "switch-tab" => {
            let index = parse_display_tab_index(
                rest.first()
                    .ok_or_else(|| "switch-tab requires an index".to_string())?,
            )
            .ok_or_else(|| "switch-tab index must be 1 or greater".to_string())?;
            Ok(ControlRequest::SwitchTab { index })
        }
        "close-tab" => Ok(ControlRequest::CloseTab {
            index: rest.first().and_then(|arg| parse_display_tab_index(arg)),
        }),
        "focus" => Ok(ControlRequest::FocusWindow),
        "fullscreen" => Ok(ControlRequest::ToggleFullscreen),
        "opacity" => {
            let opacity = rest
                .first()
                .ok_or_else(|| "opacity requires a value".to_string())?
                .parse()
                .map_err(|_| "opacity requires a number".to_string())?;
            Ok(ControlRequest::SetOpacity { opacity })
        }
        "run" => {
            let command = rest
                .first()
                .ok_or_else(|| "run requires a command".to_string())?
                .clone();
            Ok(ControlRequest::RunCommand {
                command,
                args: rest.iter().skip(1).cloned().collect(),
                target: None,
            })
        }
        "resize" => {
            let width = rest
                .first()
                .ok_or_else(|| "resize requires width and height".to_string())?
                .parse()
                .map_err(|_| "resize width must be a number".to_string())?;
            let height = rest
                .get(1)
                .ok_or_else(|| "resize requires width and height".to_string())?
                .parse()
                .map_err(|_| "resize height must be a number".to_string())?;
            Ok(ControlRequest::Resize { width, height })
        }
        "screenshot" => Ok(ControlRequest::Screenshot {
            path: rest.first().cloned(),
        }),
        "search" => {
            let pattern = rest.join(" ");
            if pattern.is_empty() {
                Err("search requires a pattern".to_string())
            } else {
                Ok(ControlRequest::Search {
                    pattern,
                    regex: false,
                    case_sensitive: false,
                    include_scrollback: true,
                    target: None,
                })
            }
        }
        "wait-for" => parse_wait_for(rest),
        "subscribe" => Ok(ControlRequest::Subscribe { target: None }),
        "last-command" => {
            let tail_lines = match rest {
                [] => None,
                [flag, lines] if flag == "--tail" => Some(
                    lines
                        .parse()
                        .map_err(|_| "--tail requires a number of lines".to_string())?,
                ),
                _ => return Err("usage: last-command [--tail <lines>]".to_string()),
            };
            Ok(ControlRequest::LastCommand {
                tail_lines,
                target: None,
            })
        }
        "regions" if rest.is_empty() => Ok(ControlRequest::Regions { target: None }),
        "regions" => Err("usage: regions".to_string()),
        "override" => parse_override(rest),
        "status" => parse_status(rest),
        "auth" => Ok(ControlRequest::Authenticate {
            token: rest
                .first()
                .ok_or_else(|| "auth requires a token".to_string())?
                .clone(),
        }),
        _ => Err(format!("unknown command: {}", command)),
    }
}

const OVERRIDE_USAGE: &str = "usage: override push <file> | override clear";

/// `override push FILE` (Luau when FILE ends in `.luau`, TOML otherwise) or
/// `override clear`: an override for the native app in the pane (ADR 0023
/// §9). The file is read here, so the app sees it even when its directory
/// is not the terminal's.
fn parse_override(args: &[String]) -> Result<ControlRequest, String> {
    match args {
        [push, file] if push == "push" => {
            let path = Path::new(file);
            let source = read_override_source(path)?;
            Ok(ControlRequest::PushOverride {
                format: OverrideFormat::for_path(path),
                source,
                target: None,
            })
        }
        [clear] if clear == "clear" => Ok(ControlRequest::ClearOverride { target: None }),
        _ => Err(OVERRIDE_USAGE.to_string()),
    }
}

/// The text of the override file at `path`, within the push limit.
fn read_override_source(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > MAX_OVERRIDE_SOURCE_LEN {
        return Err(format!(
            "{}: {} bytes; an override is at most {MAX_OVERRIDE_SOURCE_LEN}",
            path.display(),
            bytes.len()
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8 text", path.display()))
}

/// `split [right|down] [cmd args...]`: a pane next to the focused one.
fn parse_split(args: &[String]) -> Result<ControlRequest, String> {
    let (direction, rest) = match args.first().map(String::as_str) {
        Some("right") => (SplitDirection::Right, &args[1..]),
        Some("down") => (SplitDirection::Down, &args[1..]),
        Some(flag) if flag.starts_with('-') => {
            return Err("usage: split [right|down] [cmd] [args]".to_string())
        }
        _ => (SplitDirection::Right, args),
    };
    Ok(ControlRequest::SplitPane {
        direction,
        command: rest.first().cloned(),
        args: rest.iter().skip(1).cloned().collect(),
        working_dir: None,
        target: None,
    })
}

/// `lua open [--split right|down] [--exec CMD]... [--allow-exec] [--log FILE]
/// [--] FILE`: a Luau tool in a pane of its own (ADR 0024 §2).
#[derive(Debug, PartialEq, Default)]
struct LuaOpen {
    script: String,
    split: Option<SplitDirection>,
    /// Command-line prefixes `run{}` may start; `allow_exec` lets it start
    /// anything.
    exec: Vec<String>,
    allow_exec: bool,
    log: Option<String>,
}

impl LuaOpen {
    const USAGE: &'static str =
        "usage: lua open <file> [--split right|down] [--exec <cmd>]... [--allow-exec] [--log <file>]";

    fn parse(args: &[String]) -> Result<Self, String> {
        let mut open = LuaOpen::default();
        let mut script = None;
        let mut options_over = false;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |flag: &str| {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value\n{}", Self::USAGE))
            };
            match arg.as_str() {
                "--" if !options_over => options_over = true,
                "--split" if !options_over => {
                    open.split = Some(match value("--split")?.as_str() {
                        "right" => SplitDirection::Right,
                        "down" => SplitDirection::Down,
                        other => return Err(format!("--split takes right or down, not {other}")),
                    })
                }
                "--exec" if !options_over => open.exec.push(value("--exec")?),
                "--allow-exec" if !options_over => open.allow_exec = true,
                "--log" if !options_over => open.log = Some(value("--log")?),
                flag if flag.starts_with('-') && !options_over => {
                    return Err(format!("unknown option {flag}\n{}", Self::USAGE))
                }
                path if script.is_none() => script = Some(path.to_string()),
                extra => return Err(format!("unexpected argument {extra}\n{}", Self::USAGE)),
            }
        }
        open.script = script.ok_or_else(|| Self::USAGE.to_string())?;
        Ok(open)
    }

    /// What ulttui-lua would refuse at startup, checked here so the error
    /// reaches the person who typed the command instead of a pane that
    /// closes: the script must exist in `cwd`, and each grant must pass the
    /// same check ulttui-lua makes.
    fn check(&self, cwd: &Path) -> Result<(), String> {
        if !cwd.join(&self.script).is_file() {
            return Err(format!("{}: no such file", self.script));
        }
        for command in &self.exec {
            ultty_protocol::exec_grant::parse(command)?;
        }
        Ok(())
    }

    /// The request that opens the script with `runner` (the `ulttui-lua`
    /// binary) in `cwd`, the caller's directory, which also makes the script
    /// and log paths absolute. Nobody sees ulttui-lua's stderr in a pane that
    /// closes when it exits, so it is told to hold a startup error on screen
    /// until a key is pressed.
    fn request(&self, runner: &str, cwd: &Path) -> ControlRequest {
        let script = cwd.join(&self.script).display().to_string();
        let mut runner_args = vec!["--wait-on-error".to_string()];
        for command in &self.exec {
            runner_args.extend(["--exec".to_string(), command.clone()]);
        }
        if self.allow_exec {
            runner_args.push("--allow-exec".to_string());
        }
        if let Some(log) = &self.log {
            runner_args.extend(["--log".to_string(), cwd.join(log).display().to_string()]);
        }
        runner_args.push(script);
        let working_dir = Some(cwd.display().to_string());
        match self.split {
            None => ControlRequest::NewTab {
                title: Path::new(&self.script)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                command: Some(runner.to_string()),
                args: runner_args,
                working_dir,
            },
            // Native scenes are drawn only in a tab of one pane, so a split
            // pane draws ANSI.
            Some(direction) => ControlRequest::SplitPane {
                direction,
                command: Some(runner.to_string()),
                args: std::iter::once("--ansi".to_string())
                    .chain(runner_args)
                    .collect(),
                working_dir,
                target: None,
            },
        }
    }
}

/// The `ulttui-lua` binary: the one next to this executable (a build or
/// install directory), else whatever the Ultty process finds on its PATH.
fn lua_runner() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("ulttui-lua")))
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "ulttui-lua".to_string())
}

fn parse_wait_for(args: &[String]) -> Result<ControlRequest, String> {
    let mut timeout_ms = 30000;
    let mut until = WaitCondition::Text;
    let mut pattern_parts = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let event = match args[index].as_str() {
            "--command-done" => Some(WaitCondition::CommandFinished),
            "--bell" => Some(WaitCondition::Bell),
            "--quiet" => {
                let ms = args
                    .get(index + 1)
                    .ok_or_else(|| "wait-for --quiet requires milliseconds".to_string())?
                    .parse()
                    .map_err(|_| "wait-for --quiet must be a number".to_string())?;
                index += 1;
                Some(WaitCondition::Quiet { ms })
            }
            _ => None,
        };
        if let Some(event) = event {
            if !until.is_text() {
                return Err("wait-for takes one of --command-done, --bell, --quiet".to_string());
            }
            until = event;
            index += 1;
        } else if args[index] == "--timeout" {
            let timeout = args
                .get(index + 1)
                .ok_or_else(|| "wait-for --timeout requires milliseconds".to_string())?;
            timeout_ms = timeout
                .parse()
                .map_err(|_| "wait-for timeout must be a number".to_string())?;
            index += 2;
        } else {
            pattern_parts.push(args[index].clone());
            index += 1;
        }
    }

    let pattern = pattern_parts.join(" ");
    match (until.is_text(), pattern.is_empty()) {
        (true, true) => Err("wait-for requires a pattern or an event to wait for".to_string()),
        (false, false) => Err("wait-for takes a pattern or an event, not both".to_string()),
        _ => Ok(ControlRequest::WaitFor {
            pattern,
            regex: false,
            timeout_ms,
            until,
            target: None,
        }),
    }
}

/// `status clear`, or `status <state> [label...] [--progress <0-1|N%>]
/// [--attention]`.
fn parse_status(args: &[String]) -> Result<ControlRequest, String> {
    let state = match args.first().map(String::as_str) {
        Some("clear") if args.len() == 1 => {
            return Ok(ControlRequest::SetPaneStatus {
                status: None,
                target: None,
            })
        }
        Some("idle") => PaneState::Idle,
        Some("working") => PaneState::Working,
        Some("waiting") => PaneState::Waiting,
        Some("done") => PaneState::Done,
        Some("error") => PaneState::Error,
        _ => return Err("status takes idle, working, waiting, done, error or clear".to_string()),
    };
    let mut status = PaneStatus {
        state,
        label: String::new(),
        progress: None,
        attention: false,
    };
    let mut label = Vec::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--attention" => status.attention = true,
            "--progress" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--progress requires a value".to_string())?;
                status.progress = Some(parse_progress(value)?);
                index += 1;
            }
            word => label.push(word),
        }
        index += 1;
    }
    status.label = label.join(" ");
    Ok(ControlRequest::SetPaneStatus {
        status: Some(status),
        target: None,
    })
}

/// A fraction from 0 to 1, or a percentage such as `40%`.
fn parse_progress(value: &str) -> Result<f32, String> {
    let (number, scale) = match value.strip_suffix('%') {
        Some(percent) => (percent, 100.0),
        None => (value, 1.0),
    };
    number
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite())
        .map(|n| (n / scale).clamp(0.0, 1.0))
        .ok_or_else(|| format!("--progress must be 0-1 or a percentage, not {value}"))
}

/// Point a pane-scoped request at pane `id`; other requests refuse `--pane`.
fn set_target(request: &mut ControlRequest, id: u64) -> Result<(), String> {
    match request {
        ControlRequest::GetText { target, .. }
        | ControlRequest::GetSelection { target }
        | ControlRequest::SendText { target, .. }
        | ControlRequest::SendKey { target, .. }
        | ControlRequest::SendRaw { target, .. }
        | ControlRequest::RunCommand { target, .. }
        | ControlRequest::Search { target, .. }
        | ControlRequest::WaitFor { target, .. }
        | ControlRequest::Subscribe { target }
        | ControlRequest::SetPaneStatus { target, .. }
        | ControlRequest::LastCommand { target, .. }
        | ControlRequest::Regions { target }
        | ControlRequest::PushOverride { target, .. }
        | ControlRequest::ClearOverride { target }
        | ControlRequest::SplitPane { target, .. } => {
            *target = Some(id);
            Ok(())
        }
        _ => Err("--pane only applies to text, selection, send-text, send-key, send-raw, run, search, wait-for, subscribe, status, last-command, regions, override, split and lua open --split".to_string()),
    }
}

/// A `split` (or `lua open --split`) with no `--pane` splits the pane this
/// ulttyctl runs in, when Ultty told it which (`ULTTY_PANE`); otherwise the
/// server uses the focused pane.
fn default_split_target(request: &mut ControlRequest, own_pane: Option<u64>) {
    if let ControlRequest::SplitPane { target: None, .. } = request {
        if let Some(pane) = own_pane {
            let _ = set_target(request, pane);
        }
    }
}

fn own_pane() -> Option<u64> {
    std::env::var("ULTTY_PANE").ok()?.trim().parse().ok()
}

fn parse_key(s: &str) -> Option<Key> {
    match s.to_lowercase().as_str() {
        "enter" | "return" => Some(Key::Enter),
        "tab" => Some(Key::Tab),
        "escape" | "esc" => Some(Key::Escape),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "insert" | "ins" => Some(Key::Insert),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "space" => Some(Key::Char(' ')),
        s if s.starts_with('f') && s.len() <= 3 => {
            let n: u8 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                Some(Key::Function(n))
            } else {
                None
            }
        }
        s if s.len() == 1 => Some(Key::Char(s.chars().next()?)),
        _ => None,
    }
}

fn parse_display_tab_index(s: &str) -> Option<usize> {
    s.parse::<usize>().ok()?.checked_sub(1)
}

fn parse_modifiers(s: &str) -> Modifiers {
    let mut mods = Modifiers::NONE;
    let lower = s.to_lowercase();
    if lower.contains("shift") {
        mods = mods.union(Modifiers::SHIFT);
    }
    if lower.contains("ctrl") || lower.contains("control") {
        mods = mods.union(Modifiers::CTRL);
    }
    if lower.contains("alt") {
        mods = mods.union(Modifiers::ALT);
    }
    if lower.contains("meta") || lower.contains("super") || lower.contains("cmd") {
        mods = mods.union(Modifiers::META);
    }
    mods
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.len() & 1 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn should_print_version(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--version" || arg == "-V")
}

fn version_output() -> String {
    format!("ulttyctl {}", env!("CARGO_PKG_VERSION"))
}

fn send_command(socket_path: &Path, command: &str, timeout_secs: u64) -> Result<String, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| format!("failed to connect to ultty (is it running?): {}", e))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .map_err(|e| format!("failed to set timeout: {}", e))?;

    stream
        .write_all(command.as_bytes())
        .map_err(|e| format!("failed to send command: {}", e))?;

    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| format!("failed to shutdown write: {}", e))?;

    read_response(&mut stream, timeout_secs)
}

/// Serve MCP on stdin and stdout. Each tool call finds the running Ultty
/// afresh, so the server outlives a restarted terminal.
fn run_mcp() -> std::io::Result<()> {
    const MCP_READ_TIMEOUT_SECS: u64 = 10;
    let mut send = |request: ControlRequest| {
        let endpoint = find_instance()?;
        let read_timeout = read_timeout_for(&request, MCP_READ_TIMEOUT_SECS);
        let envelope = ControlEnvelope {
            output_format: OutputFormat::Json,
            request: with_auth_token(request, read_auth_token(&endpoint.token)),
        };
        let command = serde_json::to_string(&envelope)
            .map_err(|e| format!("failed to encode request: {}", e))?;
        send_command(&endpoint.socket, &command, read_timeout).and_then(|raw| parse_response(&raw))
    };
    // Screenshots go to the private control directory, not a shared /tmp.
    let screenshot_dir = control_dir().ok();
    let stdin = std::io::stdin();
    ultty::control::mcp::serve(
        stdin.lock(),
        std::io::stdout().lock(),
        &mut send,
        screenshot_dir.as_deref(),
    )
}

/// Subscribe and copy each event line to `out` until Ultty closes the
/// stream. The write side stays open: shutting it down ends a subscription.
fn stream_events(socket_path: &Path, command: &str, out: &mut impl Write) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| format!("failed to connect to ultty (is it running?): {}", e))?;
    stream
        .write_all(format!("{command}\n").as_bytes())
        .map_err(|e| format!("failed to send command: {}", e))?;
    let mut lines = BufReader::new(stream).lines();
    let first = lines
        .next()
        .ok_or_else(|| "ultty closed the connection without answering".to_string())?
        .map_err(|e| format!("failed to read reply: {}", e))?;
    match parse_response(&first)? {
        ControlResponse::Data(ControlData::Subscribed) => {}
        other => {
            return Err(format_response(&other, OutputFormat::Text)
                .trim()
                .to_string())
        }
    }
    for line in lines {
        let line = line.map_err(|e| format!("failed to read event: {}", e))?;
        writeln!(out, "{line}")
            .and_then(|()| out.flush())
            .map_err(|e| format!("failed to write event: {}", e))?;
    }
    Ok(())
}

fn read_response(stream: &mut UnixStream, timeout_secs: u64) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut response = Vec::new();
    let mut buf = [0_u8; 4096];

    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && !response.is_empty() =>
            {
                break;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("failed to read response: {}", e)),
        }
    }

    String::from_utf8(response).map_err(|e| format!("response was not valid UTF-8: {}", e))
}

fn print_help() {
    println!(
        r#"ulttyctl - control ultty from the command line

USAGE:
    ulttyctl [FLAGS] <command> [args...]

GLOBAL FLAGS:
    --json                  Output in JSON format
    --read-timeout <secs>   Socket read timeout in seconds (default: 5;
                            wait-for always waits out its own timeout)
    --pane <id>             Act on pane <id> (from `panes`) instead of the
                            focused one. Goes before the command; applies to
                            text, selection, send-*, run, search, wait-for,
                            subscribe, status, last-command, regions,
                            override, split and lua open --split (which
                            otherwise split the pane ulttyctl runs in, from
                            $ULTTY_PANE)
    -V, --version           Show ulttyctl version without connecting to Ultty
    -h, --help              Show this help

INPUT COMMANDS:
    send-text <text>        Send text as if typed
    send-key <key> [mods]   Send a key press (e.g., send-key enter, send-key a ctrl)
    send-raw <hex>          Send raw bytes (hex-encoded, e.g., send-raw 1b5b41)

TAB COMMANDS:
    new-tab [cmd] [args]    Create new tab with optional command (prints the
                            new pane's id)
    split [right|down] [cmd] [args]
                            Open a pane next to the focused one, running cmd
                            or a shell (prints the new pane's id)
    switch-tab <n>          Switch to tab number n (1-indexed)
    close-tab [n]           Close tab n (or current if omitted)
    tabs                    List all tabs
    panes                   List every pane with its id (* marks the focused one)

QUERY COMMANDS:
    info                    Get terminal info (size, cursor, etc.)
    text [--scrollback]     Get visible text (optionally with scrollback)
    selection               Get current selection
    search <pattern>        Search for pattern in scrollback
    last-command [--tail <lines>]
                            The last command the shell ran and its output
                            (needs OSC 133 shell integration); exits with
                            the command's status
    regions                 The native apps in the pane's tab: each region
                            by depth with its content kind, accessibility
                            role, position and size in cells, metadata and
                            text (all of it with --json)

WAIT COMMANDS:
    wait-for <pattern> [--timeout <ms>]
                            Wait for pattern to appear (default 30s timeout)
    wait-for --command-done | --bell | --quiet <ms> [--timeout <ms>]
                            Wait for the next command to finish (exits with
                            its status; needs shell integration), the next
                            bell, or <ms> without output
    subscribe               Print events as JSON lines until ultty exits:
                            pane_opened, pane_closed, command_started,
                            command_finished (needs shell integration),
                            bell, notification, title_changed, process_exited

WINDOW COMMANDS:
    focus                   Focus the ultty window
    fullscreen              Toggle fullscreen
    opacity <0.0-1.0>       Set window opacity
    resize <width> <height> Resize the window (pixels); fails, naming the size
                            it has, if it does not reach it within 2 s

EXECUTION:
    run <command> [args]    Run a command in current tab

LUA TOOLS (docs/lua.md):
    lua open <file> [--split right|down] [--exec <cmd>]... [--allow-exec]
             [--log <file>]
                            Run a Luau script with ulttui-lua in a new tab
                            (or a split, drawn with ANSI), in this directory,
                            and print its pane's id. run{{}} may start only
                            command lines beginning with an --exec <cmd>,
                            e.g. --exec 'ulttyctl --json panes'; a bare
                            program name allows any arguments. A grant on
                            a shell, env, sudo, an interpreter or bare
                            ulttyctl is refused before any pane opens: say
                            --allow-exec. If the tool still cannot start,
                            its pane shows why until a key is pressed

STATUS:
    status <state> [label] [--progress <0-1|N%>] [--attention]
                            Show what this pane is doing in the tab bar:
                            idle, working, waiting, done or error.
                            --attention marks an unfocused window urgent
    status clear            Remove the pane's status

MCP:
    mcp                     Serve the tools above to an AI agent over MCP
                            (stdio). For Claude Code:
                            claude mcp add ultty -- ulttyctl mcp

AUTHENTICATION:
    Inside an ultty window, ulttyctl talks to that window's ultty (found
    through ULTTY_PID, which ultty sets for its shells); elsewhere it talks
    to the most recently started ultty that is running. Each
    ultty listens on $XDG_RUNTIME_DIR/ultty-ctl-<pid>.sock (or, without
    XDG_RUNTIME_DIR, in a private ultty-<uid> directory under $TMPDIR or
    /tmp). Every command is authenticated automatically with the token that
    instance writes next to it, ultty-auth-<pid>.token (override with the
    ULTTY_AUTH_TOKEN environment variable).
    auth <token>            Check a token explicitly

OTHER:
    ping                    Test connection
    version                 Show version info
    screenshot [path]       Take screenshot (auto-timestamps if no path given)

KEYS:
    Named keys: enter, tab, escape, backspace, delete, insert, home, end,
                pageup, pagedown, up, down, left, right, space, f1-f12
    Single characters: a, b, 1, etc.
    Modifiers: ctrl, shift, alt, meta/super/cmd (combine: ctrl+shift)

EXAMPLES:
    ulttyctl send-text "ls -la"
    ulttyctl send-key enter
    ulttyctl send-key c ctrl
    ulttyctl send-raw 1b5b41          # send ESC [ A (up arrow)
    ulttyctl new-tab htop
    ulttyctl switch-tab 2
    ulttyctl text --scrollback
    ulttyctl search "error"
    ulttyctl wait-for "$ " --timeout 5000
    ulttyctl resize 1920 1080
    ulttyctl run nvim src/main.rs
    ulttyctl --json info

EXIT STATUS:
    0 on success, 1 when Ultty reports an error, a wait-for times out, or
    Ultty cannot be reached, 2 for a bad command line.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::{
        default_split_target, exit_status, parse_cli_request, parse_response, read_response,
        read_timeout_for, response_is_failure, should_print_version, version_output,
        with_auth_token, LuaOpen,
    };

    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use ultty_protocol::control::{
        ControlData, ControlEvent, ControlRequest, ControlResponse, PaneState, PaneStatus,
        SplitDirection, WaitCondition,
    };
    use ultty_protocol::OverrideFormat;

    #[test]
    fn read_response_retries_until_delayed_writer_closes() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();

        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            writer.write_all(b"OK\n").unwrap();
        });

        let response = read_response(&mut reader, 1).unwrap();
        handle.join().unwrap();

        assert_eq!(response, "OK\n");
    }

    #[test]
    fn read_response_returns_without_waiting_for_writer_to_close() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();

        let handle = thread::spawn(move || {
            writer.write_all(b"PONG\n").unwrap();
            thread::sleep(Duration::from_millis(200));
        });

        let response = read_response(&mut reader, 1).unwrap();
        handle.join().unwrap();

        assert_eq!(response, "PONG\n");
    }

    #[test]
    fn version_flag_is_detected() {
        assert!(should_print_version(&["--version".to_string()]));
        assert!(should_print_version(&["-V".to_string()]));
        assert!(should_print_version(&[
            "--read-timeout".to_string(),
            "1".to_string(),
            "--version".to_string()
        ]));
    }

    #[test]
    fn unrelated_args_do_not_request_version() {
        assert!(!should_print_version(&["version".to_string()]));
        assert!(!should_print_version(&["ping".to_string()]));
    }

    #[test]
    fn version_output_includes_binary_name_and_package_version() {
        assert_eq!(
            version_output(),
            format!("ulttyctl {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parse_cli_request_preserves_run_argument_boundaries() {
        let request = parse_cli_request(&[
            "run".to_string(),
            "/usr/bin/env".to_string(),
            "bash".to_string(),
            "-lc".to_string(),
            "printf 'hello world\\n'".to_string(),
        ])
        .unwrap();

        assert_eq!(
            request,
            ControlRequest::RunCommand {
                command: "/usr/bin/env".to_string(),
                args: vec![
                    "bash".to_string(),
                    "-lc".to_string(),
                    "printf 'hello world\\n'".to_string(),
                ],
                target: None,
            }
        );
    }

    #[test]
    fn requests_are_wrapped_with_the_auth_token() {
        let wrapped = with_auth_token(ControlRequest::GetInfo, Some("secret".to_string()));

        assert_eq!(
            wrapped,
            ControlRequest::WithToken {
                token: "secret".to_string(),
                request: Box::new(ControlRequest::GetInfo),
            }
        );
    }

    #[test]
    fn explicit_auth_command_is_not_wrapped() {
        let request = ControlRequest::Authenticate {
            token: "given".to_string(),
        };

        assert_eq!(
            with_auth_token(request.clone(), Some("secret".to_string())),
            request
        );
    }

    #[test]
    fn requests_without_a_token_are_sent_unwrapped() {
        assert_eq!(
            with_auth_token(ControlRequest::Ping, None),
            ControlRequest::Ping
        );
    }

    #[test]
    fn override_push_reads_the_file_and_takes_its_format_from_the_name() {
        let dir = std::env::temp_dir().join(format!("ulttyctl-override-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let toml = dir.join("try.toml");
        let luau = dir.join("try.luau");
        std::fs::write(&toml, "[widget.a]\nhidden = true\n").unwrap();
        std::fs::write(&luau, "return {}").unwrap();

        let args = |file: &Path| {
            vec![
                "--pane".to_string(),
                "3".to_string(),
                "override".to_string(),
                "push".to_string(),
                file.display().to_string(),
            ]
        };
        assert_eq!(
            parse_cli_request(&args(&toml)).unwrap(),
            ControlRequest::PushOverride {
                format: OverrideFormat::Toml,
                source: "[widget.a]\nhidden = true\n".to_string(),
                target: Some(3),
            }
        );
        assert_eq!(
            parse_cli_request(&args(&luau)).unwrap(),
            ControlRequest::PushOverride {
                format: OverrideFormat::Luau,
                source: "return {}".to_string(),
                target: Some(3),
            }
        );
        assert_eq!(
            parse_cli_request(&["override".to_string(), "clear".to_string()]).unwrap(),
            ControlRequest::ClearOverride { target: None }
        );

        let missing = parse_cli_request(&args(&dir.join("absent.toml"))).unwrap_err();
        assert!(missing.contains("absent.toml"), "{missing}");
        let usage = parse_cli_request(&["override".to_string()]).unwrap_err();
        assert!(usage.starts_with("usage: override"), "{usage}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The source limit is the one a user meets: a file of exactly the
    /// limit is sent, one byte more is refused here with its size.
    #[test]
    fn override_push_refuses_a_file_over_the_source_limit_and_sends_one_at_it() {
        use ultty_protocol::MAX_OVERRIDE_SOURCE_LEN;
        let dir =
            std::env::temp_dir().join(format!("ulttyctl-override-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let at = dir.join("at.toml");
        let over = dir.join("over.toml");
        std::fs::write(&at, "#".repeat(MAX_OVERRIDE_SOURCE_LEN)).unwrap();
        std::fs::write(&over, "#".repeat(MAX_OVERRIDE_SOURCE_LEN + 1)).unwrap();
        let args = |file: &Path| {
            vec![
                "override".to_string(),
                "push".to_string(),
                file.display().to_string(),
            ]
        };

        match parse_cli_request(&args(&at)).unwrap() {
            ControlRequest::PushOverride { source, .. } => {
                assert_eq!(source.len(), MAX_OVERRIDE_SOURCE_LEN)
            }
            other => panic!("{other:?}"),
        }
        let refused = parse_cli_request(&args(&over)).unwrap_err();
        assert!(
            refused.contains("over.toml")
                && refused.contains(&(MAX_OVERRIDE_SOURCE_LEN + 1).to_string()),
            "{refused}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_requests_wait_long_enough_for_the_app_to_answer() {
        let clear = ControlRequest::ClearOverride { target: None };
        assert_eq!(read_timeout_for(&clear, 5), 10);
        assert_eq!(read_timeout_for(&clear, 30), 30);
    }

    #[test]
    fn parse_cli_request_parses_json_query_command() {
        assert_eq!(
            parse_cli_request(&["info".to_string()]).unwrap(),
            ControlRequest::GetInfo
        );
        assert_eq!(
            parse_cli_request(&["panes".to_string()]).unwrap(),
            ControlRequest::ListPanes
        );
    }

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn leading_pane_flag_targets_pane_scoped_commands() {
        assert_eq!(
            parse_cli_request(&args("--pane 3 send-text hi there")).unwrap(),
            ControlRequest::SendText {
                text: "hi there".to_string(),
                target: Some(3),
            }
        );
        assert_eq!(
            parse_cli_request(&args("--pane 3 selection")).unwrap(),
            ControlRequest::GetSelection { target: Some(3) }
        );
    }

    #[test]
    fn wait_for_takes_a_pattern_or_one_event() {
        let until = |line: &str| match parse_cli_request(&args(line)).unwrap() {
            ControlRequest::WaitFor { until, .. } => until,
            other => panic!("{other:?}"),
        };
        assert_eq!(until("wait-for ready"), WaitCondition::Text);
        assert_eq!(
            until("wait-for --command-done"),
            WaitCondition::CommandFinished
        );
        assert_eq!(until("wait-for --bell --timeout 5"), WaitCondition::Bell);
        assert_eq!(
            until("wait-for --quiet 300"),
            WaitCondition::Quiet { ms: 300 }
        );
        for bad in [
            "wait-for",
            "wait-for ready --bell",
            "wait-for --bell --command-done",
            "wait-for --quiet",
        ] {
            assert!(parse_cli_request(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn command_done_exits_with_the_commands_status() {
        let finished = |exit_code| {
            ControlResponse::Data(ControlData::WaitEvent {
                event: ControlEvent::CommandFinished { pane: 0, exit_code },
            })
        };
        assert_eq!(exit_status(&finished(Some(0))), 0);
        assert_eq!(exit_status(&finished(Some(3))), 3);
        assert_eq!(exit_status(&finished(Some(-1))), 1);
        assert_eq!(exit_status(&finished(None)), 0);
        assert_eq!(
            exit_status(&ControlResponse::Data(ControlData::WaitTimeout)),
            1
        );
    }

    #[test]
    fn status_takes_a_state_label_progress_and_attention() {
        assert_eq!(
            parse_cli_request(&args(
                "--pane 2 status waiting need a review --attention --progress 40%"
            ))
            .unwrap(),
            ControlRequest::SetPaneStatus {
                status: Some(PaneStatus {
                    state: PaneState::Waiting,
                    label: "need a review".to_string(),
                    progress: Some(0.4),
                    attention: true,
                }),
                target: Some(2),
            }
        );
        assert_eq!(
            parse_cli_request(&args("status clear")).unwrap(),
            ControlRequest::SetPaneStatus {
                status: None,
                target: None
            }
        );
        for bad in [
            "status",
            "status busy",
            "status working --progress",
            "status working --progress x",
        ] {
            assert!(parse_cli_request(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn regions_takes_a_pane_and_no_arguments() {
        assert_eq!(
            parse_cli_request(&args("--pane 3 regions")).unwrap(),
            ControlRequest::Regions { target: Some(3) }
        );
        assert!(parse_cli_request(&args("regions extra")).is_err());
    }

    #[test]
    fn last_command_takes_a_tail_and_exits_with_the_commands_status() {
        assert_eq!(
            parse_cli_request(&args("--pane 1 last-command --tail 20")).unwrap(),
            ControlRequest::LastCommand {
                tail_lines: Some(20),
                target: Some(1),
            }
        );
        assert!(parse_cli_request(&args("last-command --tail")).is_err());
        assert!(parse_cli_request(&args("last-command extra")).is_err());

        let last = |exit_code, running| {
            ControlResponse::Data(ControlData::LastCommand {
                command: "make".to_string(),
                output: String::new(),
                exit_code,
                running,
                output_available: true,
            })
        };
        assert_eq!(exit_status(&last(Some(2), false)), 2);
        assert_eq!(exit_status(&last(None, true)), 0);
    }

    #[test]
    fn pane_flag_after_the_command_is_just_text() {
        assert_eq!(
            parse_cli_request(&args("send-text --pane 3")).unwrap(),
            ControlRequest::SendText {
                text: "--pane 3".to_string(),
                target: None,
            }
        );
    }

    #[test]
    fn pane_flag_is_refused_where_it_cannot_apply() {
        assert!(parse_cli_request(&args("--pane 3 tabs")).is_err());
        assert!(parse_cli_request(&args("--pane x text")).is_err());
        assert!(parse_cli_request(&args("--pane")).is_err());
    }

    #[test]
    fn split_defaults_to_the_right_and_takes_a_command() {
        assert_eq!(
            parse_cli_request(&args("split")).unwrap(),
            ControlRequest::SplitPane {
                direction: SplitDirection::Right,
                command: None,
                args: Vec::new(),
                working_dir: None,
                target: None,
            }
        );
        assert_eq!(
            parse_cli_request(&args("--pane 3 split down htop -d 10")).unwrap(),
            ControlRequest::SplitPane {
                direction: SplitDirection::Down,
                command: Some("htop".to_string()),
                args: vec!["-d".to_string(), "10".to_string()],
                working_dir: None,
                target: Some(3),
            }
        );
        // A program called `right` would be ambiguous; a flag is an error.
        assert!(parse_cli_request(&args("split --down")).is_err());
    }

    #[test]
    fn split_targets_the_callers_own_pane_unless_told_otherwise() {
        let mut request = parse_cli_request(&args("split")).unwrap();
        default_split_target(&mut request, Some(7));
        assert!(matches!(
            request,
            ControlRequest::SplitPane {
                target: Some(7),
                ..
            }
        ));

        let mut request = parse_cli_request(&args("--pane 3 split")).unwrap();
        default_split_target(&mut request, Some(7));
        assert!(matches!(
            request,
            ControlRequest::SplitPane {
                target: Some(3),
                ..
            }
        ));

        let mut request = parse_cli_request(&args("split")).unwrap();
        default_split_target(&mut request, None);
        assert!(matches!(
            request,
            ControlRequest::SplitPane { target: None, .. }
        ));

        // Only splits: a new tab has no pane to be next to.
        let mut request = parse_cli_request(&args("new-tab")).unwrap();
        default_split_target(&mut request, Some(7));
        assert_eq!(request, parse_cli_request(&args("new-tab")).unwrap());
    }

    #[test]
    fn lua_open_parses_its_grants_and_placement() {
        assert_eq!(
            LuaOpen::parse(&args("tool.luau")).unwrap(),
            LuaOpen {
                script: "tool.luau".to_string(),
                ..LuaOpen::default()
            }
        );
        assert_eq!(
            LuaOpen::parse(&args(
                "--split down --exec git --log l.log tool.luau --exec ulttyctl --allow-exec"
            ))
            .unwrap(),
            LuaOpen {
                script: "tool.luau".to_string(),
                split: Some(SplitDirection::Down),
                exec: vec!["git".to_string(), "ulttyctl".to_string()],
                allow_exec: true,
                log: Some("l.log".to_string()),
            }
        );
        // A grant with spaces is one argument, kept whole for ulttui-lua.
        let open = LuaOpen::parse(&[
            "--exec".to_string(),
            "ulttyctl --json panes".to_string(),
            "tool.luau".to_string(),
        ])
        .unwrap();
        assert_eq!(open.exec, ["ulttyctl --json panes"]);
        // `--` ends the options, so a file called `--help` can be opened.
        assert_eq!(
            LuaOpen::parse(&args("--exec git -- --help")).unwrap(),
            LuaOpen {
                script: "--help".to_string(),
                exec: vec!["git".to_string()],
                ..LuaOpen::default()
            }
        );
        for bad in [
            "",
            "--exec",
            "--exec git",
            "--split left tool.luau",
            "--split",
            "--trust tool.luau",
            "a.luau b.luau",
            "-- a.luau b.luau",
        ] {
            assert!(LuaOpen::parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(parse_cli_request(&args("lua")).is_err());
        assert!(parse_cli_request(&args("lua close x.luau")).is_err());
        assert!(parse_cli_request(&args("lua open /nonexistent/x.luau")).is_err());
    }

    #[test]
    fn lua_open_refuses_before_opening_what_ulttui_lua_would_refuse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tool.luau"), "return {}").unwrap();
        let check = |line: &str| LuaOpen::parse(&args(line)).unwrap().check(dir.path());

        assert_eq!(check("tool.luau"), Ok(()));
        assert_eq!(check("--exec git --allow-exec tool.luau"), Ok(()));
        let one_command = LuaOpen::parse(&[
            "--exec".to_string(),
            "ulttyctl --json panes".to_string(),
            "tool.luau".to_string(),
        ])
        .unwrap();
        assert_eq!(one_command.check(dir.path()), Ok(()));

        assert_eq!(
            check("missing.luau"),
            Err("missing.luau: no such file".into())
        );
        // A grant on a program runner is refused with ulttui-lua's own words,
        // which name the flag to use instead.
        for refused in ["sh", "/usr/bin/env", "python3", "ulttyctl"] {
            let error = check(&format!("--exec git --exec {refused} tool.luau")).unwrap_err();
            assert_eq!(
                error,
                ultty_protocol::exec_grant::parse(refused).unwrap_err()
            );
            assert!(error.contains("--allow-exec"), "{error}");
        }
        // --allow-exec does not excuse a refused grant: ulttui-lua still
        // refuses it, so the pane would only show an error.
        assert!(check("--exec sh --allow-exec tool.luau").is_err());
    }

    #[test]
    fn lua_open_runs_the_script_in_the_callers_directory_with_its_grants() {
        let open = LuaOpen::parse(&args("--exec ulttyctl --log l.log tools/panes.luau")).unwrap();
        assert_eq!(
            open.request("/opt/ultty/ulttui-lua", std::path::Path::new("/home/me")),
            ControlRequest::NewTab {
                title: Some("panes.luau".to_string()),
                command: Some("/opt/ultty/ulttui-lua".to_string()),
                args: args(
                    "--wait-on-error --exec ulttyctl --log /home/me/l.log /home/me/tools/panes.luau"
                ),
                working_dir: Some("/home/me".to_string()),
            }
        );
        let open = LuaOpen::parse(&args("--allow-exec --log /var/l.log /abs/tool.luau")).unwrap();
        assert_eq!(
            open.request("ulttui-lua", std::path::Path::new("/home/me")),
            ControlRequest::NewTab {
                title: Some("tool.luau".to_string()),
                command: Some("ulttui-lua".to_string()),
                args: args("--wait-on-error --allow-exec --log /var/l.log /abs/tool.luau"),
                working_dir: Some("/home/me".to_string()),
            }
        );
    }

    #[test]
    fn lua_open_in_a_split_draws_ansi_and_takes_a_pane() {
        let open = LuaOpen::parse(&args("--split right --exec git tool.luau")).unwrap();
        assert_eq!(
            open.request("/a=b/ulttui-lua", std::path::Path::new("/w")),
            ControlRequest::SplitPane {
                direction: SplitDirection::Right,
                command: Some("/a=b/ulttui-lua".to_string()),
                args: args("--ansi --wait-on-error --exec git /w/tool.luau"),
                working_dir: Some("/w".to_string()),
                target: None,
            }
        );
        // `--pane` names the pane to split next to; the file must exist for
        // the full command line, so use this one.
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/lua/hello.luau")
            .display()
            .to_string();
        match parse_cli_request(&args(&format!("--pane 5 lua open --split down {here}"))).unwrap() {
            ControlRequest::SplitPane {
                direction: SplitDirection::Down,
                target: Some(5),
                ..
            } => {}
            other => panic!("{other:?}"),
        }
        assert!(parse_cli_request(&args(&format!("--pane 5 lua open {here}"))).is_err());
    }

    #[test]
    fn empty_reply_is_an_error() {
        assert!(parse_response("").is_err());
        assert!(parse_response("\n").is_err());
    }

    #[test]
    fn reply_parses_as_control_response() {
        let raw = serde_json::to_string(&ControlResponse::Data(ControlData::Pong)).unwrap();
        assert_eq!(
            parse_response(&format!("{raw}\n")),
            Ok(ControlResponse::Data(ControlData::Pong))
        );
    }

    #[test]
    fn errors_and_wait_timeouts_fail() {
        assert!(response_is_failure(&ControlResponse::Error {
            code: 403,
            message: "Authentication required".to_string(),
        }));
        assert!(response_is_failure(&ControlResponse::Data(
            ControlData::WaitTimeout
        )));
        assert!(!response_is_failure(&ControlResponse::Data(
            ControlData::WaitMatched { line: 0, col: 0 }
        )));
        assert!(!response_is_failure(&ControlResponse::Text {
            text: "ERROR on screen".to_string(),
        }));
    }

    #[test]
    fn wait_for_read_timeout_outlasts_the_wait() {
        let wait = ControlRequest::WaitFor {
            pattern: "DONE".to_string(),
            regex: false,
            timeout_ms: 10_000,
            until: Default::default(),
            target: None,
        };
        assert_eq!(read_timeout_for(&wait, 5), 15);
        assert_eq!(read_timeout_for(&wait, 60), 60);
        assert_eq!(read_timeout_for(&ControlRequest::Ping, 5), 5);
    }
}
