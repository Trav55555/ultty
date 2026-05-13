//! Runtime command scheduling for Ulttui apps.

use tokio::runtime::Runtime;

use super::{App, Application};
use crate::cmd::Cmd;

impl<A: Application> App<A> {
    pub(super) fn process_cmd(&mut self, cmd: Cmd<A::Message>, rt: &Runtime) {
        const MAX_ITERATIONS: usize = 100;
        let mut stack = vec![cmd];
        let mut iterations = 0;

        while let Some(cmd) = stack.pop() {
            match cmd {
                Cmd::None => {}
                Cmd::Message(msg) => {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        tracing::warn!(
                            "process_cmd hit MAX_ITERATIONS ({}); recursive Cmd::Message chain truncated",
                            MAX_ITERATIONS,
                        );
                        return;
                    }
                    let next_cmd = self.app.update(msg);
                    stack.push(next_cmd);
                }
                Cmd::Batch(cmds) => {
                    for cmd in cmds.into_iter().rev() {
                        stack.push(cmd);
                    }
                }
                Cmd::Spawn(future) => {
                    let handle = rt.spawn(future);
                    self.spawn_handles.push(handle);
                }
                Cmd::After(duration, f) => {
                    let handle = rt.spawn(async move {
                        tokio::time::sleep(duration).await;
                        f()
                    });
                    self.spawn_handles.push(handle);
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

    pub(super) fn poll_spawned_tasks(&mut self, rt: &Runtime) {
        let mut index = 0;
        while index < self.spawn_handles.len() {
            if self.spawn_handles[index].is_finished() {
                let handle = self.spawn_handles.swap_remove(index);
                // is_finished() == true guarantees block_on returns immediately.
                match rt.block_on(handle) {
                    Ok(msg) => self.msg_queue.push_back(msg),
                    Err(e) => {
                        tracing::warn!("Spawned task panicked: {}", e);
                    }
                }
            } else {
                index += 1;
            }
        }
    }
}
