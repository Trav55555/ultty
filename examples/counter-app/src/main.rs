use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::SemanticActionKind;

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

    fn supports_action(&self, widget_id: &str, action: SemanticActionKind) -> bool {
        action == SemanticActionKind::Activate && matches!(widget_id, "dec" | "inc" | "reset")
    }

    fn on_action(&mut self, widget_id: &str, action: SemanticActionKind) -> Option<Message> {
        if action != SemanticActionKind::Activate {
            return None;
        }
        match widget_id {
            "dec" => Some(Message::Decrement),
            "inc" => Some(Message::Increment),
            "reset" => Some(Message::Reset),
            _ => None,
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(ke) => match (&ke.key, ke.focused_id.as_deref()) {
                (Key::Enter, Some(widget_id)) => {
                    self.on_action(widget_id, SemanticActionKind::Activate)
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_counter_activation_and_enter_share_update() {
        let mut app = CounterApp::new();
        let action = app.on_action("inc", SemanticActionKind::Activate).unwrap();
        app.update(action);
        let key = Event::Key(ulttui::event::KeyEvent {
            key: Key::Enter,
            modifiers: Default::default(),
            focused_id: Some("inc".into()),
        });
        let message = app.on_event(&key).unwrap();
        app.update(message);
        assert_eq!(app.count, 2);
        assert!(!app.supports_action("unknown", SemanticActionKind::Activate));
        assert!(app
            .on_action("unknown", SemanticActionKind::Activate)
            .is_none());
    }
}
