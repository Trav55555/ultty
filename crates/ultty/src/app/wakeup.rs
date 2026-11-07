//! Bridge socket readiness (native protocol and control servers) into the
//! winit event loop. PTY output does not come through here: each terminal's
//! I/O thread wakes the UI directly.
//!
//! The watcher thread sleeps in the kernel until a socket is ready or the
//! UI changes the set of sockets; it never wakes on a timer. Once it has
//! woken the UI it leaves the sockets unarmed until [`SocketWakeup::rearm`],
//! which the UI calls after draining them, so a socket that stays ready
//! until the UI gets to it wakes the UI once instead of spinning.

use std::os::fd::{BorrowedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use polling::{Event as PollEvent, Events, Poller};

/// A runtime fd to watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WatchedFd {
    fd: RawFd,
    /// Device and inode of the open file. A socket closed and replaced by
    /// one that reuses its number loses its epoll registration; a changed
    /// identity is how that is noticed.
    identity: Option<(libc::dev_t, libc::ino_t)>,
    /// Also wake when the fd becomes writable.
    writable: bool,
}

/// State shared by the UI thread and the watcher thread.
struct Shared {
    poller: Poller,
    /// The fds the UI wants watched.
    fds: Mutex<Vec<WatchedFd>>,
    /// `fds` changed since the watcher last registered them.
    changed: AtomicBool,
    /// The UI was woken and has not called `rearm` since.
    fired: AtomicBool,
    stop: AtomicBool,
}

