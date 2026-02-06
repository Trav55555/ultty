use ulttui::event::{Event, Key};
use ulttui::prelude::*;

struct CounterApp {
    count: i32,
    history: Vec<i32>,
}

impl CounterApp {
    fn new() -> Self {
        Self {
            count: 0,
            history: vec![0],
        }
    }
}

enum Message {
    Increment,
    Decrement,
    Reset,
}

impl Application for CounterApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::Increment => {
                self.count += 1;
                self.history.push(self.count);
                Cmd::none()
            }
            Message::Decrement => {
                self.count -= 1;
                self.history.push(self.count);
                Cmd::none()
            }
            Message::Reset => {
                self.count = 0;
                self.history.clear();
                self.history.push(0);
                Cmd::none()
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(key_event) => match key_event.key {
                Key::Char('+') | Key::Char('=') => Some(Message::Increment),
                Key::Char('-') | Key::Char('_') => Some(Message::Decrement),
                Key::Char('r') => Some(Message::Reset),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .padding(1.0)
            .gap(1.0)
            .child(Text::new("Counter App - ulttui Framework Demo"))
            .child(
                Row::new()
                    .gap(2.0)
                    .child(Text::new(format!("Count: {}", self.count)))
                    .child(Text::new(format!(
                        "History: {} changes",
                        self.history.len() - 1
                    ))),
            )
            .child(
                Row::new()
                    .gap(1.0)
                    .child(Button::new("-"))
                    .child(Button::new("+"))
                    .child(Button::new("Reset")),
            )
            .child(
                Column::new()
                    .child(Text::new("Recent history:"))
                    .child(Text::new(
                        self.history
                            .iter()
                            .rev()
                            .take(5)
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join(" <- "),
                    )),
            )
            .child(Text::new("Keys: +/- to change, r to reset, Ctrl+C to quit"))
    }
}

fn main() -> anyhow::Result<()> {
    CounterApp::new().run()
}
