//! Bridge socket readiness (native protocol and control servers) into the
//! winit event loop. PTY output does not come through here: each terminal's
//! I/O thread wakes the UI directly.

use std::os::fd::{BorrowedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use polling::{Event as PollEvent, Events, Poller};
use winit::event_loop::EventLoopProxy;

use super::AppEvent;

const WATCH_TIMEOUT: Duration = Duration::from_millis(100);
const WAKE_COALESCE: Duration = Duration::from_millis(1);
const REREGISTER_INTERVAL: Duration = Duration::from_secs(1);

/// A runtime fd and whether it should also wake the loop when writable.
type WatchedFd = (RawFd, bool);

/// Sends a winit user event when any tracked runtime fd becomes readable,
/// or writable for the fds asked to be watched for that.
pub(super) struct SocketWakeup {
    fds: Arc<Mutex<Vec<WatchedFd>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SocketWakeup {
    pub(super) fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let fds = Arc::new(Mutex::new(Vec::<WatchedFd>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_fds = Arc::clone(&fds);
        let thread_stop = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("ultty-runtime-wakeup".to_string())
            .spawn(move || watch_runtime_readiness(thread_fds, thread_stop, proxy))
            .ok();

        Self { fds, stop, handle }
    }

    /// Watch `readable` for reading and `writable` for writing. An fd may
    /// appear in both.
    pub(super) fn set_fds(&self, readable: Vec<RawFd>, writable: Vec<RawFd>) {
        let next = watched_fds(readable, writable);

        if let Ok(mut current) = self.fds.lock() {
            if *current != next {
                *current = next;
            }
        }
    }
}

impl Drop for SocketWakeup {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn watch_runtime_readiness(
    fds: Arc<Mutex<Vec<WatchedFd>>>,
    stop: Arc<AtomicBool>,
    proxy: EventLoopProxy<AppEvent>,
) {
    let poller = match Poller::new() {
        Ok(poller) => poller,
        Err(err) => {
            tracing::warn!("runtime wakeup disabled: cannot create poller: {err}");
            return;
        }
    };
    let mut registered: Vec<WatchedFd> = Vec::new();
    let mut last_registration = Instant::now();
    let mut events = Events::new();

    while !stop.load(Ordering::Relaxed) {
        let wanted = fds.lock().map(|fds| fds.clone()).unwrap_or_default();
        // A closed socket's fd number can be reused by a new one while the
        // set of numbers stays the same, silently dropping its epoll
        // registration; re-registering now and then recovers from that.
        if wanted != registered || last_registration.elapsed() >= REREGISTER_INTERVAL {
            for &(fd, _) in &registered {
                // SAFETY: only used to name the fd for removal; a closed or
                // reused number makes epoll_ctl fail, which is ignored.
                let _ = poller.delete(unsafe { BorrowedFd::borrow_raw(fd) });
            }
            registered.clear();
            for (index, &(fd, writable)) in wanted.iter().enumerate() {
                // SAFETY: the fds are sockets owned by the UI thread's
                // servers; a stale number fails registration and is skipped
                // until the next refresh.
                if unsafe { poller.add(fd, interest(index, writable)) }.is_ok() {
                    registered.push((fd, writable));
                }
            }
            last_registration = Instant::now();
        } else {
            // Readiness is one-shot; re-arm everything for the next wait.
            for (index, &(fd, writable)) in registered.iter().enumerate() {
                // SAFETY: as above; failures are ignored.
                let _ = poller.modify(
                    unsafe { BorrowedFd::borrow_raw(fd) },
                    interest(index, writable),
                );
            }
        }

        events.clear();
        if poller.wait(&mut events, Some(WATCH_TIMEOUT)).is_err() {
            thread::sleep(WATCH_TIMEOUT);
            continue;
        }
        if !events.is_empty() {
            if proxy.send_event(AppEvent::RuntimeReadable).is_err() {
                break;
            }
            thread::sleep(WAKE_COALESCE);
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
        .map(|fd| (fd, writable.contains(&fd)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::watched_fds;

    #[test]
    fn watched_fds_sorts_and_deduplicates() {
        assert_eq!(
            watched_fds(vec![3, 2, 3], vec![]),
            vec![(2, false), (3, false)]
        );
    }

    #[test]
    fn watched_fds_merges_writable_interest() {
        assert_eq!(
            watched_fds(vec![4, 2], vec![5, 2]),
            vec![(2, true), (4, false), (5, true)]
        );
    }
}
