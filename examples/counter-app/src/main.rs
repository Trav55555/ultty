use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::RichText;

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
            Event::Key(ke) => match (&ke.key, ke.focused_id.as_deref()) {
                // Semantic: Enter on focused button triggers its action
                (Key::Enter, Some("dec")) => Some(Message::Decrement),
                (Key::Enter, Some("inc")) => Some(Message::Increment),
                (Key::Enter, Some("reset")) => Some(Message::Reset),
                // Keyboard shortcuts still work regardless of focus
                (Key::Char('+') | Key::Char('='), _) => Some(Message::Increment),
                (Key::Char('-') | Key::Char('_'), _) => Some(Message::Decrement),
                (Key::Char('r'), _) => Some(Message::Reset),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        let theme = Theme::current();
        let count_color = if self.count > 0 {
            theme.success
        } else if self.count < 0 {
            theme.error
        } else {
            theme.warning
        };

        let changes = self.history.len().saturating_sub(1);
        let trend = match self
            .history
            .iter()
            .rev()
            .take(2)
            .copied()
            .collect::<Vec<_>>()
            .as_slice()
        {
            [latest, prev] if latest > prev => "UP",
            [latest, prev] if latest < prev => "DOWN",
            [_, _] => "FLAT",
            _ => "FLAT",
        };

        let recent_history = self
            .history
            .iter()
            .rev()
            .take(6)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" -> ");

        Column::new()
            .padding(2.0)
            .gap(1.0)
            .child(
                RichText::new()
                    .span("Counter Studio", Style::new().fg(theme.primary).bold())
                    .span("  native ulttui demo", Style::new().fg(theme.muted)),
            )
            .child(Divider::horizontal())
            .child(RichText::new().span("CURRENT VALUE", Style::new().fg(theme.muted)))
            .child(RichText::new().span(
                format!("{:^22}", self.count),
                Style::new().fg(count_color).bold(),
            ))
            .child(
                RichText::new()
                    .span("changes ", Style::new().fg(theme.muted))
                    .span(
                        format!("{}", changes),
                        Style::new().fg(theme.foreground).bold(),
                    )
                    .span("  trend ", Style::new().fg(theme.muted))
                    .span(trend, Style::new().fg(theme.primary).bold()),
            )
            .child(Divider::horizontal())
            .child(
                Row::new()
                    .gap(1.0)
                    .child(Button::new("- Decrease").id("dec"))
                    .child(Button::new("+ Increase").id("inc"))
                    .child(Button::new("Reset").id("reset")),
            )
            .child(Divider::horizontal())
            .child(
                Column::new()
                    .gap(1.0)
                    .child(RichText::new().span("RECENT HISTORY", Style::new().fg(theme.muted)))
                    .child(RichText::new().span(recent_history, Style::new().fg(theme.foreground))),
            )
            .child(Divider::horizontal())
            .child(RichText::new().span(
                "Tab to focus buttons, Enter to trigger, +/- to change, r to reset",
                Style::new().fg(theme.muted),
            ))
    }
}

fn main() -> anyhow::Result<()> {
    CounterApp::new().run()
}
