//! Application trait and runner.

mod runtime;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backend::Backend;
use crate::cmd::{Cmd, Sub};
use crate::event::{Event, Key, KeyEvent};
use crate::focus::FocusManager;
use crate::overrides::{OverrideFile, Remapped};
use crate::screen::{Screen, ScreenAction};
use crate::theme::Theme;
use crate::toast::{Toast, ToastSeverity};
use crate::wake::{self, WakeListener, Waker};
use crate::widgets::{render_view_to_native, Badge, BadgeVariant, Column, Spacer, View};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

pub use crate::wake::Proxy;
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

impl View for OverlayView<'_> {
    fn constraints(&self) -> ultty_protocol::Constraints {
        self.base.constraints()
    }

    /// The root's style, with the user's override of the root's style
    /// over it (ADR 0023).
    fn style(&self) -> ultty_protocol::Style {
        let style = self.base.style();
        match crate::overrides::root_override(self.base) {
            Some(user) => user.apply_style(style),
            None => style,
        }
    }

    fn content(&self) -> ultty_protocol::Content {
        self.base.content()
    }

    fn semantics(&self) -> ultty_protocol::Semantics {
        self.base.semantics()
    }

    fn children_count(&self) -> usize {
        self.base.children_count() + usize::from(self.overlay.is_some())
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        let base_count = self.base.children_count();
        if index < base_count {
            return self
                .base
                .render_child_native(index, send_message, parent, next_id);
        }

        if index == base_count {
            if let Some(overlay) = &self.overlay {
                return Ok(Some(render_view_to_native(
                    overlay,
                    send_message,
                    parent,
                    next_id,
                    None,
                )?));
            }
        }

        Ok(None)
    }

    /// The root, then the user's override of the root's style painted over
    /// it, then the toasts above both (ADR 0023).
    fn render_ansi(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        self.base.render_ansi(frame, area);
        if let Some(user) = crate::overrides::root_override(self.base) {
            crate::widgets::paint_override(&user, frame, area);
        }
        if let Some(overlay) = &self.overlay {
            crate::widgets::render_view_ansi(overlay, frame, area);
        }
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        self.base.for_each_child(visit);
        if let Some(overlay) = &self.overlay {
            visit(overlay);
        }
    }
}

/// Main application trait.
pub trait Application: Sized {
    /// Message type for state updates.
    type Message: Send + 'static;

    /// Called once when the app starts, before the first `view`. Its
    /// command runs like one `update` returns: start the first loads,
    /// subscriptions and so on here.
    fn init(&mut self) -> Cmd<Self::Message> {
        Cmd::none()
    }

    /// Update state based on a message.
    fn update(&mut self, _msg: Self::Message) -> Cmd<Self::Message> {
        Cmd::none()
    }

    /// Subscriptions for external events.
    fn subscriptions(&self) -> Sub<Self::Message> {
        Sub::none()
    }

    /// Handle an event and optionally return a message.
    ///
    /// Applications own interactive state. The runner annotates key events with
    /// the current `focused_id`, then forwards the event here. Rendered `View`
    /// values are treated as output and are not mutated by the runner.
    fn on_event(&mut self, _event: &Event) -> Option<Self::Message> {
        None
    }

    /// Render the UI from current application state.
    ///
    /// `view` is an immediate-mode output function: each render pass should
    /// rebuild a fresh view tree from `self` rather than depending on identity
    /// or mutation of previously returned `View` values. Persistent interactive
    /// state belongs in the application (or in app-owned widget state), while
    /// backends may retain protocol/layout caches internally between frames.
    fn view(&self) -> impl View;

    /// Run the application.
    fn run(self) -> crate::Result<()> {
        App::new(self).run()
    }

    /// Run the application with a specific theme.
    fn run_with_theme(self, theme: crate::theme::Theme) -> crate::Result<()> {
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
    /// The user's overrides for this app (ADR 0023).
    overrides: OverrideFile,
    /// `Sub::every` timer state keyed by timer identity (see `Sub::Every`).
    timers: HashMap<String, TimerState>,
    /// Spawned tasks (`Cmd::spawn*` / `Cmd::after`) that have not delivered
    /// their message yet.
    spawn_handles: Vec<SpawnedTask>,
    /// Messages of finished spawned tasks.
    task_tx: mpsc::UnboundedSender<TaskDone<A::Message>>,
    task_rx: mpsc::UnboundedReceiver<TaskDone<A::Message>>,
    cmd_subscription_handles: HashMap<String, tokio::task::JoinHandle<()>>,
    stream_subscription_handles: HashMap<String, tokio::task::JoinHandle<()>>,
    stream_subscription_urls: HashMap<String, String>,
    stream_backoff: HashMap<String, StreamBackoff>,
    /// Wakes the event loop's wait when there is work for it.
    waker: Waker,
    /// Delivers subscription items (and [`Proxy::send`]) to `subscription_rx`.
    proxy: Proxy<A::Message>,
    subscription_rx: mpsc::UnboundedReceiver<A::Message>,
    msg_queue: VecDeque<A::Message>,
    screen_stack: Vec<Box<dyn Screen>>,
    active_toasts: Vec<(Toast, Instant)>,
    quit_requested: bool,
    backend_initialized: bool,
    backend_shutdown: bool,
    needs_render: bool,
}

/// Per-timer state for `Sub::Every`.
struct TimerState {
    /// The reference tick the next firing is scheduled from.
    last_tick: Instant,
    interval: Duration,
}

impl TimerState {
    /// When the event loop must wake for this timer.
    fn deadline(&self) -> Option<Instant> {
        self.last_tick.checked_add(self.interval)
    }
}

/// The shortest `Sub::every` interval; shorter ones (zero included) tick at
/// this rate, about once a frame.
pub const MIN_TIMER_INTERVAL: Duration = Duration::from_millis(16);

/// A running `Cmd::spawn*` / `Cmd::after` task.
struct SpawnedTask {
    /// The id given to `Cmd::spawn_with_id`, for cancelling it.
    id: Option<String>,
    handle: tokio::task::JoinHandle<()>,
    /// Set when the task is cancelled, so a message it already sent is
    /// dropped instead of delivered.
    cancelled: Arc<AtomicBool>,
}

/// The message a spawned task produced.
struct TaskDone<M> {
    cancelled: Arc<AtomicBool>,
    msg: M,
}

const STREAM_BACKOFF_BASE: Duration = Duration::from_millis(250);
const STREAM_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Reconnect bookkeeping for one `Sub::stream` id.
struct StreamBackoff {
    /// Consecutive attempts that ended without a working connection.
    failures: u32,
    /// When the next attempt may start; `None` while a task is running or
    /// has finished but not yet been scheduled.
    retry_at: Option<Instant>,
    /// Set by the task once the WebSocket handshake succeeded.
    connected: Arc<AtomicBool>,
    /// Set by the task as it ends, just before it wakes the loop (whose
    /// join handle may not report the end yet).
    ended: Arc<AtomicBool>,
}

struct StreamSubSpec<M> {
    id: String,
    url: String,
    on_message: Box<dyn Fn(String) -> M + Send + 'static>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamSubscriptionAction {
    Unchanged { id: String },
    Start { id: String },
    Restart { id: String },
    Remove { id: String },
}

struct SubCollectCtx<'a, M> {
    now: Instant,
    /// Fire due timers; otherwise only record them.
    fire: bool,
    old_timers: &'a HashMap<String, TimerState>,
    new_timers: &'a mut HashMap<String, TimerState>,
    /// Ordinal counters for unkeyed timers, per interval, within one frame.
    unkeyed_timer_counts: &'a mut HashMap<Duration, usize>,
    stream_specs: &'a mut Vec<StreamSubSpec<M>>,
    messages: &'a mut Vec<M>,
}

impl<A: Application> App<A> {
    /// Create a new app runner on the backend [`Backend::detect`] picks.
    ///
    /// It reads the user's overrides for the app named by `argv[0]` (see
    /// [`App::app_name`] and ADR 0023).
    pub fn new(app: A) -> Self {
        Self::new_with_backend(app, Backend::detect())
    }

    /// Look the user's overrides up under `name` instead of the binary's
    /// name: `~/.config/ulttui/overrides/<name>.toml`, unless the
    /// environment says otherwise ([`OverrideFile::for_app`]).
    pub fn app_name(self, name: &str) -> Self {
        self.override_file(OverrideFile::for_app(name))
    }

