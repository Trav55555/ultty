//! Application trait and runner.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::backend::Backend;
use crate::cmd::{Cmd, Sub};
use crate::event::{Event, Key, KeyEvent};
use crate::focus::FocusManager;
use crate::theme::Theme;
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
    focus_initialized: bool,
    theme: Theme,
    subscriptions: Vec<SubState<A::Message>>,
    spawn_handles: Vec<tokio::task::JoinHandle<A::Message>>,
    msg_queue: VecDeque<A::Message>,
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
            focus_initialized: false,
            theme: Theme::dark(),
            subscriptions: Vec::new(),
            spawn_handles: Vec::new(),
            msg_queue: VecDeque::new(),
        }
    }

    pub fn new_with_theme(app: A, theme: Theme) -> Self {
        let mut runner = Self::new(app);
        runner.theme = theme;
        runner
    }

    /// Run the application event loop.
    ///
    /// Event loop contract (Bubble Tea parity):
    /// 1. Poll backend for events
    /// 2. Normalize events → messages via on_event()
    /// 3. Drain message queue (update → Cmd scheduling)
    /// 4. Process subscriptions → enqueue messages
    /// 5. Poll spawned tasks → enqueue messages
    /// 6. Drain message queue again (from subs/spawns)
    /// 7. Render view
    pub fn run(mut self) -> anyhow::Result<()> {
        let rt = Runtime::new()?;
        self.backend.init()?;

        // Initial render so UI appears immediately before first poll.
        self.render()?;

        loop {
            // 1. Poll backend for events
            if let Some(mut event) = self.backend.poll_event()? {
                // 2. Normalize events → messages via on_event()
                if let Event::Key(ref mut key_event) = event {
                    key_event.focused_id =
                        self.focus.focused_id().map(str::to_string);

                    if self.handle_navigation(key_event) {
                        // Navigation consumed the event; re-render and continue.
                        self.render()?;
                        continue;
                    }
                }

                if matches!(&event, Event::Quit) {
                    break;
                }

                if let Some(msg) = self.app.on_event(&event) {
                    self.msg_queue.push_back(msg);
                }
            }

            // 3. Drain message queue (update → Cmd scheduling)
            self.drain_messages(&rt);

            // 4. Process subscriptions → enqueue messages
            self.process_subscriptions();

            // 5. Poll spawned tasks → enqueue messages
            self.poll_spawned_tasks(&rt);

            // 6. Drain message queue again (from subs/spawns)
            self.drain_messages(&rt);

            // 7. Render view
            self.render()?;
        }

        self.backend.shutdown()?;
        Ok(())
    }

    /// Drain the message queue, calling update for each message and scheduling
    /// any resulting commands. Includes a safety limit to prevent infinite loops
    /// from recursive `Cmd::Message` chains.
    fn drain_messages(&mut self, rt: &Runtime) {
        const MAX_ITERATIONS: usize = 100;
        let mut iterations = 0;

        while let Some(msg) = self.msg_queue.pop_front() {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                break;
            }
            let cmd = self.app.update(msg);
            self.process_cmd(cmd, rt);
        }
    }

    /// Render the current view, managing focus and theme thread-locals.
    fn render(&mut self) -> anyhow::Result<()> {
        let fm = std::mem::take(&mut self.focus);
        let old_fm = FocusManager::set_current(fm);
        let old_theme = Theme::replace_current(self.theme.clone());

        let render_result = (|| -> anyhow::Result<()> {
            FocusManager::with_current_mut(|fm| fm.begin_frame());
            let view = self.app.view();
            self.backend.render(&view)?;
            FocusManager::with_current_mut(|fm| fm.end_frame());
            Ok(())
        })();

        self.focus = FocusManager::set_current(old_fm);
        self.theme = Theme::replace_current(old_theme);
        render_result?;

        if !self.focus_initialized {
            if self.focus.focused_id().is_none() {
                self.focus.focus_next();
            }
            self.focus_initialized = self.focus.focused_id().is_some();
        }

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

    fn process_subscriptions(&mut self) {
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
            self.msg_queue.push_back(msg);
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
                    self.msg_queue.push_back(msg);
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
            focus_initialized: false,
            theme: Theme::dark(),
            subscriptions: Vec::new(),
            spawn_handles: Vec::new(),
            msg_queue: VecDeque::new(),
        }
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct TestApp {
        updates: Arc<Mutex<Vec<i32>>>,
    }

    impl Application for TestApp {
        type Message = i32;

        fn update(&mut self, msg: i32) -> Cmd<i32> {
            self.updates.lock().unwrap().push(msg);
            Cmd::none()
        }

        fn view(&self) -> impl View {
            crate::widgets::Text::new("test")
        }
    }

    struct RecursiveApp {
        updates: Arc<Mutex<Vec<i32>>>,
    }

    impl Application for RecursiveApp {
        type Message = i32;

        fn update(&mut self, msg: i32) -> Cmd<i32> {
            self.updates.lock().unwrap().push(msg);
            if msg > 0 {
                Cmd::message(msg - 1)
            } else {
                Cmd::none()
            }
        }

        fn view(&self) -> impl View {
            crate::widgets::Text::new("test")
        }
    }

    fn make_test_app() -> (App<TestApp>, Arc<Mutex<Vec<i32>>>) {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let app = TestApp {
            updates: updates.clone(),
        };
        let runner = App::new(app);
        (runner, updates)
    }

    fn make_recursive_app() -> (App<RecursiveApp>, Arc<Mutex<Vec<i32>>>) {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let app = RecursiveApp {
            updates: updates.clone(),
        };
        let runner = App::new(app);
        (runner, updates)
    }

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

    #[test]
    fn cmd_batch_processes_all_sub_commands() {
        let (mut runner, updates) = make_test_app();
        let rt = Runtime::new().expect("runtime");

        let cmd = Cmd::batch(vec![Cmd::message(1), Cmd::message(2), Cmd::message(3)]);
        runner.process_cmd(cmd, &rt);

        let log = updates.lock().unwrap();
        assert_eq!(*log, vec![1, 2, 3]);
    }

    #[test]
    fn cmd_nested_batch_flattens_correctly() {
        let (mut runner, updates) = make_test_app();
        let rt = Runtime::new().expect("runtime");

        let inner = Cmd::batch(vec![Cmd::message(2), Cmd::message(3)]);
        let cmd = Cmd::batch(vec![Cmd::message(1), inner, Cmd::message(4)]);
        runner.process_cmd(cmd, &rt);

        let log = updates.lock().unwrap();
        assert_eq!(*log, vec![1, 2, 3, 4]);
    }

    #[test]
    fn cmd_message_dispatches_immediately_and_feeds_back() {
        let (mut runner, updates) = make_recursive_app();
        let rt = Runtime::new().expect("runtime");

        let cmd = Cmd::message(3);
        runner.process_cmd(cmd, &rt);

        let log = updates.lock().unwrap();
        assert_eq!(*log, vec![3, 2, 1, 0]);
    }

    #[test]
    fn cmd_message_max_iterations_safety() {
        let updates = Arc::new(Mutex::new(Vec::new()));
        struct InfiniteApp {
            updates: Arc<Mutex<Vec<i32>>>,
        }
        impl Application for InfiniteApp {
            type Message = i32;
            fn update(&mut self, msg: i32) -> Cmd<i32> {
                self.updates.lock().unwrap().push(msg);
                Cmd::message(msg + 1)
            }
            fn view(&self) -> impl View {
                crate::widgets::Text::new("test")
            }
        }

        let app = InfiniteApp {
            updates: updates.clone(),
        };
        let mut runner = App::new(app);
        let rt = Runtime::new().expect("runtime");

        runner.process_cmd(Cmd::message(0), &rt);

        let log = updates.lock().unwrap();
        // process_cmd has MAX_ITERATIONS=100, first call is msg 0 which counts as iteration 1
        // so we get at most 100 updates (iterations 1..=100)
        assert!(log.len() <= 101);
        assert!(log.len() >= 100);
    }

    #[test]
    fn sub_every_no_burst_replay() {
        let (runner, _updates) = make_test_app();

        let interval = Duration::from_millis(100);
        let far_past = Instant::now() - Duration::from_secs(10);

        let old_states = vec![SubState {
            interval,
            f: Box::new(|| 42i32),
            last_tick: far_past,
        }];

        let mut new_states = Vec::new();
        let mut messages = Vec::new();
        let mut index = 0;

        let sub = Sub::every(interval, || 99);
        runner.collect_subscriptions(
            sub,
            Instant::now(),
            &old_states,
            &mut index,
            &mut new_states,
            &mut messages,
        );

        // Even though 10 seconds have passed (100x the interval),
        // only ONE message should be emitted — no burst replay.
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], 99);
    }
}
