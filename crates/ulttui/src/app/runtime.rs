//! Runtime command scheduling for Ulttui apps.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::runtime::Runtime;

use super::{App, Application, SpawnedTask, TaskDone};
use crate::cmd::Cmd;

impl<A: Application> App<A> {
    pub(super) fn process_cmd(&mut self, cmd: Cmd<A::Message>, rt: &Runtime) {
        // Bound on how deep a `Cmd::Message -> update -> Cmd::Message` chain
        // may recurse within one call. Messages beyond the bound are deferred
        // to the queue (processed on a later drain) rather than dropped, so a
        // wide batch of independent messages is never truncated.
        const MAX_CHAIN_DEPTH: usize = 100;
        let mut stack = vec![(cmd, 0usize)];

        while let Some((cmd, depth)) = stack.pop() {
            match cmd {
                Cmd::None => {}
                Cmd::Message(msg) => {
                    if depth >= MAX_CHAIN_DEPTH {
                        tracing::warn!(
                            "process_cmd: Cmd::Message chain exceeded depth {}; deferring to next tick",
                            MAX_CHAIN_DEPTH,
                        );
                        self.msg_queue.push_back(msg);
                        continue;
                    }
                    let next_cmd = self.app.update(msg);
                    stack.push((next_cmd, depth + 1));
                }
                Cmd::Batch(cmds) => {
                    for cmd in cmds.into_iter().rev() {
                        stack.push((cmd, depth));
                    }
                }
                Cmd::Spawn(future) => {
                    self.spawn_task(rt, None, future);
                }
                Cmd::SpawnWithId { id, future } => {
                    self.cancel_spawn(&id);
                    self.spawn_task(rt, Some(id), future);
                }
                Cmd::CancelSpawn { id } => {
                    self.cancel_spawn(&id);
                }
                Cmd::After(duration, f) => {
                    self.spawn_task(rt, None, async move {
                        tokio::time::sleep(duration).await;
                        f()
                    });
                }
                Cmd::Subscribe { id, stream } => {
                    if let Some(handle) = self.cmd_subscription_handles.remove(&id) {
                        handle.abort();
                    }

                    let tx = self.proxy.clone();
                    let handle = rt.spawn(async move {
                        use futures_util::StreamExt;

                        let mut stream = stream;
                        while let Some(msg) = stream.next().await {
                            if tx.send(msg).is_err() {
                                break;
                            }
                        }
                    });

                    self.cmd_subscription_handles.insert(id, handle);
                }
                Cmd::Unsubscribe { id } => {
                    if let Some(handle) = self.cmd_subscription_handles.remove(&id) {
                        handle.abort();
                    }
                }
                Cmd::ShowToast(toast) => {
                    self.active_toasts.push((toast, std::time::Instant::now()));
                }
                Cmd::PushScreen(mut screen) => {
                    screen.on_mount();
                    self.screen_stack.push(screen);
                }
                Cmd::PopScreen => {
                    if let Some(mut screen) = self.screen_stack.pop() {
                        screen.on_unmount();
                    }
                }
                Cmd::Quit => {
                    self.quit_requested = true;
                }
            }
        }
    }

    /// Run `future` on the runtime; its message comes back through
    /// `task_rx`, waking the event loop.
    fn spawn_task(
        &mut self,
        rt: &Runtime,
        id: Option<String>,
        future: impl Future<Output = A::Message> + Send + 'static,
    ) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        let (tx, waker) = (self.task_tx.clone(), self.waker.clone());
        let handle = rt.spawn(async move {
            let msg = future.await;
            let done = TaskDone {
                cancelled: token,
                msg,
            };
            if tx.send(done).is_ok() {
                waker.wake();
            }
        });
        self.spawn_handles.push(SpawnedTask {
            id,
            handle,
            cancelled,
        });
    }

    /// Abort and forget the named spawned task, if it is still tracked. A
    /// message it already produced is dropped.
    fn cancel_spawn(&mut self, id: &str) {
        if let Some(index) = self
            .spawn_handles
            .iter()
            .position(|task| task.id.as_deref() == Some(id))
        {
            let task = self.spawn_handles.swap_remove(index);
            task.cancelled.store(true, Ordering::SeqCst);
            task.handle.abort();
        }
    }

    /// Queue the messages of finished spawned tasks and forget their
    /// handles.
    pub(super) fn poll_spawned_tasks(&mut self, rt: &Runtime) -> bool {
        // Forget finished tasks first. A task sends before it finishes, so
        // whatever a forgotten task sent is already in the channel and is
        // taken below; while its handle is still kept, cancelling or
        // replacing it by id still drops its message.
        let mut index = 0;
        while index < self.spawn_handles.len() {
            if self.spawn_handles[index].handle.is_finished() {
                let task = self.spawn_handles.swap_remove(index);
                // is_finished() == true guarantees block_on returns immediately.
                if let Err(e) = rt.block_on(task.handle) {
                    if !e.is_cancelled() {
                        // A task that panicked never sends.
                        tracing::warn!("Spawned task {:?} panicked: {}", task.id, e);
                    }
                }
            } else {
                index += 1;
            }
        }
        let mut produced_messages = false;
        while let Ok(done) = self.task_rx.try_recv() {
            self.spawn_handles
                .retain(|task| !Arc::ptr_eq(&task.cancelled, &done.cancelled));
            if !done.cancelled.load(Ordering::SeqCst) {
                self.msg_queue.push_back(done.msg);
                produced_messages = true;
            }
        }
        produced_messages
    }
}
