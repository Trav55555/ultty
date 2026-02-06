//! Async fetch example — demonstrates Cmd::spawn for async tasks.

use std::time::{Duration, Instant};

use ulttui::event::{Event, Key};
use ulttui::prelude::*;

#[derive(Debug, Clone)]
enum FetchState {
    Idle,
    Loading {
        started: Instant,
        spinner_frame: usize,
    },
    Success {
        data: String,
        elapsed: Duration,
    },
    Error {
        message: String,
    },
}

struct FetchApp {
    state: FetchState,
}

impl FetchApp {
    fn new() -> Self {
        Self {
            state: FetchState::Idle,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
enum Msg {
    StartFetch,
    FetchComplete(String),
    FetchFailed(String),
    SpinnerTick,
    Reset,
}

impl Application for FetchApp {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::StartFetch => {
                self.state = FetchState::Loading {
                    started: Instant::now(),
                    spinner_frame: 0,
                };
                Cmd::spawn(async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Msg::FetchComplete(
                        "{ \"users\": 1423, \"status\": \"healthy\", \"uptime\": \"14d 7h\" }"
                            .to_string(),
                    )
                })
            }
            Msg::FetchComplete(data) => {
                if let FetchState::Loading { started, .. } = &self.state {
                    self.state = FetchState::Success {
                        data,
                        elapsed: started.elapsed(),
                    };
                }
                Cmd::none()
            }
            Msg::FetchFailed(message) => {
                self.state = FetchState::Error { message };
                Cmd::none()
            }
            Msg::SpinnerTick => {
                if let FetchState::Loading { spinner_frame, .. } = &mut self.state {
                    *spinner_frame = (*spinner_frame + 1) % 10;
                }
                Cmd::none()
            }
            Msg::Reset => {
                self.state = FetchState::Idle;
                Cmd::none()
            }
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        match &self.state {
            FetchState::Loading { .. } => {
                Sub::every(Duration::from_millis(80), || Msg::SpinnerTick)
            }
            _ => Sub::none(),
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(ke) => match ke.key {
                Key::Char('f') | Key::Enter => match &self.state {
                    FetchState::Idle | FetchState::Success { .. } | FetchState::Error { .. } => {
                        Some(Msg::StartFetch)
                    }
                    _ => None,
                },
                Key::Char('r') => Some(Msg::Reset),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .padding(1.0)
            .gap(1.0)
            .child(Text::new("Async Fetch Demo — Cmd::spawn"))
            .child(Divider::horizontal())
            .child(self.status_view())
            .child(Divider::horizontal())
            .child(Text::new("f/Enter: fetch | r: reset | Ctrl+C: quit"))
    }
}

impl FetchApp {
    fn status_view(&self) -> Column {
        match &self.state {
            FetchState::Idle => {
                Column::new()
                    .child(Text::new("Status: Idle"))
                    .child(Text::new("Press 'f' to start a simulated API fetch"))
            }
            FetchState::Loading { spinner_frame, started } => {
                let elapsed = started.elapsed();
                Column::new()
                    .child(
                        Row::new()
                            .gap(1.0)
                            .child(Spinner::new().frame(*spinner_frame))
                            .child(Text::new("Fetching data...")),
                    )
                    .child(Text::new(format!("Elapsed: {:.1}s", elapsed.as_secs_f64())))
            }
            FetchState::Success { data, elapsed } => {
                Column::new()
                    .child(Badge::success("SUCCESS"))
                    .child(Text::new(format!("Response ({:.0}ms):", elapsed.as_millis())))
                    .child(Text::new(data.clone()))
            }
            FetchState::Error { message } => {
                Column::new()
                    .child(Badge::error("ERROR"))
                    .child(Text::new(message.clone()))
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    FetchApp::new().run()
}
