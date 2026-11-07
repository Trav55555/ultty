use ulttui::event::{Event, Key};
use ulttui::prelude::*;

struct CounterApp {
    count: i32,
}

impl CounterApp {
    fn new() -> Self {
        Self { count: 0 }
    }

    fn count_color(&self) -> Color {
        let theme = Theme::current();
        if self.count > 0 {
            theme.success
        } else if self.count < 0 {
            theme.error
        } else {
            theme.foreground
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
                (Key::Char('q'), _) => std::process::exit(0),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .flex_grow(1.0)
            .height(Dimension::Percent(100.0))
            .justify_center()
            .align_center()
            .child(
                Card::new()
                    .max_width(40.0)
                    .child(Row::new().justify_center().child(Title::new("Counter")))
                    .child(Divider::horizontal())
                    .child(Row::new().justify_center().child(
                        ulttui::widgets::RichText::new().span(
                            format!("{}", self.count),
                            Style::new().fg(self.count_color()).bold(),
                        ),
                    ))
                    .child(Divider::horizontal())
                    .child(
                        Row::new()
                            .gap(2.0)
                            .justify_center()
                            .child(Button::new("-1").id("dec").flex_grow(1.0))
                            .child(Button::new("+1").id("inc").flex_grow(1.0))
                            .child(Button::new("Reset").id("reset").flex_grow(1.0)),
                    )
                    .child(
                        Row::new()
                            .justify_center()
                            .child(Muted::new("+/- keys · Tab · Enter · q quit")),
                    ),
            )
    }
}

fn main() -> anyhow::Result<()> {
    CounterApp::new().run()?;
    Ok(())
}
