//! Integration tests for the App event loop.

use std::sync::{Arc, Mutex};

use ulttui::app::{App, Application};
use ulttui::backend::mock::MockBackend;
use ulttui::backend::Backend;
use ulttui::cmd::Cmd;
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
                Cmd::none()
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
    let mut events = vec![make_key('s')];
    events.extend(std::iter::repeat_n(None, 50));
    events.push(Some(Event::Quit));
    let shared = Arc::new(Mutex::new(None));
    let app = SpawnApp {
        shared: Arc::clone(&shared),
    };
    let mock = MockBackend::new(events);
    let runner = App::new_with_backend(app, Backend::Mock(mock));
    runner.run().unwrap();

    let value = shared.lock().expect("spawn lock");
    assert_eq!(*value, Some(42));
}
