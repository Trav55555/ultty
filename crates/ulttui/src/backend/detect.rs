//! Backend detection logic.
//!
//! Native mode is used only when the app runs directly on an Ultty tab's
//! terminal. Ultty gives each tab's shell `ULTTY_SOCKET` (the tab's protocol
//! socket) and `ULTTY_TTY` (the tab's PTY). Both are inherited by anything
//! started from that shell, including a tmux or screen server, `script`, or a
//! nested terminal, which then run the app on a *different* terminal. Drawing
//! through the socket from there would paint over the Ultty tab and bypass
//! the program in between, so the app's own terminal must match `ULTTY_TTY`.
//!
//! `ULTTY_FORCE_NATIVE=1` skips the terminal check.
//!
//! Without a usable socket, an app on a terminal probes for the in-band
//! transport (ADR 0018), which reaches Ultty over ssh, sudo, containers, and
//! recorders. `ULTTY_IN_BAND=0` skips the probe.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn is_ultty() -> bool {
    socket_path().is_some()
}

/// The native protocol socket to use, if this process runs on the Ultty
/// terminal that owns it.
pub fn socket_path() -> Option<PathBuf> {
    native_socket(
        std::env::var_os("ULTTY_SOCKET"),
        std::env::var_os("ULTTY_TTY"),
        std::env::var_os("ULTTY_FORCE_NATIVE").is_some_and(|v| v == "1"),
        controlling_terminal,
    )
}

/// Whether to probe for in-band native mode: stdin and stdout are a
/// terminal that might be Ultty.
pub fn in_band_candidate() -> bool {
    // SAFETY: isatty only inspects the fd.
    let on_tty = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let multiplexer = std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some();
    on_tty
        && !multiplexer
        && probe_allowed(
            std::env::var_os("ULTTY_IN_BAND").as_deref(),
            std::env::var_os("TERM").as_deref(),
        )
}

fn probe_allowed(opt: Option<&std::ffi::OsStr>, term: Option<&std::ffi::OsStr>) -> bool {
    if opt.is_some_and(|v| v == "0") {
        return false;
    }
    let term = term.and_then(|t| t.to_str()).unwrap_or("");
    // A dumb terminal has no escape sequences, and the Linux console prints
    // an unknown APC string instead of ignoring it. tmux takes an APC string
    // as a pane title and GNU screen as its hardstatus line, so the probe
    // would rename the pane; their TERM values survive ssh, unlike the TMUX
    // and STY variables.
    !(matches!(term, "dumb" | "linux") || term.starts_with("tmux") || term.starts_with("screen"))
}

fn native_socket(
    socket: Option<OsString>,
    expected_tty: Option<OsString>,
    force: bool,
    current_tty: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let socket = PathBuf::from(socket?);
    if force {
        return Some(socket);
    }
    let expected = PathBuf::from(expected_tty?);
    let current = current_tty()?;
    same_terminal(&current, &expected).then_some(socket)
}

fn same_terminal(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Name of the terminal on stdin, stdout, or stderr, whichever is one.
fn controlling_terminal() -> Option<PathBuf> {
    [0, 1, 2].into_iter().find_map(tty_name)
}

fn tty_name(fd: libc::c_int) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: `buf` is a writable buffer of the length passed; ttyname_r
    // writes a NUL-terminated name into it or returns an error code.
    let rc = unsafe { libc::ttyname_r(fd, buf.as_mut_ptr(), buf.len()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: on success ttyname_r NUL-terminated `buf`.
    let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tty(name: &str) -> impl FnOnce() -> Option<PathBuf> + '_ {
        move || Some(PathBuf::from(name))
    }

    #[test]
    fn uses_the_socket_on_the_terminal_that_owns_it() {
        let socket = native_socket(
            Some("/run/ultty.sock".into()),
            Some("/dev/pts/3".into()),
            false,
            tty("/dev/pts/3"),
        );
        assert_eq!(socket, Some(PathBuf::from("/run/ultty.sock")));
    }

    #[test]
    fn ignores_an_inherited_socket_on_another_terminal() {
        // e.g. inside a tmux pane started from the Ultty tab
        let socket = native_socket(
            Some("/run/ultty.sock".into()),
            Some("/dev/pts/3".into()),
            false,
            tty("/dev/pts/9"),
        );
        assert_eq!(socket, None);
    }

    #[test]
    fn requires_the_owning_terminal_to_be_named() {
        let socket = native_socket(
            Some("/run/ultty.sock".into()),
            None,
            false,
            tty("/dev/pts/3"),
        );
        assert_eq!(socket, None);
    }

    #[test]
    fn needs_a_terminal_unless_forced() {
        let socket = || Some(OsString::from("/run/ultty.sock"));
        let expected = || Some(OsString::from("/dev/pts/3"));
        assert_eq!(native_socket(socket(), expected(), false, || None), None);
        assert_eq!(
            native_socket(socket(), expected(), true, || None),
            Some(PathBuf::from("/run/ultty.sock"))
        );
    }

    #[test]
    fn in_band_probe_is_skipped_when_disabled_or_unsafe() {
        use std::ffi::OsStr;
        let xterm = Some(OsStr::new("xterm-256color"));
        assert!(probe_allowed(None, xterm));
        assert!(probe_allowed(None, None));
        assert!(probe_allowed(Some(OsStr::new("1")), xterm));
        assert!(!probe_allowed(Some(OsStr::new("0")), xterm));
        assert!(!probe_allowed(None, Some(OsStr::new("dumb"))));
        assert!(!probe_allowed(None, Some(OsStr::new("linux"))));
        assert!(!probe_allowed(None, Some(OsStr::new("tmux-256color"))));
        assert!(!probe_allowed(
            None,
            Some(OsStr::new("screen.xterm-256color"))
        ));
    }

    #[test]
    fn no_socket_means_ansi_mode() {
        assert_eq!(
            native_socket(None, Some("/dev/pts/3".into()), true, tty("/dev/pts/3")),
            None
        );
    }
}
