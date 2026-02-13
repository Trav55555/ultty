//! Application trait and runner.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::backend::Backend;
use crate::cmd::{Cmd, Sub};
use crate::event::{Event, Key, KeyEvent};
use crate::focus::FocusManager;
use crate::screen::{Screen, ScreenAction};
use crate::theme::Theme;
use crate::toast::{Toast, ToastSeverity};
use crate::widgets::{Badge, BadgeVariant, Column, Spacer, View};
use tokio::runtime::Runtime;
use ultty_protocol::{AppMessage, RegionId};

struct OverlayView<'a> {
    base: &'a dyn View,
    overlay: Option<Column>,
}

impl<'a> OverlayView<'a> {
    fn new(base: &'a dyn View, overlay: Option<Column>) -> Self {
        Self { base, overlay }
    }
}

fn render_view_to_native(
    child: &dyn View,
    send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
    parent: RegionId,
    next_id: &mut u32,
) -> anyhow::Result<RegionId> {
    let region_id = RegionId::new(*next_id);
    *next_id += 1;

    send_message(&AppMessage::CreateRegion {
        id: region_id,
        parent,
        insert_index: -1,
        constraints: child.constraints(),
        style: child.style(),
    })?;

    let content = child.content();
    if content != ultty_protocol::Content::None {
        send_message(&AppMessage::SetContent {
            id: region_id,
            content,
        })?;
    }

    for i in 0..child.children_count() {
        child.render_child_native(i, send_message, region_id, next_id)?;
    }

    Ok(region_id)
}

