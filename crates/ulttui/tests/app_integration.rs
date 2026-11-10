//! Integration tests for the App event loop.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ulttui::app::{App, Application, Proxy};
use ulttui::backend::mock::MockBackend;
use ulttui::backend::Backend;
use ulttui::cmd::{Cmd, Sub};
use ulttui::event::{Event, Key, KeyEvent, Modifiers};
use ulttui::widgets::{Button, Column, Text, View};

// --- Simple counter app for testing ---

struct TestCounter {
    count: i32,
    shared: Arc<Mutex<i32>>,
}

enum TestMsg {
    Increment,
}

impl Application for TestCounter {
    type Message = TestMsg;

    fn update(&mut self, msg: TestMsg) -> Cmd<TestMsg> {
        match msg {
            TestMsg::Increment => {
                self.count += 1;
                if let Ok(mut shared) = self.shared.lock() {
                    *shared = self.count;
                }
                Cmd::none()
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<TestMsg> {
        match event {
            Event::Key(ke) => match ke.key {
                Key::Char('+') => Some(TestMsg::Increment),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Text::new(format!("Count: {}", self.count))
    }
}

struct SpawnApp {
    shared: Arc<Mutex<Option<i32>>>,
}

struct DropSignal {
    shared: Arc<Mutex<Option<i32>>>,
}

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            if shared.is_none() {
                *shared = Some(42);
            }
        }
    }
}

enum SpawnMsg {
    Start,
    Done(i32),
}

impl Application for SpawnApp {
    type Message = SpawnMsg;

    fn update(&mut self, msg: SpawnMsg) -> Cmd<SpawnMsg> {
        match msg {
            SpawnMsg::Start => {
                let shared = Arc::clone(&self.shared);
                Cmd::spawn(async move {
                    // Yield to ensure the task is scheduled
                    tokio::task::yield_now().await;
                    let _signal = DropSignal { shared };
                    SpawnMsg::Done(42)
                })
            }
            SpawnMsg::Done(value) => {
                if let Ok(mut shared) = self.shared.lock() {
                    *shared = Some(value);
                }
                // The test ends when the spawned result arrives, not after a
                // fixed number of polls, so it cannot race the task.
                Cmd::Quit
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<SpawnMsg> {
        match event {
            Event::Key(ke) => match ke.key {
                Key::Char('s') => Some(SpawnMsg::Start),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Text::new("Spawn")
    }
}

fn make_key(ch: char) -> Option<Event> {
    Some(Event::Key(KeyEvent {
        key: Key::Char(ch),
        modifiers: Modifiers::default(),
        focused_id: None,
    }))
}

fn make_tab(shift: bool) -> Option<Event> {
    Some(Event::Key(KeyEvent {
        key: Key::Tab,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
        focused_id: None,
    }))
}

// --- Tests ---

#[test]
fn event_loop_processes_key_events() {
    let events = vec![
        make_key('+'),
        make_key('+'),
        make_key('+'),
        None,
        Some(Event::Quit),
    ];
    let shared = Arc::new(Mutex::new(0));
    let app = TestCounter {
        count: 0,
        shared: Arc::clone(&shared),
    };
    let mock = MockBackend::new(events);
    let runner = App::new_with_backend(app, Backend::Mock(mock));
    runner.run().unwrap();

    let final_count = shared.lock().expect("count lock");
    assert_eq!(*final_count, 3);
}

/// Records the `focused_id` the runner attached to every key event.
struct FocusProbe {
    focused_ids: Arc<Mutex<Vec<Option<String>>>>,
}

impl Application for FocusProbe {
    type Message = ();

    fn on_event(&mut self, event: &Event) -> Option<()> {
        if let Event::Key(ke) = event {
            self.focused_ids.lock().unwrap().push(ke.focused_id.clone());
        }
        None
    }

    fn view(&self) -> impl View {
        Column::new()
            .child(Button::new("First").id("first"))
            .child(Button::new("Second").id("second"))
    }
}

#[test]
fn tab_navigation_cycles_focus_across_registered_widgets() {
    let events = vec![
        make_tab(false),
        make_tab(true),
        make_tab(false),
        make_key('x'),
        Some(Event::Quit),
    ];
    let focused_ids = Arc::new(Mutex::new(Vec::new()));
    let app = FocusProbe {
        focused_ids: Arc::clone(&focused_ids),
    };
    let mock = MockBackend::new(events);
    let runner = App::new_with_backend(app, Backend::Mock(mock));
    runner.run().unwrap();

    // The first render focuses "first"; each Tab (left unhandled by the app)
    // moves focus and the next key event reports the new focused widget.
    let seen = focused_ids.lock().unwrap();
    assert_eq!(
        *seen,
        vec![
            Some("first".to_string()),
            Some("second".to_string()),
            Some("first".to_string()),
            Some("second".to_string()),
        ]
    );
}

#[test]
fn cmd_spawn_executes() {
    let events = vec![make_key('s')];
    let shared = Arc::new(Mutex::new(None));
    let app = SpawnApp {
        shared: Arc::clone(&shared),
    };
    let mock = MockBackend::new(events).idle_until_quit();
    let runner = App::new_with_backend(app, Backend::Mock(mock));
    runner.run().unwrap();

    let value = shared.lock().expect("spawn lock");
    assert_eq!(*value, Some(42));
}

// --- Waking the event loop ---

/// Counts ticks of an optional timer and quits on `Quit` or after
/// `quit_after_ticks` ticks.
struct WakeProbe {
    timer: Option<Duration>,
    ticks: usize,
    quit_after_ticks: usize,
    init_cmd: Option<Cmd<WakeMsg>>,
}

enum WakeMsg {
    Tick,
    Quit,
}

impl Application for WakeProbe {
    type Message = WakeMsg;

    fn init(&mut self) -> Cmd<WakeMsg> {
        self.init_cmd.take().unwrap_or_default()
    }

    fn update(&mut self, msg: WakeMsg) -> Cmd<WakeMsg> {
        match msg {
            WakeMsg::Tick => {
                self.ticks += 1;
                if self.ticks >= self.quit_after_ticks {
                    return Cmd::Quit;
                }
                Cmd::none()
            }
            WakeMsg::Quit => Cmd::Quit,
        }
    }

    fn subscriptions(&self) -> Sub<WakeMsg> {
        match self.timer {
            Some(interval) => Sub::every(interval, || WakeMsg::Tick),
            None => Sub::none(),
        }
    }

    fn view(&self) -> impl View {
        Text::new(format!("ticks {}", self.ticks))
    }
}

fn probe(timer: Option<Duration>, quit_after_ticks: usize) -> WakeProbe {
    WakeProbe {
        timer,
        ticks: 0,
        quit_after_ticks,
        init_cmd: None,
    }
}

/// Run `app` on a mock terminal that has no input; returns the run time
/// and the number of event loop passes.
fn run_idle(
    app: WakeProbe,
    while_running: impl FnOnce(Proxy<WakeMsg>) + Send + 'static,
) -> (Duration, usize) {
    let mock = MockBackend::new(Vec::new()).blocking_when_exhausted();
    let polls = mock.poll_count();
    let runner = App::new_with_backend(app, Backend::Mock(mock));
    let proxy = runner.proxy();
    let driver = std::thread::spawn(move || while_running(proxy));
    let started = Instant::now();
    runner.run().unwrap();
    let elapsed = started.elapsed();
    driver.join().unwrap();
    (elapsed, polls.load(Ordering::SeqCst))
}

#[test]
fn an_idle_app_sleeps_until_a_proxy_message_wakes_it() {
    let (elapsed, passes) = run_idle(probe(None, usize::MAX), |proxy| {
        std::thread::sleep(Duration::from_millis(300));
        assert!(proxy.send(WakeMsg::Quit).is_ok());
    });
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    // The first pass, then the one the message woke: no polling while idle.
    assert!(passes <= 3, "{passes} passes in {elapsed:?}");
}

#[test]
fn a_timer_deadline_wakes_the_loop() {
    let (elapsed, passes) = run_idle(probe(Some(Duration::from_millis(100)), 3), |_| {});
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(passes <= 8, "{passes} passes for 3 ticks");
}

#[test]
fn a_zero_interval_timer_ticks_at_the_minimum_interval() {
    let (elapsed, passes) = run_idle(probe(Some(Duration::ZERO), 5), |_| {});
    let min = ulttui::app::MIN_TIMER_INTERVAL * 5;
    assert!(elapsed >= min, "{elapsed:?} for 5 ticks");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(passes <= 12, "{passes} passes for 5 ticks");
}

#[test]
fn a_finished_command_wakes_the_loop() {
    let mut app = probe(None, 1);
    app.init_cmd = Some(Cmd::spawn(async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        WakeMsg::Tick
    }));
    let (elapsed, passes) = run_idle(app, |_| {});
    assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(passes <= 3, "{passes} passes");
}

/// Logs the order of `init`, `update` and `view` calls.
struct InitProbe {
    log: Arc<Mutex<Vec<String>>>,
}

impl Application for InitProbe {
    type Message = &'static str;

    fn init(&mut self) -> Cmd<&'static str> {
        self.log.lock().unwrap().push("init".into());
        Cmd::batch(vec![
            Cmd::message("from init"),
            Cmd::spawn(async { "spawned by init" }),
        ])
    }

    fn update(&mut self, msg: &'static str) -> Cmd<&'static str> {
        self.log.lock().unwrap().push(msg.into());
        if msg == "spawned by init" {
            return Cmd::Quit;
        }
        Cmd::none()
    }

    fn view(&self) -> impl View {
        self.log.lock().unwrap().push("view".into());
        Text::new("init probe")
    }
}

#[test]
fn init_runs_once_before_the_first_view_and_its_command_runs() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let app = InitProbe { log: log.clone() };
    let mock = MockBackend::new(vec![make_key('a'), make_key('b')]).blocking_when_exhausted();
    App::new_with_backend(app, Backend::Mock(mock))
        .run()
        .unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log[..3], ["init", "from init", "view"], "{log:?}");
    assert_eq!(log.iter().filter(|e| *e == "init").count(), 1, "{log:?}");
    assert!(log.iter().any(|e| e == "spawned by init"), "{log:?}");
}

struct QuitAtOnce(Arc<AtomicUsize>);

impl Application for QuitAtOnce {
    type Message = ();

    fn init(&mut self) -> Cmd<()> {
        Cmd::Quit
    }

    fn view(&self) -> impl View {
        self.0.fetch_add(1, Ordering::SeqCst);
        Text::new("never")
    }
}

#[test]
fn quitting_in_init_ends_the_run_without_a_frame() {
    let views = Arc::new(AtomicUsize::new(0));
    let mock = MockBackend::new(Vec::new()).blocking_when_exhausted();
    App::new_with_backend(QuitAtOnce(views.clone()), Backend::Mock(mock))
        .run()
        .unwrap();
    assert_eq!(views.load(Ordering::SeqCst), 0);
}
