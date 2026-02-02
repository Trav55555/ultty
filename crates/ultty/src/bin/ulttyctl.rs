use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use ultty_protocol::control::control_socket_path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_help();
        std::process::exit(0);
    }

    let use_json = args.iter().any(|a| a == "--json");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--json").collect();

    let command = if use_json {
        format!("{{\"type\":\"{}\"}}", args.join(" "))
    } else {
        args.join(" ")
    };

    match send_command(&command) {
        Ok(response) => {
            print!("{}", response);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

fn send_command(command: &str) -> Result<String, String> {
    let socket_path = control_socket_path();

    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("failed to connect to ultty (is it running?): {}", e))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("failed to set timeout: {}", e))?;

    stream
        .write_all(command.as_bytes())
        .map_err(|e| format!("failed to send command: {}", e))?;

    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| format!("failed to shutdown write: {}", e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("failed to read response: {}", e))?;

    Ok(response)
}

fn print_help() {
    println!(
        r#"ulttyctl - control ultty from the command line

USAGE:
    ulttyctl [--json] <command> [args...]

GLOBAL FLAGS:
    --json                  Output in JSON format

INPUT COMMANDS:
    send-text <text>        Send text as if typed
    send-key <key> [mods]   Send a key press (e.g., send-key enter, send-key a ctrl)

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

AUTHENTICATION:
    auth <token>            Authenticate with token

OTHER:
    ping                    Test connection
    version                 Show version info
    screenshot [path]       Take screenshot

EXAMPLES:
    ulttyctl send-text "ls -la"
    ulttyctl send-key enter
    ulttyctl send-key c ctrl
    ulttyctl new-tab htop
    ulttyctl switch-tab 2
    ulttyctl text --scrollback
    ulttyctl search "error"
    ulttyctl wait-for "$ " --timeout 5000
    ulttyctl --json info
"#
    );
}
