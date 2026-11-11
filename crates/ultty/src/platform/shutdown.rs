//! Turning termination signals into an orderly exit.
//!
//! Killed by SIGTERM, SIGHUP or SIGINT with the default disposition, Ultty
//! would die on the spot and leave its sockets and token behind. Instead
//! the signals are caught and handed to a callback that asks the event loop
//! to exit, so the servers' `Drop` cleanup runs as it does when the window
//! closes.

use std::io;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// The signals that ask Ultty to exit.
pub const SHUTDOWN_SIGNALS: [libc::c_int; 3] = [SIGTERM, SIGHUP, SIGINT];

/// Call `on_first` once, on a thread of its own, when the process receives
/// any of [`SHUTDOWN_SIGNALS`].
///
/// A signal this process inherited as ignored stays ignored: `nohup ultty`
/// must still outlive a hangup. Later signals are swallowed, so shutdown
/// starts once however many arrive; a process that hangs on the way out
/// needs SIGKILL. The handlers stay for the life of the process. Child
/// processes are unaffected: exec resets caught signals to their default
/// action.
pub fn on_shutdown_signal(on_first: impl FnOnce() + Send + 'static) -> io::Result<()> {
    let mut caught = Vec::with_capacity(SHUTDOWN_SIGNALS.len());
    for signal in SHUTDOWN_SIGNALS {
        if is_ignored(signal)? {
            tracing::debug!("Signal {} was ignored at start; leaving it so", signal);
        } else {
            caught.push(signal);
        }
    }
    // signal-hook's handler only writes to a self-pipe, which is
    // async-signal-safe; the callback runs on the thread below.
    let mut signals = Signals::new(caught)?;
    std::thread::Builder::new()
        .name("ultty-signals".into())
        .spawn(move || {
            let mut on_first = Some(on_first);
            for signal in signals.forever() {
                match on_first.take() {
                    Some(on_first) => {
                        tracing::info!("Received signal {}; shutting down", signal);
                        on_first();
                    }
                    None => tracing::info!("Received signal {} while shutting down", signal),
                }
            }
        })?;
    Ok(())
}

/// Whether `signal`'s disposition is currently `SIG_IGN`.
fn is_ignored(signal: libc::c_int) -> io::Result<bool> {
    // SAFETY: a zeroed sigaction (plain integers, a mask and a handler
    // address) is a valid out-parameter, and a null `act` makes sigaction
    // only read the current disposition, changing nothing.
    let (rc, current) = unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        let rc = libc::sigaction(signal, std::ptr::null(), &mut current);
        (rc, current)
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(current.sa_sigaction == libc::SIG_IGN)
}
