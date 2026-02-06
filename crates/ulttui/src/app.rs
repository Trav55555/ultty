//! Application trait and runner.

use std::time::{Duration, Instant};

use crate::backend::Backend;
use crate::cmd::{Cmd, Sub};
use crate::event::{Event, Key, KeyEvent};
use crate::focus::FocusManager;
use crate::widgets::View;
use tokio::runtime::Runtime;

/// Main application trait.
pub trait Application: Sized {
    /// Message type for state updates.
    type Message: Send + 'static;

    /// Update state based on a message.
    fn update(&mut self, _msg: Self::Message) -> Cmd<Self::Message> {
        Cmd::none()
    }

    /// Subscriptions for external events.
    fn subscriptions(&self) -> Sub<Self::Message> {
        Sub::none()
    }

    /// Handle an event and optionally return a message.
    fn on_event(&mut self, _event: &Event) -> Option<Self::Message> {
        None
    }

    /// Render the UI.
    fn view(&self) -> impl View;

    /// Run the application.
    fn run(self) -> anyhow::Result<()> {
        App::new(self).run()
    }

    /// Run the application with a specific theme.
    fn run_with_theme(self, theme: crate::theme::Theme) -> anyhow::Result<()> {
        App::new_with_theme(self, theme).run()
    }
}

/// Application runner.
pub struct App<A: Application> {
    app: A,
    backend: Backend,
    focus: FocusManager,
    subscriptions: Vec<SubState<A::Message>>,
    spawn_handles: Vec<tokio::task::JoinHandle<A::Message>>,
}

#[allow(dead_code)]
struct SubState<M> {
    interval: Duration,
    f: Box<dyn Fn() -> M + Send + 'static>,
    last_tick: Instant,
}

impl<A: Application> App<A> {
    /// Create a new app runner.
    pub fn new(app: A) -> Self {
        Self {
            app,
            backend: Backend::detect(),
            focus: FocusManager::new(),
            subscriptions: Vec::new(),
            spawn_handles: Vec::new(),
        }
    }

    pub fn new_with_theme(app: A, theme: crate::theme::Theme) -> Self {
        crate::theme::Theme::set_current(theme);
        Self::new(app)
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let rt = Runtime::new()?;
        self.backend.init()?;

        loop {
            let fm = std::mem::take(&mut self.focus);
            let old_fm = FocusManager::set_current(fm);

            FocusManager::with_current_mut(|fm| fm.begin_frame());
            {
                let view = self.app.view();
                self.backend.render(&view)?;
            }
            FocusManager::with_current_mut(|fm| fm.end_frame());

            self.focus = FocusManager::set_current(old_fm);

            self.process_subscriptions(&rt);

            self.poll_spawned_tasks(&rt);

            if let Some(event) = self.backend.poll_event()? {
                match &event {
                    Event::Quit => break,
                    Event::Key(key_event) => {
                        if self.handle_navigation(key_event) {
                            continue;
                        }
                    }
                    _ => {}
                }

                if let Some(msg) = self.app.on_event(&event) {
                    let cmd = self.app.update(msg);
                    self.process_cmd(cmd, &rt);
                }
            }
        }

        self.backend.shutdown()?;
        Ok(())
    }

    fn handle_navigation(&mut self, key_event: &KeyEvent) -> bool {
        match key_event.key {
            Key::Tab if !key_event.modifiers.shift => {
                self.focus.focus_next();
                true
            }
            Key::Tab if key_event.modifiers.shift => {
                self.focus.focus_prev();
                true
            }
            _ => false,
        }
    }

    fn process_subscriptions(&mut self, rt: &Runtime) {
        let now = Instant::now();
        let old_states = std::mem::take(&mut self.subscriptions);
        let mut new_states = Vec::new();
        let mut messages = Vec::new();
        let mut index = 0;

        self.collect_subscriptions(
            self.app.subscriptions(),
            now,
            &old_states,
            &mut index,
            &mut new_states,
            &mut messages,
        );

        self.subscriptions = new_states;

        for msg in messages {
            let cmd = self.app.update(msg);
            self.process_cmd(cmd, rt);
        }
    }

    fn collect_subscriptions(
        &self,
        sub: Sub<A::Message>,
        now: Instant,
        old_states: &[SubState<A::Message>],
        index: &mut usize,
        new_states: &mut Vec<SubState<A::Message>>,
        messages: &mut Vec<A::Message>,
    ) {
        match sub {
            Sub::None => {}
            Sub::Every { interval, f } => {
                let last_tick = old_states
                    .get(*index)
                    .map(|state| state.last_tick)
                    .unwrap_or(now);

                let mut next_tick = last_tick;
                if now.duration_since(last_tick) >= interval {
                    messages.push(f());
                    next_tick = now;
                }

                new_states.push(SubState {
                    interval,
                    f,
                    last_tick: next_tick,
                });
                *index += 1;
            }
            Sub::Batch(subs) => {
                for sub in subs {
                    self.collect_subscriptions(sub, now, old_states, index, new_states, messages);
                }
            }
        }
    }

    fn process_cmd(&mut self, cmd: Cmd<A::Message>, rt: &Runtime) {
        const MAX_ITERATIONS: usize = 100;
        let mut stack = vec![cmd];
        let mut iterations = 0;

        while let Some(cmd) = stack.pop() {
            match cmd {
                Cmd::None => {}
                Cmd::Message(msg) => {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
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
            }
        }
    }

    fn poll_spawned_tasks(&mut self, rt: &Runtime) {
        let mut index = 0;
        while index < self.spawn_handles.len() {
            if self.spawn_handles[index].is_finished() {
                let handle = self.spawn_handles.swap_remove(index);
                if let Ok(msg) = rt.block_on(handle) {
                    let cmd = self.app.update(msg);
                    self.process_cmd(cmd, rt);
                }
            } else {
                index += 1;
            }
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<A: Application> App<A> {
    /// Create an app with a specific backend (for testing).
    pub fn new_with_backend(app: A, backend: Backend) -> Self {
        Self {
            app,
            backend,
            focus: FocusManager::new(),
            subscriptions: Vec::new(),
            spawn_handles: Vec::new(),
        }
    }
}

#[cfg(test)]
mod async_tests {
    use super::Runtime;
    use std::time::{Duration, Instant};

    #[test]
    fn cmd_spawn_produces_message() {
        let rt = Runtime::new().expect("runtime");
        let handle = rt.spawn(async { 42 });
        let result = rt.block_on(handle).expect("join");
        assert_eq!(result, 42);
    }

    #[test]
    fn cmd_after_produces_message_after_delay() {
        let rt = Runtime::new().expect("runtime");
        let start = Instant::now();
        let handle = rt.spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            "done"
        });
        let result = rt.block_on(handle).expect("join");
        assert_eq!(result, "done");
        assert!(start.elapsed() >= Duration::from_millis(40));
    }
}