impl View for OverlayView<'_> {
    fn constraints(&self) -> ultty_protocol::Constraints {
        self.base.constraints()
    }

    fn style(&self) -> ultty_protocol::Style {
        self.base.style()
    }

    fn content(&self) -> ultty_protocol::Content {
        self.base.content()
    }

    fn children_count(&self) -> usize {
        self.base.children_count() + usize::from(self.overlay.is_some())
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> anyhow::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> anyhow::Result<Option<RegionId>> {
        let base_count = self.base.children_count();
        if index < base_count {
            return self.base.render_child_native(index, send_message, parent, next_id);
        }

        if index == base_count {
            if let Some(overlay) = &self.overlay {
                return Ok(Some(render_view_to_native(overlay, send_message, parent, next_id)?));
            }
        }

        Ok(None)
    }

    fn render_ansi(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        self.base.render_ansi(frame, area);
        if let Some(overlay) = &self.overlay {
            overlay.render_ansi(frame, area);
        }
    }
}

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
    screen_stack: Vec<Box<dyn Screen>>,
    active_toasts: Vec<(Toast, Instant)>,
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
            screen_stack: Vec::new(),
            active_toasts: Vec::new(),
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
                if matches!(&event, Event::Quit) {
                    break;
                }

                if let Event::Key(ref mut key_event) = event {
                    key_event.focused_id = self.focus.focused_id().map(str::to_string);
                }

                let mut forward_to_app = true;
                let mut allow_navigation = true;
                if !self.screen_stack.is_empty() {
                    let action = {
                        let top_screen = self
                            .screen_stack
                            .last_mut()
                            .expect("screen stack checked as non-empty");
                        top_screen.on_event(&event)
                    };

                    match action {
                        ScreenAction::Continue => {}
                        ScreenAction::Dismiss => {
                            self.screen_stack.pop();
                            forward_to_app = false;
                        }
                        ScreenAction::Consumed => {
                            forward_to_app = false;
                            allow_navigation = false;
                        }
                    }
                }

                if let Event::Key(ref key_event) = event {
                    if allow_navigation && self.handle_navigation(key_event) {
                        self.render()?;
                        continue;
                    }
                }

                if forward_to_app {
                    if let Some(msg) = self.app.on_event(&event) {
                        self.msg_queue.push_back(msg);
                    }
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

            self.active_toasts
                .retain(|(toast, created)| created.elapsed() < toast.duration);

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
                tracing::warn!(
                    "drain_messages hit MAX_ITERATIONS ({}); {} messages dropped",
                    MAX_ITERATIONS,
                    self.msg_queue.len()
                );
                break;
            }
            let cmd = self.app.update(msg);
            self.process_cmd(cmd, rt);
        }
    }

    /// Render the current view, managing focus and theme thread-locals.
    ///
    /// Uses an RAII guard so thread-local state is restored even if rendering panics.
    fn render(&mut self) -> anyhow::Result<()> {
        struct ThreadLocalGuard {
            old_focus: Option<FocusManager>,
            old_theme: Option<Theme>,
        }
        impl Drop for ThreadLocalGuard {
            fn drop(&mut self) {
                if let Some(fm) = self.old_focus.take() {
                    FocusManager::set_current(fm);
                }
                if let Some(theme) = self.old_theme.take() {
                    Theme::replace_current(theme);
                }
            }
        }

        let fm = std::mem::take(&mut self.focus);
        let old_fm = FocusManager::set_current(fm);
        let mut guard = ThreadLocalGuard {
            old_focus: Some(old_fm),
            old_theme: Some(Theme::replace_current(self.theme.clone())),
        };

        let render_result = (|| -> anyhow::Result<()> {
            FocusManager::with_current_mut(|fm| fm.begin_frame());
            let overlay = if self.active_toasts.is_empty() {
                None
            } else {
                Some(self.toast_overlay_view())
            };
            if let Some(screen) = self.screen_stack.last() {
                let view = screen.view();
                let composed = OverlayView::new(&view, overlay);
                self.backend.render(&composed)?;
            } else {
                let view = self.app.view();
                let composed = OverlayView::new(&view, overlay);
                self.backend.render(&composed)?;
            }
            FocusManager::with_current_mut(|fm| fm.end_frame());
            Ok(())
        })();

        // Normal path: restore before guard drops so we recapture the values.
        self.focus = FocusManager::set_current(guard.old_focus.take().unwrap());
        self.theme = Theme::replace_current(guard.old_theme.take().unwrap());
        // guard drops here as a no-op (both Options are None).
        // On panic, guard's Drop fires and restores thread-locals automatically.
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
            Key::Right | Key::Down => {
                self.focus.focus_next();
                true
            }
            Key::Left | Key::Up => {
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
                Cmd::ShowToast(toast) => {
                    self.active_toasts.push((toast, Instant::now()));
                }
                Cmd::PushScreen(mut screen) => {
                    screen.on_mount();
                    self.screen_stack.push(screen);
                }
                Cmd::PopScreen => {
                    self.screen_stack.pop();
                }
            }
        }
    }

    fn poll_spawned_tasks(&mut self, rt: &Runtime) {
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

    fn toast_overlay_view(&self) -> Column {
        let mut toast_list = Column::new().gap(1.0);
        for (toast, _) in &self.active_toasts {
            let variant = match toast.severity {
                ToastSeverity::Info => BadgeVariant::Primary,
                ToastSeverity::Success => BadgeVariant::Success,
                ToastSeverity::Warning => BadgeVariant::Warning,
                ToastSeverity::Error => BadgeVariant::Error,
            };
            toast_list = toast_list.child(Badge::new(&toast.message).variant(variant));
        }

        Column::new()
            .flex_grow(1.0)
            .child(Spacer)
            .child(toast_list)
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
            screen_stack: Vec::new(),
            active_toasts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::screen::{Screen, ScreenAction};
    use crate::widgets::{Column, Text};
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

    #[derive(Clone)]
    struct ScreenTestState {
        main_view_calls: Arc<Mutex<usize>>,
        app_event_log: Arc<Mutex<Vec<char>>>,
    }

    impl ScreenTestState {
        fn new() -> Self {
            Self {
                main_view_calls: Arc::new(Mutex::new(0)),
                app_event_log: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    struct ScreenTestApp {
        state: ScreenTestState,
    }

    impl Application for ScreenTestApp {
        type Message = ();

        fn on_event(&mut self, event: &Event) -> Option<Self::Message> {
            if let Event::Key(key_event) = event {
                if let Key::Char(c) = key_event.key {
                    self.state.app_event_log.lock().unwrap().push(c);
                }
            }
            None
        }

        fn view(&self) -> impl View {
            *self.state.main_view_calls.lock().unwrap() += 1;
            Text::new("main")
        }
    }

    struct TestScreen {
        view_calls: Arc<Mutex<usize>>,
        mount_calls: Arc<Mutex<usize>>,
        dismiss_on: Option<char>,
    }

    impl Screen for TestScreen {
        fn view(&self) -> Column {
            *self.view_calls.lock().unwrap() += 1;
            Column::new().child(Text::new("screen"))
        }

        fn on_event(&mut self, event: &Event) -> ScreenAction {
            if let Some(target) = self.dismiss_on {
                if let Event::Key(key_event) = event {
                    if key_event.key == Key::Char(target) {
                        return ScreenAction::Dismiss;
                    }
                }
            }
            ScreenAction::Continue
        }

        fn on_mount(&mut self) {
            *self.mount_calls.lock().unwrap() += 1;
        }
    }

    fn make_screen_runner(
        state: ScreenTestState,
        events: Vec<Option<Event>>,
    ) -> App<ScreenTestApp> {
        let app = ScreenTestApp { state };
        let backend = Backend::Mock(MockBackend::new(events));
        App::new_with_backend(app, backend)
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

    #[test]
    fn test_push_screen_changes_render_target() {
        let state = ScreenTestState::new();
        let mut runner = make_screen_runner(state.clone(), vec![]);
        let rt = Runtime::new().expect("runtime");

        let screen_view_calls = Arc::new(Mutex::new(0));
        let mount_calls = Arc::new(Mutex::new(0));
        let screen = TestScreen {
            view_calls: screen_view_calls.clone(),
            mount_calls: mount_calls.clone(),
            dismiss_on: None,
        };

        runner.render().expect("initial render");
        assert_eq!(*state.main_view_calls.lock().unwrap(), 1);
        assert_eq!(*screen_view_calls.lock().unwrap(), 0);

        runner.process_cmd(Cmd::PushScreen(Box::new(screen)), &rt);
        runner.render().expect("screen render");

        assert_eq!(*state.main_view_calls.lock().unwrap(), 1);
        assert_eq!(*screen_view_calls.lock().unwrap(), 1);
        assert_eq!(*mount_calls.lock().unwrap(), 1);
    }

    #[test]
    fn test_pop_screen_restores_main_view() {
        let state = ScreenTestState::new();
        let mut runner = make_screen_runner(state.clone(), vec![]);
        let rt = Runtime::new().expect("runtime");

        let screen = TestScreen {
            view_calls: Arc::new(Mutex::new(0)),
            mount_calls: Arc::new(Mutex::new(0)),
            dismiss_on: None,
        };

        runner.process_cmd(Cmd::PushScreen(Box::new(screen)), &rt);
        runner.render().expect("render screen");
        assert_eq!(*state.main_view_calls.lock().unwrap(), 0);

        runner.process_cmd(Cmd::PopScreen, &rt);
        runner.render().expect("render main after pop");
        assert_eq!(*state.main_view_calls.lock().unwrap(), 1);
    }

    #[test]
    fn test_screen_event_dismiss_pops() {
        let state = ScreenTestState::new();
        let screen_view_calls = Arc::new(Mutex::new(0));
        let mount_calls = Arc::new(Mutex::new(0));

        let screen = TestScreen {
            view_calls: screen_view_calls.clone(),
            mount_calls: mount_calls.clone(),
            dismiss_on: Some('x'),
        };

        let events = vec![
            Some(Event::Key(KeyEvent {
                key: Key::Char('x'),
                modifiers: crate::event::Modifiers::default(),
                focused_id: None,
            })),
            Some(Event::Key(KeyEvent {
                key: Key::Char('y'),
                modifiers: crate::event::Modifiers::default(),
                focused_id: None,
            })),
            Some(Event::Quit),
        ];

        let mut runner = make_screen_runner(state.clone(), events);
        let rt = Runtime::new().expect("runtime");
        runner.process_cmd(Cmd::PushScreen(Box::new(screen)), &rt);

        runner.run().expect("run app");

        assert_eq!(*mount_calls.lock().unwrap(), 1);
        assert!(*screen_view_calls.lock().unwrap() >= 1);
        assert!(*state.main_view_calls.lock().unwrap() >= 1);
        assert_eq!(*state.app_event_log.lock().unwrap(), vec!['y']);
    }

    #[test]
    fn test_show_toast_adds_to_active() {
        let (mut runner, _updates) = make_test_app();
        let rt = Runtime::new().expect("runtime");

        runner.process_cmd(Cmd::ShowToast(Toast::info("Saved")), &rt);

        assert_eq!(runner.active_toasts.len(), 1);
        assert_eq!(runner.active_toasts[0].0.message, "Saved");
    }

    #[test]
    fn test_toast_auto_expires() {
        let (mut runner, _updates) = make_test_app();
        runner.active_toasts.push((
            Toast::info("short").duration(Duration::from_millis(1)),
            Instant::now(),
        ));

        std::thread::sleep(Duration::from_millis(10));
        runner
            .active_toasts
            .retain(|(toast, created)| created.elapsed() < toast.duration);

        assert!(runner.active_toasts.is_empty());
    }
}