    /// Use `file` for the user's overrides ([`OverrideFile::disabled`] for
    /// none).
    pub fn override_file(mut self, file: OverrideFile) -> Self {
        self.overrides = file;
        self.needs_render = true;
        self
    }

    /// Create a new app runner on `backend` instead of detecting one:
    /// [`Backend::ansi`] to draw with ANSI inside Ultty too, or a mock in
    /// tests.
    ///
    /// Like [`App::new`] it reads the user's overrides, except on a mock
    /// backend, so tests never see the user's files.
    pub fn new_with_backend(app: A, backend: Backend) -> Self {
        let reads_overrides = backend.reads_user_overrides();
        let mut runner = Self::with_backend(app, backend);
        if reads_overrides {
            runner.overrides = OverrideFile::for_app(&crate::overrides::default_app_name());
        }
        runner
    }

    fn with_backend(app: A, backend: Backend) -> Self {
        let (subscription_tx, subscription_rx) = mpsc::unbounded_channel();
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let waker = Waker::default();
        Self {
            app,
            backend,
            focus: FocusManager::new(),
            focus_initialized: false,
            theme: Theme::dark(),
            overrides: OverrideFile::disabled(),
            timers: HashMap::new(),
            spawn_handles: Vec::new(),
            task_tx,
            task_rx,
            cmd_subscription_handles: HashMap::new(),
            stream_subscription_handles: HashMap::new(),
            stream_subscription_urls: HashMap::new(),
            stream_backoff: HashMap::new(),
            proxy: Proxy::new(subscription_tx, waker.clone()),
            waker,
            subscription_rx,
            msg_queue: VecDeque::new(),
            screen_stack: Vec::new(),
            active_toasts: Vec::new(),
            quit_requested: false,
            backend_initialized: false,
            backend_shutdown: false,
            needs_render: true,
        }
    }

    /// A handle for sending the app messages from other threads while it
    /// runs (a file watch, a worker thread). Each send wakes the event loop.
    pub fn proxy(&self) -> Proxy<A::Message> {
        self.proxy.clone()
    }

    pub fn new_with_theme(app: A, theme: Theme) -> Self {
        let mut runner = Self::new(app);
        runner.theme = theme;
        runner
    }

    /// Run the application event loop.
    ///
    /// `Application::init` runs once, before the first frame. Then each pass
    /// of the loop (Bubble Tea parity):
    /// 1. Poll backend for events
    /// 2. Normalize events → messages via on_event()
    /// 3. Drain message queue (update → Cmd scheduling)
    /// 4. Process subscriptions → enqueue messages
    /// 5. Poll spawned tasks → enqueue messages
    /// 6. Drain message queue again (from subs/spawns)
    /// 7. Render view
    ///
    /// Between passes the loop sleeps until there is something to do: input
    /// from the terminal, a timer, toast or reconnect deadline, a finished
    /// command, a subscription item or a [`Proxy::send`]. An idle app does
    /// not wake at all.
    pub fn run(mut self) -> crate::Result<()> {
        let rt = Runtime::new()?;
        let wake = self.waker.listen()?;
        self.backend.init()?;
        self.backend_initialized = true;
        if matches!(self.backend, Backend::Ansi(_)) {
            // The terminal reports a resize only as SIGWINCH, which need not
            // interrupt this thread's wait.
            Self::wake_on_resize(&rt, self.waker.clone());
        }

        let init = self.app.init();
        self.process_cmd(init, &rt);

        if !self.quit_requested {
            // Initial render so UI appears immediately before first poll.
            self.render()?;
            while self.run_tick(&rt)? {
                self.wait_for_work(&wake)?;
            }
        }

        self.cleanup_runtime_resources();
        self.shutdown_backend()?;
        Ok(())
    }

