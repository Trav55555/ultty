use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use ultty_protocol::control::control_socket_path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
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
    // Remove --read-timeout and its value
    let args = remove_flag_with_value(args, "--read-timeout");

    let command = if use_json {
        // Pass the text command; the server handles both JSON and plain text
        args.join(" ")
    } else {
        args.join(" ")
    };

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
    auth <token>            Authenticate with token

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
    use super::read_response;

    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::Duration;

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
}
