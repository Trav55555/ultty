use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use ultty_protocol::control::{
    auth_token_path, control_socket_path, ControlEnvelope, ControlRequest, OutputFormat,
};
use ultty_protocol::{Key, Modifiers};

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

    let request = match parse_cli_request(&args) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    };
    let request = with_auth_token(request, read_auth_token());
    let envelope = ControlEnvelope {
        output_format: if use_json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        },
        request,
    };
    let command =
        serde_json::to_string(&envelope).expect("control request envelope should serialize");

    match send_command(&command, timeout_secs) {
        Ok(response) => {
            print!("{}", response);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Read the control auth token ultty wrote at startup, if any.
///
/// `ULTTY_AUTH_TOKEN` overrides the file so scripts can pass a token in
/// explicitly.
fn read_auth_token() -> Option<String> {
    if let Ok(token) = std::env::var("ULTTY_AUTH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    std::fs::read_to_string(auth_token_path())
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
        "text" => Ok(ControlRequest::GetText {
            include_scrollback: rest.iter().any(|arg| arg == "--scrollback"),
            scrollback_lines: None,
        }),
        "selection" => Ok(ControlRequest::GetSelection),
        "send-text" => {
            let text = rest.join(" ");
            if text.is_empty() {
                Err("send-text requires text".to_string())
            } else {
                Ok(ControlRequest::SendText { text })
            }
        }
        "send-key" => {
            let key = parse_key(
                rest.first()
                    .ok_or_else(|| "send-key requires a key".to_string())?,
            )
            .ok_or_else(|| "unknown key".to_string())?;
            let modifiers = parse_modifiers(rest.get(1).map(String::as_str).unwrap_or(""));
            Ok(ControlRequest::SendKey { key, modifiers })
        }
        "send-raw" => {
            let hex = rest.join("");
            let data = hex_decode(&hex).ok_or_else(|| "send-raw requires valid hex".to_string())?;
            Ok(ControlRequest::SendRaw { data })
        }
        "new-tab" => Ok(ControlRequest::NewTab {
            title: None,
            command: rest.first().cloned(),
            args: rest.iter().skip(1).cloned().collect(),
            working_dir: None,
        }),
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
                })
            }
        }
        "wait-for" => parse_wait_for(rest),
        "auth" => Ok(ControlRequest::Authenticate {
            token: rest
                .first()
                .ok_or_else(|| "auth requires a token".to_string())?
                .clone(),
        }),
        _ => Err(format!("unknown command: {}", command)),
    }
}

fn parse_wait_for(args: &[String]) -> Result<ControlRequest, String> {
    let mut timeout_ms = 30000;
    let mut pattern_parts = Vec::new();
    let mut index = 0;

    while index < args.len() {
        if args[index] == "--timeout" {
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
    if pattern.is_empty() {
        Err("wait-for requires a pattern".to_string())
    } else {
        Ok(ControlRequest::WaitFor {
            pattern,
            regex: false,
            timeout_ms,
        })
    }
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

fn send_command(command: &str, timeout_secs: u64) -> Result<String, String> {
    let socket_path = control_socket_path();

    let mut stream = UnixStream::connect(&socket_path)
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
    --read-timeout <secs>   Socket read timeout in seconds (default: 5)
    -V, --version           Show ulttyctl version without connecting to Ultty
    -h, --help              Show this help

INPUT COMMANDS:
    send-text <text>        Send text as if typed
    send-key <key> [mods]   Send a key press (e.g., send-key enter, send-key a ctrl)
    send-raw <hex>          Send raw bytes (hex-encoded, e.g., send-raw 1b5b41)

TAB COMMANDS:
    new-tab [cmd] [args]    Create new tab with optional command
    switch-tab <n>          Switch to tab number n (1-indexed)
    close-tab [n]           Close tab n (or current if omitted)
    tabs                    List all tabs

QUERY COMMANDS:
    info                    Get terminal info (size, cursor, etc.)
    text [--scrollback]     Get visible text (optionally with scrollback)
    selection               Get current selection
    search <pattern>        Search for pattern in scrollback

WAIT COMMANDS:
    wait-for <pattern> [--timeout <ms>]
                            Wait for pattern to appear (default 30s timeout)

WINDOW COMMANDS:
    focus                   Focus the ultty window
    fullscreen              Toggle fullscreen
    opacity <0.0-1.0>       Set window opacity
    resize <width> <height> Resize the window (pixels)

EXECUTION:
    run <command> [args]    Run a command in current tab

AUTHENTICATION:
    Every command is authenticated automatically with the token ultty writes
    to $XDG_RUNTIME_DIR/ultty-auth.token at startup (override with the
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
"#
    );
}

#[cfg(test)]
mod tests {
    use super::{
        parse_cli_request, read_response, should_print_version, version_output, with_auth_token,
    };

    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::Duration;

    use ultty_protocol::control::ControlRequest;

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
    fn parse_cli_request_parses_json_query_command() {
        assert_eq!(
            parse_cli_request(&["info".to_string()]).unwrap(),
            ControlRequest::GetInfo
        );
    }
}