    fn wake_on_resize(rt: &Runtime, waker: Waker) {
        rt.spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut resizes = match signal(SignalKind::window_change()) {
                Ok(resizes) => resizes,
                Err(err) => {
                    tracing::warn!("cannot watch for terminal resizes: {err}");
                    return;
                }
            };
            while resizes.recv().await.is_some() {
                waker.wake();
            }
        });
    }

    /// Sleep until there may be work: terminal input, a wake, or the next
    /// deadline. Returns at once when work is already waiting.
    fn wait_for_work(&mut self, wake: &WakeListener) -> crate::Result<()> {
        if !self.msg_queue.is_empty()
            || self.waker.take_pending()
            || self.backend.has_pending_events()?
        {
            return Ok(());
        }
        let timeout = self
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let mut fds = vec![wake.fd()];
        fds.extend(self.backend.wait_fd());
        wake::poll_readable(&fds, timeout)?;
        wake.drain();
        // Whatever woke us is handled by the next pass.
        self.waker.take_pending();
        Ok(())
    }

    /// The earliest time the loop has to run without being woken: a timer, a
    /// toast expiring, a stream reconnect, or the backend's own deadline.
    fn next_deadline(&self) -> Option<Instant> {
        let timers = self.timers.values().filter_map(TimerState::deadline);
        let toasts = self
            .active_toasts
            .iter()
            .filter_map(|(toast, created)| created.checked_add(toast.duration));
        let retries = self.stream_backoff.values().filter_map(|b| b.retry_at);
        timers
            .chain(toasts)
            .chain(retries)
            .chain(self.backend.deadline())
            .min()
    }

    /// Run one pass of the application event loop.
    ///
    /// Returns false when the runner should stop.
    fn run_tick(&mut self, rt: &Runtime) -> crate::Result<bool> {
        // 1. Poll backend for events
        if let Some(event) = self
            .backend
            .poll_event()?
            .and_then(|event| self.prepare_event(event))
        {
            if matches!(&event, Event::Quit) {
                if let Some(msg) = self.app.on_event(&event) {
                    self.msg_queue.push_back(msg);
                }
                self.drain_messages(rt);
                self.poll_spawned_tasks(rt);
                self.poll_subscriptions();
                self.drain_messages(rt);
                return Ok(false);
            }

            let mut forward_to_app = true;
            let mut allow_navigation = true;
            let mut event_changed_state = false;
            if !self.screen_stack.is_empty() {
                let action = {
                    let top_screen = self
                        .screen_stack
                        .last_mut()
                        .expect("screen stack checked as non-empty");
                    top_screen.on_event(&event)
                };
                event_changed_state = true;

                match action {
                    ScreenAction::Continue => {}
                    ScreenAction::Dismiss => {
                        if let Some(mut screen) = self.screen_stack.pop() {
                            screen.on_unmount();
                        }
                        forward_to_app = false;
                        event_changed_state = true;
                    }
                    ScreenAction::Consumed => {
                        forward_to_app = false;
                        allow_navigation = false;
                    }
                }
            }

            // The app (and its focused widget) sees every key first; Tab only
            // drives focus navigation when the app leaves it unhandled.
            let mut handled_by_app = false;
            if forward_to_app {
                event_changed_state = true;
                if let Some(msg) = self.app.on_event(&event) {
                    self.msg_queue.push_back(msg);
                    handled_by_app = true;
                }
            }

            if let Event::Key(ref key_event) = event {
                if allow_navigation && !handled_by_app && self.handle_navigation(key_event) {
                    event_changed_state = true;
                }
            }

            if event_changed_state {
                self.needs_render = true;
            }
        }

        // 3. Drain message queue (update → Cmd scheduling)
        if self.drain_messages(rt) {
            self.needs_render = true;
        }

        // 4. Process subscriptions → enqueue messages
        if self.process_subscriptions(rt) {
            self.needs_render = true;
        }

        // 5. Poll spawned tasks → enqueue messages
        if self.poll_spawned_tasks(rt) {
            self.needs_render = true;
        }

        // 5b. Poll subscription streams → enqueue messages
        if self.poll_subscriptions() {
            self.needs_render = true;
        }

        // 6. Drain message queue again (from subs/spawns)
        if self.drain_messages(rt) {
            self.needs_render = true;
            // Those messages may have changed the subscriptions; register
            // new timers now so the wait knows their deadlines. Due timers
            // fire on the next pass.
            self.refresh_subscriptions(rt, false);
        }

        let old_toast_count = self.active_toasts.len();
        self.active_toasts
            .retain(|(toast, created)| created.elapsed() < toast.duration);
        if self.active_toasts.len() != old_toast_count {
            self.needs_render = true;
        }

        // 7. Render view
        if self.needs_render {
            self.render()?;
            self.needs_render = false;
        }

        Ok(!self.quit_requested)
    }

    /// Note the focused widget on a key event and apply the user's key
    /// remaps. `None` when a remap moved focus and used up the key.
    fn prepare_event(&mut self, mut event: Event) -> Option<Event> {
        let Event::Key(key_event) = &mut event else {
            return Some(event);
        };
        key_event.focused_id = self.focus.focused_id().map(str::to_string);
        let typing = self.focus.focused_accepts_text();
        match self.overrides.overrides().remap(key_event, typing) {
            None => Some(event),
            Some(Remapped::Key(remapped)) => Some(Event::Key(remapped)),
            Some(Remapped::Focus(id)) => {
                if !self.focus.focusable_ids().contains(&id) {
                    tracing::debug!(id, "override focus target is not focusable; key passed on");
                    return Some(event);
                }
                self.focus.focus(id);
                self.needs_render = true;
                None
            }
        }
    }

    fn cleanup_runtime_resources(&mut self) {
        for (_, handle) in self.cmd_subscription_handles.drain() {
            handle.abort();
        }
        for (_, handle) in self.stream_subscription_handles.drain() {
            handle.abort();
        }
        for task in self.spawn_handles.drain(..) {
            task.handle.abort();
        }
    }

    fn shutdown_backend(&mut self) -> crate::Result<()> {
        if self.backend_initialized && !self.backend_shutdown {
            self.backend.shutdown()?;
            self.backend_shutdown = true;
        }
        Ok(())
    }

    /// Drain the message queue, calling update for each message and scheduling
    /// any resulting commands.
    ///
    /// At most `MAX_ITERATIONS` messages are processed per call so one tick
    /// cannot starve rendering; the rest stay queued for the next drain. If the
    /// queue itself grows past `MAX_QUEUE_SIZE`, the oldest messages are
    /// dropped so the newest state-changing messages survive.
    fn drain_messages(&mut self, rt: &Runtime) -> bool {
        const MAX_ITERATIONS: usize = 100;
        const MAX_QUEUE_SIZE: usize = 10_000;

        if self.msg_queue.len() > MAX_QUEUE_SIZE {
            let excess = self.msg_queue.len() - MAX_QUEUE_SIZE;
            tracing::error!(
                "msg_queue has {} entries (max {}); dropping {} oldest",
                self.msg_queue.len(),
                MAX_QUEUE_SIZE,
                excess
            );
            self.msg_queue.drain(..excess);
        }

        let mut processed_any = false;
        for _ in 0..MAX_ITERATIONS {
            let Some(msg) = self.msg_queue.pop_front() else {
                break;
            };
            processed_any = true;
            let cmd = self.app.update(msg);
            self.process_cmd(cmd, rt);
        }
        if !self.msg_queue.is_empty() {
            tracing::warn!(
                "drain_messages processed {} messages; {} deferred to next tick",
                MAX_ITERATIONS,
                self.msg_queue.len()
            );
        }
        processed_any
    }

    /// Render the current view, managing focus and theme thread-locals.
    ///
    /// Uses an RAII guard so thread-local state is restored even if rendering panics.
    fn render(&mut self) -> crate::Result<()> {
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

        self.overrides.refresh();
        let overrides = self.overrides.overrides().clone();
        let render_result = crate::overrides::with_overrides(overrides, || -> crate::Result<()> {
            FocusManager::with_current_mut(|fm| fm.begin_frame());
            let overlay = if self.active_toasts.is_empty() {
                None
            } else {
                Some(self.toast_overlay_view())
            };
            if let Some(screen) = self.screen_stack.last() {
                let view = screen.view();
                let composed = OverlayView::new(&view, overlay);
                FocusManager::with_current_mut(|fm| fm.register_view_tree(&composed));
                self.backend.render(&composed)?;
            } else {
                let view = self.app.view();
                let composed = OverlayView::new(&view, overlay);
                FocusManager::with_current_mut(|fm| fm.register_view_tree(&composed));
                self.backend.render(&composed)?;
            }
            FocusManager::with_current_mut(|fm| fm.end_frame());
            Ok(())
        });

        // Normal path: restore before guard drops so we recapture the values.
        self.focus = FocusManager::set_current(guard.old_focus.take().unwrap_or_default());
        self.theme = Theme::replace_current(guard.old_theme.take().unwrap_or_else(Theme::dark));
        // guard drops here as a no-op (both Options are None).
        // On panic, guard's Drop fires and restores thread-locals automatically.
        render_result?;

        if !self.focus_initialized {
            if self.focus.focused_id().is_none() {
                self.focus.focus_next();
            }
            self.focus_initialized = self.focus.focused_id().is_some();
        }
        self.backend.report_focus(self.focus.focused_id())?;

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

    fn process_subscriptions(&mut self, rt: &Runtime) -> bool {
        self.refresh_subscriptions(rt, true)
    }

    /// Re-read the app's subscriptions: keep timer phases, start and stop
    /// streams. With `fire`, timers that are due fire (and their messages
    /// are queued); without, they stay due.
    fn refresh_subscriptions(&mut self, rt: &Runtime, fire: bool) -> bool {
        let now = Instant::now();
        let old_timers = std::mem::take(&mut self.timers);
        let mut new_timers = HashMap::new();
        let mut unkeyed_timer_counts = HashMap::new();
        let mut stream_specs = Vec::new();
        let mut messages = Vec::new();

        let mut ctx = SubCollectCtx {
            now,
            fire,
            old_timers: &old_timers,
            new_timers: &mut new_timers,
            unkeyed_timer_counts: &mut unkeyed_timer_counts,
            stream_specs: &mut stream_specs,
            messages: &mut messages,
        };

        collect_subscriptions(self.app.subscriptions(), &mut ctx);

        self.timers = new_timers;
        self.reconcile_stream_subscriptions(rt, stream_specs, now);

        let produced_messages = !messages.is_empty();
        for msg in messages {
            self.msg_queue.push_back(msg);
        }
        produced_messages
    }

    fn reconcile_stream_subscriptions(
        &mut self,
        rt: &Runtime,
        stream_specs: Vec<StreamSubSpec<A::Message>>,
        now: Instant,
    ) {
        let mut desired_by_id: HashMap<String, StreamSubSpec<A::Message>> = HashMap::new();
        for spec in stream_specs {
            desired_by_id.insert(spec.id.clone(), spec);
        }

        let active_urls = self.stream_subscription_urls.clone();
        let running_by_id = self
            .stream_subscription_handles
            .iter()
            .map(|(id, handle)| {
                let ended = self
                    .stream_backoff
                    .get(id)
                    .is_some_and(|backoff| backoff.ended.load(Ordering::SeqCst));
                (id.clone(), !ended && !handle.is_finished())
            })
            .collect::<HashMap<_, _>>();
        let desired_urls = desired_by_id
            .iter()
            .map(|(id, spec)| (id.clone(), spec.url.clone()))
            .collect::<HashMap<_, _>>();

        for action in stream_subscription_actions(&active_urls, &running_by_id, &desired_urls) {
            match action {
                StreamSubscriptionAction::Unchanged { .. } => {}
                StreamSubscriptionAction::Remove { id } => {
                    if let Some(handle) = self.stream_subscription_handles.remove(&id) {
                        handle.abort();
                    }
                    self.stream_subscription_urls.remove(&id);
                    self.stream_backoff.remove(&id);
                }
                StreamSubscriptionAction::Start { id } => {
                    self.stream_backoff.remove(&id);
                    let spec = desired_by_id
                        .remove(&id)
                        .expect("start action should have a desired spec");
                    self.start_stream_subscription(rt, spec, 0);
                }
                StreamSubscriptionAction::Restart { id } => {
                    let url_changed = active_urls.get(&id) != desired_urls.get(&id);
                    if url_changed {
                        self.stream_backoff.remove(&id);
                    }
                    let failures = match self.stream_backoff.get_mut(&id) {
                        Some(backoff) if backoff.retry_at.is_none() => {
                            // The task just ended. A connection that had been
                            // established counts as a fresh failure sequence.
                            if backoff.connected.load(Ordering::SeqCst) {
                                backoff.failures = 0;
                            }
                            backoff.failures += 1;
                            let delay = stream_backoff_delay(backoff.failures, jitter_unit());
                            backoff.retry_at = Some(now + delay);
                            tracing::warn!(
                                "stream subscription {} ended; reconnecting in {:?} (attempt {})",
                                id,
                                delay,
                                backoff.failures
                            );
                            continue;
                        }
                        Some(backoff) if backoff.retry_at.is_some_and(|at| now < at) => continue,
                        Some(backoff) => backoff.failures,
                        None => 0,
                    };
                    let spec = desired_by_id
                        .remove(&id)
                        .expect("restart action should have a desired spec");
                    self.start_stream_subscription(rt, spec, failures);
                }
            }
        }
    }

    fn start_stream_subscription(
        &mut self,
        rt: &Runtime,
        spec: StreamSubSpec<A::Message>,
        failures: u32,
    ) {
        if let Some(handle) = self.stream_subscription_handles.remove(&spec.id) {
            handle.abort();
        }
        let connected = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let task = stream_subscription_task(
            self.proxy.clone(),
            spec.id.clone(),
            spec.url.clone(),
            spec.on_message,
            connected.clone(),
        );
        let (waker, task_ended) = (self.waker.clone(), ended.clone());
        let handle = rt.spawn(async move {
            task.await;
            // The loop reconnects (after a backoff) once it sees the end.
            task_ended.store(true, Ordering::SeqCst);
            waker.wake();
        });

        self.stream_subscription_handles
            .insert(spec.id.clone(), handle);
        self.stream_subscription_urls
            .insert(spec.id.clone(), spec.url);
        self.stream_backoff.insert(
            spec.id,
            StreamBackoff {
                failures,
                retry_at: None,
                connected,
                ended,
            },
        );
    }

    /// Poll subscription channels for incoming messages.
    fn poll_subscriptions(&mut self) -> bool {
        let mut received_any = false;
        while let Ok(msg) = self.subscription_rx.try_recv() {
            self.msg_queue.push_back(msg);
            received_any = true;
        }
        received_any
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

        Column::new().flex_grow(1.0).child(Spacer).child(toast_list)
    }
}

