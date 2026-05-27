//! Bridge runtime fd readiness into the winit event loop.

use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use polling::{Event as PollEvent, Events, Poller};
use winit::event_loop::EventLoopProxy;

use super::AppEvent;

const WATCH_TIMEOUT: Duration = Duration::from_millis(100);
const WAKE_COALESCE: Duration = Duration::from_millis(1);

/// Sends a winit user event when any tracked runtime fd becomes readable.
pub(super) struct PtyWakeup {
    fds: Arc<Mutex<Vec<RawFd>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PtyWakeup {
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

impl Drop for PtyWakeup {
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
    while !stop.load(Ordering::Relaxed) {
        let snapshot = fds.lock().map(|fds| fds.clone()).unwrap_or_default();
        if snapshot.is_empty() {
            thread::sleep(WATCH_TIMEOUT);
            continue;
        }

        if wait_for_readable_fd(&snapshot, WATCH_TIMEOUT).unwrap_or(false) {
            if proxy.send_event(AppEvent::RuntimeReadable).is_err() {
                break;
            }
            thread::sleep(WAKE_COALESCE);
        }
    }
}

fn wait_for_readable_fd(fds: &[RawFd], timeout: Duration) -> std::io::Result<bool> {
    let poller = Poller::new()?;
    for (index, fd) in fds.iter().copied().enumerate() {
        // SAFETY: `fd` is a best-effort snapshot owned by the UI thread. If it
        // has already closed, registration fails and this wake cycle skips it.
        let _ = unsafe { poller.add(fd, PollEvent::readable(index)) };
    }

    let mut events = Events::new();
    poller.wait(&mut events, Some(timeout))?;
    Ok(!events.is_empty())
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