/// Calls a waker when any tracked runtime fd becomes readable, or writable
/// for the fds asked to be watched for that.
pub(super) struct SocketWakeup {
    /// `None` when no poller could be created; sockets are then serviced
    /// only when something else wakes the loop.
    shared: Option<Arc<Shared>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SocketWakeup {
    /// `wake` runs on the watcher thread; returning false stops the thread
    /// (the event loop is gone).
    pub(super) fn new(wake: impl Fn() -> bool + Send + 'static) -> Self {
        let poller = match Poller::new() {
            Ok(poller) => poller,
            Err(err) => {
                tracing::warn!("runtime wakeup disabled: cannot create poller: {err}");
                return Self {
                    shared: None,
                    handle: None,
                };
            }
        };
        let shared = Arc::new(Shared {
            poller,
            fds: Mutex::new(Vec::new()),
            changed: AtomicBool::new(false),
            fired: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let handle = thread::Builder::new()
            .name("ultty-runtime-wakeup".to_string())
            .spawn({
                let shared = Arc::clone(&shared);
                move || watch_runtime_readiness(&shared, wake)
            })
            .ok();
        Self {
            shared: Some(shared),
            handle,
        }
    }

    /// Watch `readable` for reading and `writable` for writing. An fd may
    /// appear in both.
    pub(super) fn set_fds(&self, readable: Vec<RawFd>, writable: Vec<RawFd>) {
        let Some(shared) = &self.shared else {
            return;
        };
        let next = watched_fds(readable, writable);
        let Ok(mut current) = shared.fds.lock() else {
            return;
        };
        if *current != next {
            *current = next;
            drop(current);
            shared.changed.store(true, Ordering::Release);
            let _ = shared.poller.notify();
        }
    }

    /// The UI has drained the sockets: watch them again. Cheap when the
    /// watcher has not fired, so it can run after every update.
    pub(super) fn rearm(&self) {
        if let Some(shared) = &self.shared {
            if shared.fired.swap(false, Ordering::AcqRel) {
                let _ = shared.poller.notify();
            }
        }
    }
}

impl Drop for SocketWakeup {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.stop.store(true, Ordering::Release);
            let _ = shared.poller.notify();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn watch_runtime_readiness(shared: &Shared, wake: impl Fn() -> bool) {
    let poller = &shared.poller;
    let mut registered: Vec<WatchedFd> = Vec::new();
    let mut events = Events::new();

    while !shared.stop.load(Ordering::Acquire) {
        let changed = shared.changed.swap(false, Ordering::AcqRel);
        if changed {
            for watched in &registered {
                // SAFETY: only used to name the fd for removal; a closed or
                // reused number makes epoll_ctl fail, which is ignored.
                let _ = poller.delete(unsafe { BorrowedFd::borrow_raw(watched.fd) });
            }
            registered = shared.fds.lock().map(|fds| fds.clone()).unwrap_or_default();
            registered.retain(|watched| {
                // SAFETY: the fds are sockets owned by the UI thread's
                // servers. A stale number fails registration; the socket
                // that next takes the number has a new identity, so the
                // UI's next fd set registers it.
                unsafe { poller.add(watched.fd, PollEvent::none(0)) }.is_ok()
            });
        }
        // Readiness is one-shot. Arm the sockets, unless the UI was woken
        // and has not drained them yet.
        let armed = !shared.fired.load(Ordering::Acquire);
        if changed || armed {
            for (index, watched) in registered.iter().enumerate() {
                let interest = if armed {
                    interest(index, watched.writable)
                } else {
                    PollEvent::none(index)
                };
                // SAFETY: as above; failures are ignored.
                let _ = poller.modify(unsafe { BorrowedFd::borrow_raw(watched.fd) }, interest);
            }
        }

        events.clear();
        if let Err(err) = poller.wait(&mut events, None) {
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!("runtime wakeup stopped: poll failed: {err}");
            break;
        }
        if !events.is_empty() && !shared.fired.swap(true, Ordering::AcqRel) && !wake() {
            break;
        }
    }
}

fn interest(key: usize, writable: bool) -> PollEvent {
    if writable {
        PollEvent::all(key)
    } else {
        PollEvent::readable(key)
    }
}

/// Merge the read and write sets into one sorted entry per fd; epoll takes
/// each fd once.
fn watched_fds(mut readable: Vec<RawFd>, writable: Vec<RawFd>) -> Vec<WatchedFd> {
    readable.extend(&writable);
    readable.sort_unstable();
    readable.dedup();
    readable
        .into_iter()
        .map(|fd| WatchedFd {
            fd,
            identity: identity(fd),
            writable: writable.contains(&fd),
        })
        .collect()
}

/// Device and inode of the file open at `fd`.
fn identity(fd: RawFd) -> Option<(libc::dev_t, libc::ino_t)> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat only writes a `stat` into the buffer; a closed fd makes
    // it fail.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: fstat succeeded, so it initialized the buffer.
    let stat = unsafe { stat.assume_init() };
    Some((stat.st_dev, stat.st_ino))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{watched_fds, SocketWakeup, WatchedFd};

    fn fds_and_interest(fds: &[WatchedFd]) -> Vec<(i32, bool)> {
        fds.iter().map(|w| (w.fd, w.writable)).collect()
    }

    #[test]
    fn watched_fds_sorts_and_deduplicates() {
        assert_eq!(
            fds_and_interest(&watched_fds(vec![3, 2, 3], vec![])),
            vec![(2, false), (3, false)]
        );
    }

    #[test]
    fn watched_fds_merges_writable_interest() {
        assert_eq!(
            fds_and_interest(&watched_fds(vec![4, 2], vec![5, 2])),
            vec![(2, true), (4, false), (5, true)]
        );
    }

    /// A socket closed and replaced by one with the same number must be
    /// registered again, so the two must not look like the same entry.
    #[test]
    fn a_reused_fd_number_is_a_different_entry() {
        let (first, _peer) = UnixStream::pair().unwrap();
        let (second, _second_peer) = UnixStream::pair().unwrap();
        let fd = first.as_raw_fd();
        let before = watched_fds(vec![fd], vec![]);
        // SAFETY: dup2 points `fd` at `second`'s socket; `first` still owns
        // the number and closes it once, when the test ends.
        assert_eq!(unsafe { libc::dup2(second.as_raw_fd(), fd) }, fd);
        let after = watched_fds(vec![fd], vec![]);
        assert_eq!(before[0].fd, after[0].fd);
        assert_ne!(before, after);
    }

    fn counting_wakeup() -> (SocketWakeup, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let wakeup = SocketWakeup::new(move || tx.lock().unwrap().send(()).is_ok());
        (wakeup, rx)
    }

    const QUIET: Duration = Duration::from_millis(150);
    const PROMPT: Duration = Duration::from_secs(5);

    #[test]
    fn idle_sockets_never_wake_the_loop() {
        let (wakeup, woke) = counting_wakeup();
        let (socket, _peer) = UnixStream::pair().unwrap();
        wakeup.set_fds(vec![socket.as_raw_fd()], vec![]);
        assert!(woke.recv_timeout(QUIET).is_err());
    }

    #[test]
    fn a_ready_socket_wakes_once_until_rearmed() {
        let (wakeup, woke) = counting_wakeup();
        let (mut socket, mut peer) = UnixStream::pair().unwrap();
        wakeup.set_fds(vec![socket.as_raw_fd()], vec![]);

        peer.write_all(b"x").unwrap();
        woke.recv_timeout(PROMPT)
            .expect("a readable socket wakes the loop");
        // Not drained yet: no second wake, however long the UI takes.
        assert!(woke.recv_timeout(QUIET).is_err());

        // Rearmed while still readable: it wakes again.
        wakeup.rearm();
        woke.recv_timeout(PROMPT)
            .expect("still readable after rearm");

        // Drained and rearmed: quiet.
        let mut buf = [0u8; 8];
        assert_eq!(socket.read(&mut buf).unwrap(), 1);
        wakeup.rearm();
        assert!(woke.recv_timeout(QUIET).is_err());

        peer.write_all(b"y").unwrap();
        woke.recv_timeout(PROMPT).expect("new data wakes again");
    }

    #[test]
    fn a_socket_added_later_is_watched() {
        let (wakeup, woke) = counting_wakeup();
        let (first, _first_peer) = UnixStream::pair().unwrap();
        wakeup.set_fds(vec![first.as_raw_fd()], vec![]);
        let (second, mut second_peer) = UnixStream::pair().unwrap();
        wakeup.set_fds(vec![first.as_raw_fd(), second.as_raw_fd()], vec![]);

        second_peer.write_all(b"x").unwrap();
        woke.recv_timeout(PROMPT)
            .expect("the new socket wakes the loop");
    }
}