/// Forward a WebSocket's messages to the app until it closes.
async fn stream_subscription_task<M: Send + 'static>(
    tx: Proxy<M>,
    id: String,
    url: String,
    on_message: Box<dyn Fn(String) -> M + Send + 'static>,
    connected: Arc<AtomicBool>,
) {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let ws = tokio_tungstenite::connect_async(url.as_str()).await;
    let (mut socket, _) = match ws {
        Ok(parts) => parts,
        Err(err) => {
            tracing::warn!("stream subscription {} connect failed: {}", id, err);
            return;
        }
    };
    connected.store(true, Ordering::SeqCst);

    while let Some(item) = socket.next().await {
        match item {
            Ok(WsMessage::Text(text)) => {
                if tx.send(on_message(text.to_string())).is_err() {
                    break;
                }
            }
            Ok(WsMessage::Binary(bytes)) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => {
                    if tx.send(on_message(text)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!("stream subscription {} dropped non-utf8 frame: {}", id, err);
                }
            },
            Ok(WsMessage::Close(_)) => break,
            Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => {}
            Ok(WsMessage::Frame(_)) => {}
            Err(err) => {
                tracing::warn!("stream subscription {} recv failed: {}", id, err);
                break;
            }
        }
    }
}

fn collect_subscriptions<M>(sub: Sub<M>, ctx: &mut SubCollectCtx<'_, M>) {
    match sub {
        Sub::None => {}
        Sub::Every { id, interval, f } => {
            // Identity uses the interval asked for, the schedule the clamped one.
            let asked = interval;
            let interval = interval.max(MIN_TIMER_INTERVAL);
            let key = match id {
                Some(id) => id,
                None => unkeyed_timer_key(asked, ctx.unkeyed_timer_counts),
            };
            if ctx.new_timers.contains_key(&key) {
                tracing::warn!(
                    "duplicate Sub::every id {key:?} in one frame; ignoring the later one"
                );
                return;
            }
            let last_tick = ctx
                .old_timers
                .get(&key)
                .map(|state| state.last_tick)
                .unwrap_or(ctx.now);
            let mut next_tick = last_tick;
            if ctx.fire {
                let (fired, next) = advance_timer(last_tick, interval, ctx.now);
                if fired {
                    ctx.messages.push(f());
                }
                next_tick = next;
            }
            ctx.new_timers.insert(
                key,
                TimerState {
                    last_tick: next_tick,
                    interval,
                },
            );
        }
        Sub::Stream {
            id,
            url,
            on_message,
        } => {
            ctx.stream_specs.push(StreamSubSpec {
                id,
                url,
                on_message,
            });
        }
        Sub::Batch(subs) => {
            for sub in subs {
                collect_subscriptions(sub, ctx);
            }
        }
    }
}

/// Identity for a `Sub::every` without an explicit id: the interval plus the
/// timer's ordinal among unkeyed timers with that same interval this frame.
fn unkeyed_timer_key(interval: Duration, counts: &mut HashMap<Duration, usize>) -> String {
    let ordinal = counts.entry(interval).or_insert(0);
    let key = format!("every:{}ns#{}", interval.as_nanos(), *ordinal);
    *ordinal += 1;
    key
}

/// Decide whether a timer fires at `now` and compute its next reference tick.
///
/// The next tick is scheduled from the previous tick rather than from `now`,
/// so polling latency does not accumulate as drift. If several periods were
/// missed the timer catches up to the latest missed tick and fires once; it
/// never bursts once per missed period.
fn advance_timer(last_tick: Instant, interval: Duration, now: Instant) -> (bool, Instant) {
    if interval.is_zero() {
        return (true, now);
    }
    let elapsed = now.saturating_duration_since(last_tick);
    if elapsed < interval {
        return (false, last_tick);
    }
    let periods = u32::try_from(elapsed.as_nanos() / interval.as_nanos()).unwrap_or(u32::MAX);
    let next_tick = last_tick
        .checked_add(interval.saturating_mul(periods))
        .unwrap_or(now);
    (true, next_tick)
}

/// Delay before the `failures`-th consecutive reconnect attempt of a stream
/// subscription: 250 ms doubling per failure, plus up to 25% jitter
/// (`jitter_unit` in `0.0..=1.0`), capped at 30 s.
fn stream_backoff_delay(failures: u32, jitter_unit: f64) -> Duration {
    let exponent = failures.saturating_sub(1).min(20);
    let raw = STREAM_BACKOFF_BASE.saturating_mul(1u32 << exponent);
    let jitter = raw.mul_f64(0.25 * jitter_unit.clamp(0.0, 1.0));
    raw.saturating_add(jitter).min(STREAM_BACKOFF_MAX)
}

/// Cheap uniform-ish value in `0.0..1.0` for backoff jitter, derived from the
/// std hasher's per-instance random keys so no RNG dependency is needed.
fn jitter_unit() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let hash = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (hash % 1_000) as f64 / 1_000.0
}

fn stream_subscription_actions(
    active_urls: &HashMap<String, String>,
    running_by_id: &HashMap<String, bool>,
    desired_urls: &HashMap<String, String>,
) -> Vec<StreamSubscriptionAction> {
    let mut ids = BTreeSet::new();
    ids.extend(active_urls.keys().cloned());
    ids.extend(desired_urls.keys().cloned());

    ids.into_iter()
        .map(|id| match (active_urls.get(&id), desired_urls.get(&id)) {
            (Some(_), None) => StreamSubscriptionAction::Remove { id },
            (None, Some(_)) => StreamSubscriptionAction::Start { id },
            (Some(active_url), Some(desired_url))
                if active_url == desired_url
                    && running_by_id.get(&id).copied().unwrap_or(false) =>
            {
                StreamSubscriptionAction::Unchanged { id }
            }
            (Some(_), Some(_)) => StreamSubscriptionAction::Restart { id },
            (None, None) => unreachable!("id comes from active or desired stream maps"),
        })
        .collect()
}

