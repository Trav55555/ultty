//! A tmux server of a test's own, shared by the integration tests that run
//! tmux inside a test terminal.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A tmux server of the test's own, isolated from the user's tmux.
///
/// Every invocation passes `-S <socket>`, so the test never touches the
/// user's server, and `-f /dev/null`, so the server never loads the user's
/// `~/.tmux.conf`. A user config can run plugins (tmux-resurrect with
/// tmux-continuum restores a whole saved workspace into the server and
/// autosaves over it) and rebind the prefix the tests type.
///
/// The session sets `exit-unattached`, so the server exits by itself once
/// its client goes, which happens when the test terminal closes. That
/// covers a test process killed before `Drop` runs. `Drop` still kills the
/// server and removes the socket.
pub struct TmuxServer {
    socket: PathBuf,
}

impl TmuxServer {
    pub fn new(test: &str) -> Self {
        Self {
            socket: std::env::temp_dir()
                .join(format!("ultty-tmux-{test}-{}.sock", std::process::id())),
        }
    }

    /// The arguments every `tmux` invocation on this server starts with.
    pub fn args(&self) -> Vec<String> {
        vec![
            "-f".to_owned(),
            "/dev/null".to_owned(),
            "-S".to_owned(),
            self.socket.display().to_string(),
        ]
    }

    /// The tmux subcommands that create session `test`, attached to the
    /// terminal tmux runs in, and then let the server exit when no client
    /// is attached. `exit-unattached` is set after `new-session` in the
    /// same command list, when the client is already attached, so the
    /// server cannot exit before the client arrives.
    pub fn session_args() -> [&'static str; 8] {
        [
            "new-session",
            "-s",
            "test",
            ";",
            "set-option",
            "-s",
            "exit-unattached",
            "on",
        ]
    }

    /// `tmux` on this server, ready for a subcommand.
    pub fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(self.args());
        command
    }

    /// The command line, typed into a test terminal, that starts session
    /// `test` on this server attached to that terminal. `TMUX` is cleared
    /// so a run inside tmux does not refuse to nest.
    pub fn attach_command(&self) -> String {
        let words: Vec<String> = self
            .args()
            .iter()
            .map(String::as_str)
            .chain(Self::session_args())
            .map(shell_quote)
            .collect();
        format!("unset TMUX; tmux {}\r", words.join(" "))
    }
}

/// `word` single-quoted for a POSIX shell.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        // Best effort: a server that never started, or already exited,
        // has nothing to clean.
        let _ = self
            .command()
            .arg("kill-server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&self.socket);
    }
}
