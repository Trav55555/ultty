//! Waking the event loop: from other threads ([`Proxy`]) and on file
//! descriptors becoming readable.
//!
//! The event loop sleeps in `poll(2)` on the terminal's input and the read
//! end of a socket pair until the next timer deadline. Anything that has work
//! for it (a finished command, a subscription item, a [`Proxy::send`] from
//! another thread) queues a message and then writes one byte to the pair.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;

/// Wakes the event loop out of its wait. Cheap to clone; any thread.
#[derive(Clone, Default)]
pub(crate) struct Waker {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// A wake happened that the loop has not looked at yet. Wakes before
    /// the loop listens only set this; the loop checks it before sleeping.
    pending: AtomicBool,
    /// Write end of the loop's wake socket, set once the loop listens.
    signal: OnceLock<UnixStream>,
}

impl Waker {
    /// Make the loop's current or next wait return.
    pub(crate) fn wake(&self) {
        if self.shared.pending.swap(true, Ordering::AcqRel) {
            // Already pending: the loop has not slept since, or a byte is
            // already on its way.
            return;
        }
        if let Some(signal) = self.shared.signal.get() {
            // Non-blocking (a full buffer already means "wake up"), and no
            // SIGPIPE if the loop is gone: a host may have SIGPIPE restored.
            // SAFETY: one valid byte; the socket is open while `signal` lives.
            unsafe {
                libc::send(
                    signal.as_raw_fd(),
                    [1u8].as_ptr().cast(),
                    1,
                    libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
                )
            };
        }
    }

    /// Whether there was a wake since the last call, clearing it.
    pub(crate) fn take_pending(&self) -> bool {
        self.shared.pending.swap(false, Ordering::AcqRel)
    }

    /// Create the socket the loop sleeps on. Only the first call has any
    /// effect on where wakes go; the loop calls it once, when it starts.
    pub(crate) fn listen(&self) -> io::Result<WakeListener> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        if self.shared.signal.set(writer).is_err() {
            return Err(io::Error::other("the event loop is already listening"));
        }
        Ok(WakeListener { reader })
    }
}

/// The read end of the loop's wake socket.
pub(crate) struct WakeListener {
    reader: UnixStream,
}

impl WakeListener {
    pub(crate) fn fd(&self) -> RawFd {
        self.reader.as_raw_fd()
    }

    /// Discard the wake bytes that have arrived.
    pub(crate) fn drain(&self) {
        let mut buf = [0u8; 64];
        while matches!((&self.reader).read(&mut buf), Ok(n) if n > 0) {}
    }
}

/// Sends messages to a running app's `update` from outside the event loop,
/// such as a thread watching files, and wakes the loop to handle them.
///
/// Get one from [`App::proxy`](crate::app::App::proxy) before calling
/// [`App::run`](crate::app::App::run).
pub struct Proxy<M> {
    tx: mpsc::UnboundedSender<M>,
    waker: Waker,
}

impl<M> Clone for Proxy<M> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<M> std::fmt::Debug for Proxy<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy").finish_non_exhaustive()
    }
}

impl<M> Proxy<M> {
    pub(crate) fn new(tx: mpsc::UnboundedSender<M>, waker: Waker) -> Self {
        Self { tx, waker }
    }

    /// Queue `msg` for the app's `update` and wake the event loop. Gives the
    /// message back once the app has stopped.
    pub fn send(&self, msg: M) -> Result<(), M> {
        self.tx.send(msg).map_err(|err| err.0)?;
        self.waker.wake();
        Ok(())
    }
}

/// Wait until one of `fds` is ready to read, or `timeout` passes (`None`: no
/// limit). Returns whether a descriptor is ready; a signal interrupting the
/// wait counts as not ready.
///
/// A hang-up or error condition counts as ready: the read that follows
/// sees the end of input or the error and reports it. A descriptor that is
/// not open (`POLLNVAL`) is an error here, since no read would end the
/// wait.
pub(crate) fn poll_readable(fds: &[RawFd], timeout: Option<Duration>) -> io::Result<bool> {
    let mut pollfds: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let timeout_ms = match timeout {
        None => -1,
        // Round up, so a wait for a deadline does not wake just before it
        // and spin until it has passed.
        Some(timeout) => {
            let ms = timeout.as_nanos().div_ceil(1_000_000);
            ms.min(libc::c_int::MAX as u128) as libc::c_int
        }
    };
    // SAFETY: `pollfds` is a valid array of `pollfds.len()` entries.
    let rc = unsafe {
        libc::poll(
            pollfds.as_mut_ptr(),
            pollfds.len() as libc::nfds_t,
            timeout_ms,
        )
    };
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(err);
    }
    if pollfds.iter().any(|p| p.revents & libc::POLLNVAL != 0) {
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }
    Ok(rc > 0)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_wake_makes_the_listener_readable_once_until_taken() {
        let waker = Waker::default();
        let listener = waker.listen().unwrap();
        assert!(!poll_readable(&[listener.fd()], Some(Duration::ZERO)).unwrap());

        waker.wake();
        waker.wake();
        assert!(poll_readable(&[listener.fd()], Some(Duration::ZERO)).unwrap());
        listener.drain();
        assert!(waker.take_pending());
        assert!(!waker.take_pending());
        assert!(!poll_readable(&[listener.fd()], Some(Duration::ZERO)).unwrap());
    }

    #[test]
    fn a_wake_before_listening_is_kept_as_pending() {
        let waker = Waker::default();
        waker.wake();
        let _listener = waker.listen().unwrap();
        assert!(waker.take_pending());
    }

    #[test]
    fn a_proxy_send_from_another_thread_ends_the_wait() {
        let waker = Waker::default();
        let listener = waker.listen().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let proxy = Proxy::new(tx, waker);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            proxy.send(7).unwrap();
        });
        let started = Instant::now();
        assert!(poll_readable(&[listener.fd()], Some(Duration::from_secs(10))).unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(rx.try_recv().unwrap(), 7);
        sender.join().unwrap();
    }

    #[test]
    fn a_hang_up_is_ready_and_a_closed_descriptor_is_an_error() {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(writer);
        assert!(poll_readable(&[reader.as_raw_fd()], Some(Duration::ZERO)).unwrap());
        // Far above any descriptor this process opens.
        let never_open = 1 << 20;
        let err = poll_readable(&[never_open], Some(Duration::ZERO)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EBADF));
    }

    const SIGPIPE_CHILD: &str = "ULTTUI_WAKE_SIGPIPE_CHILD";

    /// Run by `waking_after_the_loop_is_gone_does_not_raise_sigpipe` in a
    /// process of its own, since the signal disposition is process-wide.
    #[test]
    fn wake_with_default_sigpipe() {
        if std::env::var_os(SIGPIPE_CHILD).is_none() {
            return;
        }
        // SAFETY: sets the default disposition, as a host program may.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        let waker = Waker::default();
        drop(waker.listen().unwrap());
        waker.wake();
    }

    #[test]
    fn waking_after_the_loop_is_gone_does_not_raise_sigpipe() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "wake::tests::wake_with_default_sigpipe"])
            .env(SIGPIPE_CHILD, "1")
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{:?}\n{}",
            status.status,
            String::from_utf8_lossy(&status.stdout)
        );
    }

    #[test]
    fn the_timeout_is_rounded_up_to_whole_milliseconds() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let started = Instant::now();
        assert!(!poll_readable(&[reader.as_raw_fd()], Some(Duration::from_micros(1500))).unwrap());
        assert!(started.elapsed() >= Duration::from_micros(1500));
    }
}