impl<A: Application> Drop for App<A> {
    fn drop(&mut self) {
        self.cleanup_runtime_resources();
        if self.backend_initialized && !self.backend_shutdown {
            if let Err(err) = self.backend.shutdown() {
                tracing::warn!("backend shutdown during drop failed: {err}");
            }
            self.backend_shutdown = true;
        }
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::screen::{Screen, ScreenAction};
    use crate::widgets::{Column, Text};
    use std::sync::atomic::{AtomicBool, Ordering};
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

    struct TickApp {
        updates: Arc<Mutex<Vec<char>>>,
        view_calls: Arc<Mutex<usize>>,
    }

    impl Application for TickApp {
        type Message = char;

        fn update(&mut self, msg: char) -> Cmd<char> {
            self.updates.lock().unwrap().push(msg);
            Cmd::none()
        }

        fn on_event(&mut self, event: &Event) -> Option<Self::Message> {
            match event {
                Event::Key(key_event) => match key_event.key {
                    Key::Char(c) => Some(c),
                    _ => None,
                },
                Event::Quit => Some('q'),
                _ => None,
            }
        }

        fn view(&self) -> impl View {
            *self.view_calls.lock().unwrap() += 1;
            Text::new("tick")
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

    fn test_runtime() -> Runtime {
        Runtime::new().expect("runtime")
    }

    fn mock_render_count<A: Application>(runner: &App<A>) -> usize {
        match &runner.backend {
            Backend::Mock(backend) => backend.render_count(),
            _ => 0,
        }
    }

    #[test]
    fn runtime_tick_processes_event_update_and_render() {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let view_calls = Arc::new(Mutex::new(0));
        let app = TickApp {
            updates: updates.clone(),
            view_calls: view_calls.clone(),
        };
        let backend = Backend::Mock(MockBackend::new(vec![Some(Event::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        }))]));
        let mut runner = App::new_with_backend(app, backend);
        let rt = test_runtime();

        let should_continue = runner.run_tick(&rt).expect("tick succeeds");

        assert!(should_continue);
        assert_eq!(*updates.lock().unwrap(), vec!['a']);
        assert_eq!(*view_calls.lock().unwrap(), 1);
        assert_eq!(mock_render_count(&runner), 1);
    }

    #[test]
    fn runtime_tick_forwards_arrow_keys_to_app() {
        let updates = Arc::new(Mutex::new(Vec::new()));

        struct ArrowApp {
            updates: Arc<Mutex<Vec<Key>>>,
        }

        impl Application for ArrowApp {
            type Message = Key;

            fn update(&mut self, msg: Key) -> Cmd<Key> {
                self.updates.lock().unwrap().push(msg);
                Cmd::none()
            }

            fn on_event(&mut self, event: &Event) -> Option<Self::Message> {
                match event {
                    Event::Key(key_event) => Some(key_event.key.clone()),
                    _ => None,
                }
            }

            fn view(&self) -> impl View {
                Text::new("arrows")
            }
        }

        let backend = Backend::Mock(MockBackend::new(vec![Some(Event::Key(KeyEvent {
            key: Key::Down,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        }))]));
        let app = ArrowApp {
            updates: updates.clone(),
        };
        let mut runner = App::new_with_backend(app, backend);
        runner.needs_render = false;
        let rt = test_runtime();

        let should_continue = runner.run_tick(&rt).expect("tick succeeds");

        assert!(should_continue);
        assert_eq!(*updates.lock().unwrap(), vec![Key::Down]);
        assert_eq!(mock_render_count(&runner), 1);
    }

    #[test]
    fn runtime_tick_without_changes_does_not_render() {
        let app = TickApp {
            updates: Arc::new(Mutex::new(Vec::new())),
            view_calls: Arc::new(Mutex::new(0)),
        };
        let backend = Backend::Mock(MockBackend::new(vec![None]));
        let mut runner = App::new_with_backend(app, backend);
        runner.needs_render = false;
        let rt = test_runtime();

        let should_continue = runner.run_tick(&rt).expect("tick succeeds");

        assert!(should_continue);
        assert_eq!(mock_render_count(&runner), 0);
    }

    #[test]
    fn runtime_tick_handles_quit_without_rendering() {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let view_calls = Arc::new(Mutex::new(0));
        let app = TickApp {
            updates: updates.clone(),
            view_calls: view_calls.clone(),
        };
        let backend = Backend::Mock(MockBackend::new(vec![Some(Event::Quit)]));
        let mut runner = App::new_with_backend(app, backend);
        let rt = test_runtime();

        let should_continue = runner.run_tick(&rt).expect("tick succeeds");

        assert!(!should_continue);
        assert_eq!(*updates.lock().unwrap(), vec!['q']);
        assert_eq!(*view_calls.lock().unwrap(), 0);
        assert_eq!(mock_render_count(&runner), 0);
    }

    #[test]
    fn cmd_batch_processes_all_sub_commands() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();

        let cmd = Cmd::batch(vec![Cmd::message(1), Cmd::message(2), Cmd::message(3)]);
        runner.process_cmd(cmd, &rt);

        let log = updates.lock().unwrap();
        assert_eq!(*log, vec![1, 2, 3]);
    }

    #[test]
    fn cmd_nested_batch_flattens_correctly() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();

        let inner = Cmd::batch(vec![Cmd::message(2), Cmd::message(3)]);
        let cmd = Cmd::batch(vec![Cmd::message(1), inner, Cmd::message(4)]);
        runner.process_cmd(cmd, &rt);

