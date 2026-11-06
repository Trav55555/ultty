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

/// Sends a winit user event when any tracked runtime fd becomes readable.
pub(super) struct SocketWakeup {
    fds: Arc<Mutex<Vec<RawFd>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SocketWakeup {
    pub(super) fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let fds = Arc::new(Mutex::new(Vec::<RawFd>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_fds = Arc::clone(&fds);
        let thread_stop = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("ultty-runtime-wakeup".to_string())
            .spawn(move || watch_runtime_readiness(thread_fds, thread_stop, proxy))
            .ok();

        Self { fds, stop, handle }
    }

    pub(super) fn set_fds(&self, mut next: Vec<RawFd>) {
        normalize_fds(&mut next);

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
    fds: Arc<Mutex<Vec<RawFd>>>,
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
    let mut registered: Vec<RawFd> = Vec::new();
    let mut last_registration = Instant::now();
    let mut events = Events::new();

    while !stop.load(Ordering::Relaxed) {
        let wanted = fds.lock().map(|fds| fds.clone()).unwrap_or_default();
        // A closed socket's fd number can be reused by a new one while the
        // set of numbers stays the same, silently dropping its epoll
        // registration; re-registering now and then recovers from that.
        if wanted != registered || last_registration.elapsed() >= REREGISTER_INTERVAL {
            for &fd in &registered {
                // SAFETY: only used to name the fd for removal; a closed or
                // reused number makes epoll_ctl fail, which is ignored.
                let _ = poller.delete(unsafe { BorrowedFd::borrow_raw(fd) });
            }
            registered.clear();
            for (index, &fd) in wanted.iter().enumerate() {
                // SAFETY: the fds are listener sockets owned by the UI
                // thread's servers; a stale number fails registration and
                // is skipped until the next refresh.
                if unsafe { poller.add(fd, PollEvent::readable(index)) }.is_ok() {
                    registered.push(fd);
                }
            }
            last_registration = Instant::now();
        } else {
            // Readiness is one-shot; re-arm everything for the next wait.
            for (index, &fd) in registered.iter().enumerate() {
                // SAFETY: as above; failures are ignored.
                let _ = poller.modify(
                    unsafe { BorrowedFd::borrow_raw(fd) },
                    PollEvent::readable(index),
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

fn normalize_fds(fds: &mut Vec<RawFd>) {
    fds.sort_unstable();
    fds.dedup();
}

#[cfg(test)]
mod tests {
    use super::normalize_fds;

    #[test]
    fn normalize_fds_sorts_and_deduplicates() {
        let mut fds = vec![3, 2, 3];
        normalize_fds(&mut fds);
        assert_eq!(fds, vec![2, 3]);
    }
}
