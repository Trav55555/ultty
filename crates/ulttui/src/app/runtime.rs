//! Runtime command scheduling for Ulttui apps.

use tokio::runtime::Runtime;

use super::{App, Application};
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
                    self.spawn_handles.push((None, rt.spawn(future)));
                }
                Cmd::SpawnWithId { id, future } => {
                    self.cancel_spawn(&id);
                    self.spawn_handles.push((Some(id), rt.spawn(future)));
                }
                Cmd::CancelSpawn { id } => {
                    self.cancel_spawn(&id);
                }
                Cmd::After(duration, f) => {
                    let handle = rt.spawn(async move {
                        tokio::time::sleep(duration).await;
                        f()
                    });
                    self.spawn_handles.push((None, handle));
                }
                Cmd::Subscribe { id, stream } => {
                    if let Some(handle) = self.cmd_subscription_handles.remove(&id) {
                        handle.abort();
                    }

                    let tx = self.subscription_tx.clone();
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

    /// Abort and forget the named spawned task, if it is still tracked.
    fn cancel_spawn(&mut self, id: &str) {
        if let Some(index) = self
            .spawn_handles
            .iter()
            .position(|(name, _)| name.as_deref() == Some(id))
        {
            let (_, handle) = self.spawn_handles.swap_remove(index);
            handle.abort();
        }
    }

    /// Collect messages from finished spawned tasks, pruning their handles.
    pub(super) fn poll_spawned_tasks(&mut self, rt: &Runtime) -> bool {
        let mut produced_messages = false;
        let mut index = 0;
        while index < self.spawn_handles.len() {
            if self.spawn_handles[index].1.is_finished() {
                let (id, handle) = self.spawn_handles.swap_remove(index);
                // is_finished() == true guarantees block_on returns immediately.
                match rt.block_on(handle) {
                    Ok(msg) => {
                        self.msg_queue.push_back(msg);
                        produced_messages = true;
                    }
                    Err(e) if e.is_cancelled() => {
                        tracing::debug!("Spawned task {:?} was cancelled", id);
                    }
                    Err(e) => {
                        tracing::warn!("Spawned task {:?} panicked: {}", id, e);
                    }
                }
            } else {
                index += 1;
            }
        }
        produced_messages
    }
}