        let log = updates.lock().unwrap();
        assert_eq!(*log, vec![1, 2, 3, 4]);
    }

    #[test]
    fn cmd_message_dispatches_immediately_and_feeds_back() {
        let (mut runner, updates) = make_recursive_app();
        let rt = test_runtime();

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
        let rt = test_runtime();

        runner.process_cmd(Cmd::message(0), &rt);

        // The chain is cut at depth 100 and the overflow message is deferred,
        // not dropped: it is still queued for the next drain.
        assert_eq!(updates.lock().unwrap().len(), 100);
        assert_eq!(runner.msg_queue.len(), 1);
        assert_eq!(runner.msg_queue.front(), Some(&100));
    }

    #[test]
    fn cmd_batch_of_many_messages_is_fully_delivered_then_quits() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();

        let mut cmds: Vec<Cmd<i32>> = (0..150).map(Cmd::message).collect();
        cmds.push(Cmd::Quit);
        runner.process_cmd(Cmd::batch(cmds), &rt);

        let log = updates.lock().unwrap();
        assert_eq!(log.len(), 150);
        assert_eq!(*log, (0..150).collect::<Vec<_>>());
        assert!(runner.quit_requested);
        assert!(runner.msg_queue.is_empty());
    }

    #[test]
    fn drain_messages_defers_overflow_instead_of_dropping() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();
        runner.msg_queue.extend(0..150);

        assert!(runner.drain_messages(&rt));
        assert_eq!(updates.lock().unwrap().len(), 100);
        assert_eq!(runner.msg_queue.len(), 50);

        assert!(runner.drain_messages(&rt));
        assert_eq!(*updates.lock().unwrap(), (0..150).collect::<Vec<_>>());
        assert!(runner.msg_queue.is_empty());
    }

    #[test]
    fn drain_messages_drops_oldest_when_queue_overflows() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();
        runner.msg_queue.extend(0..10_005);

        runner.drain_messages(&rt);

        // The five oldest (0..5) were dropped; processing starts at 5.
        assert_eq!(updates.lock().unwrap()[0], 5);
        assert_eq!(runner.msg_queue.len(), 10_000 - 100);
    }

    #[test]
    fn cmd_spawn_with_id_can_be_cancelled_and_is_pruned() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();

        runner.process_cmd(
            Cmd::spawn_with_id("slow", async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                1
            }),
            &rt,
        );
        assert_eq!(runner.spawn_handles.len(), 1);

        runner.process_cmd(Cmd::cancel("slow"), &rt);
        assert!(runner.spawn_handles.is_empty());
        assert!(!runner.poll_spawned_tasks(&rt));
        assert!(updates.lock().unwrap().is_empty());
    }

    #[test]
    fn cmd_spawn_with_same_id_replaces_previous_task() {
        let (mut runner, updates) = make_test_app();
        let rt = test_runtime();

        runner.process_cmd(
            Cmd::spawn_with_id("job", async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                1
            }),
            &rt,
        );
        runner.process_cmd(Cmd::spawn_with_id("job", async { 2 }), &rt);
        assert_eq!(runner.spawn_handles.len(), 1);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !runner.poll_spawned_tasks(&rt) {
            assert!(Instant::now() < deadline, "replacement task never finished");
            std::thread::yield_now();
        }
        runner.drain_messages(&rt);
        assert_eq!(*updates.lock().unwrap(), vec![2]);
        assert!(runner.spawn_handles.is_empty());
    }

    #[test]
    fn advance_timer_does_not_fire_before_interval() {
        let now = Instant::now();
        let last = now - Duration::from_millis(40);
        let (fired, next) = advance_timer(last, Duration::from_millis(100), now);
        assert!(!fired);
        assert_eq!(next, last);
    }

    #[test]
    fn advance_timer_schedules_from_last_tick_not_now() {
        let now = Instant::now();
        let interval = Duration::from_millis(100);
        let last = now - Duration::from_millis(130);
        let (fired, next) = advance_timer(last, interval, now);
        assert!(fired);
        // Next reference tick is last + 1 period, so the 30 ms of polling
        // latency is not added to the period.
        assert_eq!(next, last + interval);
    }

    #[test]
    fn advance_timer_catches_up_without_bursting() {
        let now = Instant::now();
        let interval = Duration::from_millis(100);
        let far_past = now - Duration::from_secs(10);
        let (fired, next) = advance_timer(far_past, interval, now);
        assert!(fired);
        // 100 periods were missed: skip to the latest one, fire exactly once.
        assert_eq!(next, far_past + interval * 100);
        assert!(next <= now);
        assert!(now - next < interval);
    }

    struct TogglingTimerApp {
        poll_enabled: Arc<AtomicBool>,
    }

    impl Application for TogglingTimerApp {
        type Message = &'static str;

        fn subscriptions(&self) -> Sub<Self::Message> {
            let poll = if self.poll_enabled.load(Ordering::SeqCst) {
                Sub::every(Duration::from_millis(100), || "poll")
            } else {
                Sub::none()
            };
            Sub::batch(vec![
                poll,
                Sub::every(Duration::from_secs(60), || "refresh"),
            ])
        }

        fn view(&self) -> impl View {
            Text::new("timers")
        }
    }

    #[test]
    fn toggling_conditional_timer_does_not_reset_sibling_timer() {
        let poll_enabled = Arc::new(AtomicBool::new(true));
        let mut runner = App::new(TogglingTimerApp {
            poll_enabled: poll_enabled.clone(),
        });
        let rt = test_runtime();

        runner.process_subscriptions(&rt);
        let refresh_key = "every:60000000000ns#0";
        assert!(runner.timers.contains_key(refresh_key));
        assert!(runner.timers.contains_key("every:100000000ns#0"));

        // Pretend the refresh timer has been running for 59 s.
        let almost_due = Instant::now() - Duration::from_secs(59);
        runner.timers.get_mut(refresh_key).unwrap().last_tick = almost_due;

        poll_enabled.store(false, Ordering::SeqCst);
        runner.process_subscriptions(&rt);

        assert_eq!(runner.timers.len(), 1);
        assert_eq!(runner.timers[refresh_key].last_tick, almost_due);
        assert!(runner.msg_queue.is_empty());
    }

    #[test]
    fn sub_every_with_id_keeps_identity_when_position_changes() {
        let (mut runner, _updates) = make_test_app();
        let rt = test_runtime();
        runner.timers.insert(
            "poll".to_string(),
            TimerState {
                last_tick: Instant::now() - Duration::from_secs(1),
                interval: Duration::from_secs(60),
            },
        );
        let old_tick = runner.timers["poll"].last_tick;

        let mut ctx_timers = HashMap::new();
        let mut counts = HashMap::new();
        let mut streams = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = SubCollectCtx {
            now: Instant::now(),
            fire: true,
            old_timers: &runner.timers,
            new_timers: &mut ctx_timers,
            unkeyed_timer_counts: &mut counts,
            stream_specs: &mut streams,
            messages: &mut messages,
        };
        collect_subscriptions(
            Sub::batch(vec![
                Sub::every(Duration::from_secs(5), || 1),
                Sub::every_with_id("poll", Duration::from_secs(60), || 2),
            ]),
            &mut ctx,
        );

        assert_eq!(ctx_timers["poll"].last_tick, old_tick);
        assert!(messages.is_empty());
        drop(rt);
    }

    #[test]
    fn stream_backoff_delay_doubles_with_jitter_and_caps() {
        let no_jitter: Vec<u128> = (1..=8)
            .map(|f| stream_backoff_delay(f, 0.0).as_millis())
            .collect();
        assert_eq!(
            no_jitter,
            vec![250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000]
        );
        assert_eq!(stream_backoff_delay(0, 0.0).as_millis(), 250);
        assert_eq!(
            stream_backoff_delay(1, 1.0),
            Duration::from_millis(250) + Duration::from_micros(62_500)
        );
        assert_eq!(stream_backoff_delay(40, 1.0), STREAM_BACKOFF_MAX);
    }

    #[test]
    fn stream_reconnect_waits_for_backoff_before_restarting() {
        let app = StreamSubApp {
            enabled: Arc::new(AtomicBool::new(true)),
            url: Arc::new(Mutex::new("ws://127.0.0.1:1".to_string())),
        };
        let mut runner = App::new(app);
        let rt = test_runtime();

        runner.process_subscriptions(&rt);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runner.stream_subscription_handles["ws-sub"].is_finished() {
            assert!(Instant::now() < deadline, "connect to a closed port hung");
            std::thread::sleep(Duration::from_millis(5));
        }

        // First tick after the failure schedules the retry instead of respawning.
        runner.process_subscriptions(&rt);
        assert_eq!(runner.stream_backoff["ws-sub"].failures, 1);
        assert!(runner.stream_backoff["ws-sub"].retry_at.is_some());
        assert!(runner.stream_subscription_handles["ws-sub"].is_finished());

        // Ticks before retry_at leave everything untouched.
        runner.process_subscriptions(&rt);
        assert_eq!(runner.stream_backoff["ws-sub"].failures, 1);
        assert!(runner.stream_subscription_handles["ws-sub"].is_finished());

        // Once the retry time has passed, a fresh task is spawned.
        runner.stream_backoff.get_mut("ws-sub").unwrap().retry_at =
            Some(Instant::now() - Duration::from_millis(1));
        runner.process_subscriptions(&rt);
        assert_eq!(runner.stream_backoff["ws-sub"].failures, 1);
        assert!(runner.stream_backoff["ws-sub"].retry_at.is_none());
    }

    /// The `focused_id` attached to every Tab the app saw, in order.
    type TabsSeen = Arc<Mutex<Vec<Option<String>>>>;

    struct TabApp {
        tabs_seen: TabsSeen,
        consume_tab: bool,
    }

    impl Application for TabApp {
        type Message = ();

        fn on_event(&mut self, event: &Event) -> Option<Self::Message> {
            if let Event::Key(key_event) = event {
                if key_event.key == Key::Tab {
                    self.tabs_seen
                        .lock()
                        .unwrap()
                        .push(key_event.focused_id.clone());
                    return self.consume_tab.then_some(());
                }
            }
            None
        }

        fn view(&self) -> impl View {
            Column::new()
                .child(crate::widgets::Button::new("A").id("a"))
                .child(crate::widgets::Button::new("B").id("b"))
        }
    }

    fn tab_runner(consume_tab: bool) -> (App<TabApp>, TabsSeen) {
        let tabs_seen = Arc::new(Mutex::new(Vec::new()));
        let app = TabApp {
            tabs_seen: tabs_seen.clone(),
            consume_tab,
        };
        let backend = Backend::Mock(MockBackend::new(vec![Some(Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        }))]));
        (App::new_with_backend(app, backend), tabs_seen)
    }

    #[test]
    fn tab_handled_by_app_does_not_move_focus() {
        let (mut runner, tabs_seen) = tab_runner(true);
        let rt = test_runtime();
        runner.render().expect("initial render");
        assert_eq!(runner.focus.focused_id(), Some("a"));

        runner.run_tick(&rt).expect("tick");

        assert_eq!(*tabs_seen.lock().unwrap(), vec![Some("a".to_string())]);
        assert_eq!(runner.focus.focused_id(), Some("a"));
    }

    #[test]
    fn tab_unhandled_by_app_moves_focus() {
        let (mut runner, tabs_seen) = tab_runner(false);
        let rt = test_runtime();
        runner.render().expect("initial render");

        runner.run_tick(&rt).expect("tick");

        assert_eq!(*tabs_seen.lock().unwrap(), vec![Some("a".to_string())]);
        assert_eq!(runner.focus.focused_id(), Some("b"));
    }

    struct StreamSubApp {
        enabled: Arc<AtomicBool>,
        url: Arc<Mutex<String>>,
    }

    impl Application for StreamSubApp {
        type Message = String;

        fn subscriptions(&self) -> Sub<Self::Message> {
            if self.enabled.load(Ordering::SeqCst) {
                let url = self.url.lock().unwrap().clone();
                Sub::stream("ws-sub", url, |msg| msg)
            } else {
                Sub::none()
            }
        }

        fn view(&self) -> impl View {
            Text::new("stream-sub-test")
        }
    }

    #[test]
    fn sub_stream_registers_and_unsubscribes() {
        let enabled = Arc::new(AtomicBool::new(true));
        let url = Arc::new(Mutex::new("ws://127.0.0.1:0".to_string()));
        let app = StreamSubApp {
            enabled: enabled.clone(),
            url,
        };

        let mut runner = App::new(app);
        let rt = test_runtime();

        runner.process_subscriptions(&rt);
        assert!(runner.stream_subscription_handles.contains_key("ws-sub"));
        assert_eq!(
            runner
                .stream_subscription_urls
                .get("ws-sub")
                .map(String::as_str),
            Some("ws://127.0.0.1:0")
        );

        enabled.store(false, Ordering::SeqCst);
        runner.process_subscriptions(&rt);

        assert!(runner.stream_subscription_handles.is_empty());
        assert!(runner.stream_subscription_urls.is_empty());
    }

    #[test]
    fn sub_stream_url_change_updates_subscription() {
        let enabled = Arc::new(AtomicBool::new(true));
        let url = Arc::new(Mutex::new("ws://127.0.0.1:0".to_string()));
        let app = StreamSubApp {
            enabled,
            url: url.clone(),
        };

        let mut runner = App::new(app);
        let rt = test_runtime();

        runner.process_subscriptions(&rt);
        assert_eq!(
            runner
                .stream_subscription_urls
                .get("ws-sub")
                .map(String::as_str),
            Some("ws://127.0.0.1:0")
        );

        *url.lock().unwrap() = "ws://127.0.0.1:1".to_string();
        runner.process_subscriptions(&rt);

        assert!(runner.stream_subscription_handles.contains_key("ws-sub"));
        assert_eq!(
            runner
                .stream_subscription_urls
                .get("ws-sub")
                .map(String::as_str),
            Some("ws://127.0.0.1:1")
        );
    }

    #[test]
    fn stream_subscription_actions_distinguish_lifecycle_changes() {
        let active_urls = HashMap::from([
            ("b-unchanged".to_string(), "ws://same".to_string()),
            ("c-restart-url".to_string(), "ws://old".to_string()),
            ("d-restart-finished".to_string(), "ws://same".to_string()),
            ("e-remove".to_string(), "ws://stale".to_string()),
        ]);
        let running_by_id = HashMap::from([
            ("b-unchanged".to_string(), true),
            ("c-restart-url".to_string(), true),
            ("d-restart-finished".to_string(), false),
            ("e-remove".to_string(), true),
        ]);
        let desired_urls = HashMap::from([
            ("a-start".to_string(), "ws://new".to_string()),
            ("b-unchanged".to_string(), "ws://same".to_string()),
            ("c-restart-url".to_string(), "ws://new".to_string()),
            ("d-restart-finished".to_string(), "ws://same".to_string()),
        ]);

        let actions = stream_subscription_actions(&active_urls, &running_by_id, &desired_urls);

        assert_eq!(
            actions,
            vec![
                StreamSubscriptionAction::Start {
                    id: "a-start".to_string(),
                },
                StreamSubscriptionAction::Unchanged {
                    id: "b-unchanged".to_string(),
                },
                StreamSubscriptionAction::Restart {
                    id: "c-restart-url".to_string(),
                },
                StreamSubscriptionAction::Restart {
                    id: "d-restart-finished".to_string(),
                },
                StreamSubscriptionAction::Remove {
                    id: "e-remove".to_string(),
                },
            ]
        );
    }

    #[test]
    fn screen_stack_push_changes_render_target() {
        let state = ScreenTestState::new();
        let mut runner = make_screen_runner(state.clone(), vec![]);
        let rt = test_runtime();

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
    fn screen_stack_pop_restores_main_view() {
        let state = ScreenTestState::new();
        let mut runner = make_screen_runner(state.clone(), vec![]);
        let rt = test_runtime();

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
    fn screen_stack_event_dismiss_pops() {
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
        let rt = test_runtime();
        runner.process_cmd(Cmd::PushScreen(Box::new(screen)), &rt);

        runner.run().expect("run app");

        assert_eq!(*mount_calls.lock().unwrap(), 1);
        assert!(*screen_view_calls.lock().unwrap() >= 1);
        assert!(*state.main_view_calls.lock().unwrap() >= 1);
        assert_eq!(*state.app_event_log.lock().unwrap(), vec!['y']);
    }

    #[test]
    fn toast_show_command_adds_to_active() {
        let (mut runner, _updates) = make_test_app();
        let rt = test_runtime();

        runner.process_cmd(Cmd::ShowToast(Toast::info("Saved")), &rt);

        assert_eq!(runner.active_toasts.len(), 1);
        assert_eq!(runner.active_toasts[0].0.message, "Saved");
    }

    #[test]
    fn toast_auto_expires_on_tick_and_triggers_rerender() {
        let app = TickApp {
            updates: Arc::new(Mutex::new(Vec::new())),
            view_calls: Arc::new(Mutex::new(0)),
        };
        let backend = Backend::Mock(MockBackend::new(vec![None, None]));
        let mut runner = App::new_with_backend(app, backend);
        let rt = test_runtime();
        runner.process_cmd(
            Cmd::ShowToast(Toast::info("short").duration(Duration::from_millis(1))),
            &rt,
        );
        runner.needs_render = false;

        // Not yet expired: nothing to re-render.
        runner.active_toasts[0].1 = Instant::now() + Duration::from_secs(60);
        assert!(runner.run_tick(&rt).unwrap());
        assert_eq!(runner.active_toasts.len(), 1);
        assert_eq!(mock_render_count(&runner), 0);

        // Expired: the tick drops it and re-renders without the overlay.
        runner.active_toasts[0].1 = Instant::now() - Duration::from_secs(1);
        assert!(runner.run_tick(&rt).unwrap());
        assert!(runner.active_toasts.is_empty());
        assert_eq!(mock_render_count(&runner), 1);
    }

    /// A native-mode app driven over a socket pair by a fake terminal that
    /// answers every frame late: the app must handle the next message at
    /// once, not after waiting for (or a poll timeout on) that answer.
    #[test]
    fn a_late_terminal_reply_does_not_delay_the_next_message() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use ultty_protocol::{Decode, Encode, FrameId, LayoutTrigger, TerminalMessage};

        struct Bumps(u32);
        impl Application for Bumps {
            type Message = bool;
            fn update(&mut self, quit: bool) -> Cmd<bool> {
                if quit {
                    return Cmd::Quit;
                }
                self.0 += 1;
                Cmd::none()
            }
            fn view(&self) -> impl View {
                Text::new(format!("bumps {}", self.0))
            }
        }

        fn next_message(terminal: &mut UnixStream) -> AppMessage {
            let mut header = [0u8; 8];
            terminal.read_exact(&mut header).unwrap();
            let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let mut frame = header.to_vec();
            frame.resize(8 + len, 0);
            terminal.read_exact(&mut frame[8..]).unwrap();
            AppMessage::decode(&mut &frame[..]).unwrap()
        }
        fn next_frame_end(terminal: &mut UnixStream) -> FrameId {
            loop {
                if let AppMessage::EndFrame { frame_id } = next_message(terminal) {
                    return frame_id;
                }
            }
        }
        fn reply(terminal: &mut UnixStream, frame_id: FrameId) {
            let layout = TerminalMessage::LayoutComputed {
                frame_id: Some(frame_id),
                trigger: LayoutTrigger::ContentChange,
                regions: Vec::new(),
            };
            terminal.write_all(&layout.encode_to_bytes()).unwrap();
        }

        let (app_side, mut terminal) = UnixStream::pair().unwrap();
        let handshake = std::thread::spawn(move || {
            let mut hello = [0u8; 14];
            terminal.read_exact(&mut hello).unwrap();
            for msg in [
                TerminalMessage::ServerHello {
                    version: ultty_protocol::PROTOCOL_VERSION,
                    capabilities: ultty_protocol::CLIENT_DEFAULT_CAPABILITIES,
                },
                TerminalMessage::SessionReady {
                    session_id: 1,
                    root_region: RegionId::ROOT,
                    width: 40,
                    height: 10,
                },
            ] {
                terminal.write_all(&msg.encode_to_bytes()).unwrap();
            }
            terminal
        });
        let mut backend = crate::backend::NativeBackend::new();
        backend.connect_socket(app_side).unwrap();
        let mut terminal = handshake.join().unwrap();

        let runner = App::new_with_backend(Bumps(0), Backend::Native(backend));
        let proxy = runner.proxy();
        let driver = std::thread::spawn(move || {
            let mut unanswered = vec![next_frame_end(&mut terminal)];
            let mut latencies = Vec::new();
            for _ in 0..10 {
                // Sent right after the last frame, whose reply is still owed.
                let sent = Instant::now();
                assert!(proxy.send(false).is_ok());
                unanswered.push(next_frame_end(&mut terminal));
                latencies.push(sent.elapsed());
            }
            // The replies come only now.
            for frame in unanswered {
                reply(&mut terminal, frame);
            }
            assert!(proxy.send(true).is_ok());
            latencies.sort();
            // The terminal stays open until the app has said goodbye.
            (latencies[latencies.len() / 2], terminal)
        });
        runner.run().unwrap();
        let (median, _terminal) = driver.join().unwrap();
        // Waiting for the reply cost a 16 ms poll timeout per frame.
        assert!(
            median < Duration::from_millis(8),
            "message to frame took {median:?} (median)"
        );
    }

    /// A key and the widget focused when it arrived.
    type KeyLog = Arc<Mutex<Vec<(Key, Option<String>)>>>;

    /// Records the keys it is given, with the widget focused at the time.
    struct KeyLogApp {
        keys: KeyLog,
    }

    impl Application for KeyLogApp {
        type Message = (Key, Option<String>);

        fn update(&mut self, msg: Self::Message) -> Cmd<Self::Message> {
            self.keys.lock().unwrap().push(msg);
            Cmd::none()
        }

        fn on_event(&mut self, event: &Event) -> Option<Self::Message> {
            match event {
                Event::Key(key) => Some((key.key.clone(), key.focused_id.clone())),
                _ => None,
            }
        }

        fn view(&self) -> impl View {
            Column::new()
                .child(crate::widgets::Button::new("A").id("a"))
                .child(crate::widgets::Button::new("B").id("b"))
                .child(crate::widgets::TextInput::new().id("search"))
        }
    }

    fn key(key: Key) -> Option<Event> {
        Some(Event::Key(KeyEvent {
            key,
            modifiers: crate::event::Modifiers::default(),
            focused_id: None,
        }))
    }

    fn override_file(
        name: &str,
        source: &str,
    ) -> (std::path::PathBuf, crate::overrides::OverrideFile) {
        let dir = std::env::temp_dir().join(format!("ulttui-app-overrides-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, source).unwrap();
        let file = crate::overrides::OverrideFile::at(&path);
        (path, file)
    }

    #[test]
    fn user_key_remaps_reach_the_app_and_move_focus() {
        let (path, file) = override_file(
            "remap.toml",
            "[keys]\n\"j\" = \"down\"\n\"/\" = { focus = \"b\" }\n\"g\" = { focus = \"nowhere\" }\n",
        );
        let keys = Arc::new(Mutex::new(Vec::new()));
        let backend = Backend::Mock(MockBackend::new(vec![
            key(Key::Char('j')),
            key(Key::Char('/')),
            key(Key::Char('j')),
            key(Key::Char('g')),
        ]));
        let mut runner =
            App::new_with_backend(KeyLogApp { keys: keys.clone() }, backend).override_file(file);
        let rt = test_runtime();
        runner.render().unwrap();
        assert_eq!(runner.focus.focused_id(), Some("a"));

        for _ in 0..4 {
            assert!(runner.run_tick(&rt).unwrap());
        }

        assert_eq!(
            *keys.lock().unwrap(),
            vec![
                (Key::Down, Some("a".to_string())),
                // "/" moved focus to "b" and was used up.
                (Key::Down, Some("b".to_string())),
                // A focus target that is not focusable passes the key on.
                (Key::Char('g'), Some("b".to_string())),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plain_character_remaps_leave_a_focused_input_alone() {
        let (path, file) = override_file(
            "typing.toml",
            "[keys]\n\"j\" = \"down\"\n\"/\" = { focus = \"search\" }\n\"ctrl+n\" = \"down\"\n",
        );
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut events: Vec<Option<Event>> = "/join/".chars().map(|c| key(Key::Char(c))).collect();
        events.push(Some(Event::Key(KeyEvent {
            key: Key::Char('n'),
            modifiers: crate::event::Modifiers {
                ctrl: true,
                ..Default::default()
            },
            focused_id: None,
        })));
        let backend = Backend::Mock(MockBackend::new(events));
        let mut runner =
            App::new_with_backend(KeyLogApp { keys: keys.clone() }, backend).override_file(file);
        let rt = test_runtime();
        runner.render().unwrap();

        for _ in 0..7 {
            assert!(runner.run_tick(&rt).unwrap());
        }

        let keys = keys.lock().unwrap();
        let typed: String = keys
            .iter()
            .filter_map(|(key, focused)| match key {
                Key::Char(c) if focused.as_deref() == Some("search") => Some(*c),
                _ => None,
            })
            .collect();
        // "/" jumped to the input; then j, o, i, n and "/" were typed into it.
        assert_eq!(typed, "join/");
        // A chord with ctrl is still remapped inside the input.
        assert_eq!(keys.last(), Some(&(Key::Down, Some("search".to_string()))));
        let _ = std::fs::remove_file(path);
    }

    /// An app root with an id: "app" on its second row.
    struct RootView;

    impl View for RootView {
        fn widget_id(&self) -> Option<&str> {
            Some("root")
        }

        fn content(&self) -> ultty_protocol::Content {
            ultty_protocol::Content::Text(ultty_protocol::TextContent::new("app"))
        }

        fn render_ansi(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
            let row = ratatui::layout::Rect::new(area.x, area.y + 1, area.width, 1);
            frame.render_widget(ratatui::widgets::Paragraph::new("app"), row);
        }
    }

    #[test]
    fn a_root_override_styles_the_root_only_beneath_the_toasts() {
        use ratatui::style::Color as RColor;
        let overrides = std::rc::Rc::new(
            crate::overrides::Overrides::parse(
                "[widget.root]\nfg = \"red\"\nbg = \"blue\"\nhidden = true\ntext = \"renamed\"\n",
            )
            .unwrap(),
        );
        let toast = Column::new().child(Badge::new("saved"));
        let composed = OverlayView::new(&RootView, Some(toast));

        let (native, buffer) = crate::overrides::with_overrides(overrides, || {
            let native = crate::widgets::render_view_messages(&composed).unwrap();
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(12, 2)).unwrap();
            terminal
                .draw(|frame| crate::widgets::render_view_ansi(&composed, frame, frame.area()))
                .unwrap();
            (native, terminal.backend().buffer().clone())
        });

        // Native: the root region takes the style, and hidden and text are
        // ignored, as in ANSI.
        let AppMessage::CreateRegion {
            constraints, style, ..
        } = &native[0]
        else {
            panic!("the root region comes first: {native:?}");
        };
        assert_ne!(constraints.display, ultty_protocol::Display::None);
        assert_eq!(
            style.fg,
            Some(ultty_protocol::Color::Named(
                ultty_protocol::NamedColor::Red
            ))
        );
        let shown = format!("{native:?}");
        assert!(
            shown.contains("\"app\"") && !shown.contains("renamed"),
            "{shown}"
        );

        // ANSI: the root is drawn and restyled; the toast over it keeps its
        // own colours.
        let app_cell = &buffer[(0, 1)];
        assert_eq!(app_cell.symbol(), "a");
        assert_eq!((app_cell.fg, app_cell.bg), (RColor::Red, RColor::Blue));
        let toast_row: String = (0..12).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(toast_row.contains("saved"), "{toast_row:?}");
        let toast_cell = (0..12)
            .map(|x| &buffer[(x, 0)])
            .find(|cell| cell.symbol() == "s")
            .unwrap();
        assert_ne!(toast_cell.fg, RColor::Red, "the toast is not restyled");
    }

    #[test]
    fn a_broken_override_file_leaves_the_app_running_unchanged() {
        let (path, file) = override_file("broken.toml", "[widget.a\nhidden = true\n");
        let keys = Arc::new(Mutex::new(Vec::new()));
        let backend = Backend::Mock(MockBackend::new(vec![key(Key::Char('x'))]));
        let mut runner =
            App::new_with_backend(KeyLogApp { keys: keys.clone() }, backend).override_file(file);
        let rt = test_runtime();
        runner.render().unwrap();

        assert!(runner.run_tick(&rt).unwrap());

        assert_eq!(
            *keys.lock().unwrap(),
            vec![(Key::Char('x'), Some("a".to_string()))]
        );
        let Backend::Mock(mock) = &runner.backend else {
            panic!("mock backend");
        };
        let labels = format!("{:?}", mock.rendered_messages.last().unwrap());
        assert!(labels.contains("label: \"A\"") && labels.contains("label: \"B\""));
        let _ = std::fs::remove_file(path);
    }
}
