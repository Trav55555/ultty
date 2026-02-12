use ulttui::event::{Event, Key};
use ulttui::prelude::*;

struct CounterApp {
    count: i32,
}

impl CounterApp {
    fn new() -> Self {
        Self { count: 0 }
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
            Message::Increment => self.count += 1,
            Message::Decrement => self.count -= 1,
            Message::Reset => self.count = 0,
        }
        Cmd::none()
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(ke) => match (&ke.key, ke.focused_id.as_deref()) {
                (Key::Enter, Some("dec")) => Some(Message::Decrement),
                (Key::Enter, Some("inc")) => Some(Message::Increment),
                (Key::Enter, Some("reset")) => Some(Message::Reset),
                (Key::Char('+') | Key::Char('='), _) => Some(Message::Increment),
                (Key::Char('-') | Key::Char('_'), _) => Some(Message::Decrement),
                (Key::Char('r'), _) => Some(Message::Reset),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Card::new()
            .child(Title::new("Counter"))
            .child(Accent::new(format!("{}", self.count)))
            .child(
                Row::new()
                    .gap(1.0)
                    .child(Button::new("-1").id("dec"))
                    .child(Button::new("+1").id("inc"))
                    .child(Button::new("Reset").id("reset")),
            )
            .child(Muted::new("Tab to focus, Enter to press, +/- keys"))
    }
}

fn main() -> anyhow::Result<()> {
    CounterApp::new().run()
}
