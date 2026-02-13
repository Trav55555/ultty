use ulttui::event::{Event, Key};
use ulttui::prelude::*;

#[derive(Clone)]
struct TodoItem {
    text: String,
    completed: bool,
}

impl std::fmt::Display for TodoItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let marker = if self.completed { "[x]" } else { "[ ]" };
        write!(f, "{} {}", marker, self.text)
    }
}

struct TodoApp {
    items: Vec<TodoItem>,
    selected: usize,
}

impl TodoApp {
    fn new() -> Self {
        Self {
            items: vec![
                TodoItem {
                    text: "Learn Rust".into(),
                    completed: true,
                },
                TodoItem {
                    text: "Build a TUI framework".into(),
                    completed: true,
                },
                TodoItem {
                    text: "Create native protocol".into(),
                    completed: true,
                },
                TodoItem {
                    text: "Write documentation".into(),
                    completed: false,
                },
                TodoItem {
                    text: "Release v1.0".into(),
                    completed: false,
                },
            ],
            selected: 0,
        }
    }
}

enum Message {
    MoveUp,
    MoveDown,
    Toggle,
}

impl Application for TodoApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::MoveUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                Cmd::none()
            }
            Message::MoveDown => {
                if self.selected < self.items.len().saturating_sub(1) {
                    self.selected += 1;
                }
                Cmd::none()
            }
            Message::Toggle => {
                if let Some(item) = self.items.get_mut(self.selected) {
                    item.completed = !item.completed;
                }
                Cmd::none()
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(key_event) => match key_event.key {
                Key::Char('j') | Key::Down => Some(Message::MoveDown),
                Key::Char('k') | Key::Up => Some(Message::MoveUp),
                Key::Enter | Key::Char(' ') => Some(Message::Toggle),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        let items_display: Vec<String> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let prefix = if i == self.selected { "> " } else { "  " };
                format!("{}{}", prefix, item)
            })
            .collect();

        Column::new()
            .flex_grow(1.0)
            .height(Dimension::Percent(100.0))
            .justify_center()
            .align_center()
            .child(
                Card::new()
                    .max_width(50.0)
                    .child(Row::new().justify_center().child(Title::new("Todo App")))
                    .child(Divider::horizontal())
                    .child(List::new().items(items_display))
                    .child(Divider::horizontal())
                    .child(Row::new().justify_center().child(Body::new(format!(
                        "Total: {} | Done: {} | Pending: {}",
                        self.items.len(),
                        self.items.iter().filter(|i| i.completed).count(),
                        self.items.iter().filter(|i| !i.completed).count()
                    ))))
                    .child(Row::new().justify_center().child(Muted::new(
                        "j/k navigate · Enter/Space toggle · Ctrl+C quit",
                    ))),
            )
    }
}

fn main() -> anyhow::Result<()> {
    TodoApp::new().run()
}
