use std::time::{Duration, Instant};

use ulttui::event::{Event, Key};
use ulttui::prelude::*;

/// Dashboard items shown in the navigation list.
const ITEMS: &[(&str, &str)] = &[
    (
        "System Overview",
        "CPU: 23% | Memory: 4.2/16 GB | Disk: 142 GB free\n\
         Load average: 1.24 0.98 0.87\n\
         Uptime: 14 days, 7 hours\n\
         Processes: 312 total, 2 running",
    ),
    (
        "Network",
        "Interface: eth0\n\
         IP: 192.168.1.42 | Gateway: 192.168.1.1\n\
         RX: 12.4 GB | TX: 3.1 GB\n\
         Active connections: 47",
    ),
    (
        "Services",
        "nginx        running  (pid 1234)\n\
         postgresql   running  (pid 2345)\n\
         redis        running  (pid 3456)\n\
         worker       stopped",
    ),
    (
        "Logs",
        "[INFO]  Request handled in 12ms\n\
         [INFO]  Connection from 10.0.0.5\n\
         [WARN]  Slow query: 340ms\n\
         [INFO]  Cache hit ratio: 94.2%\n\
         [ERROR] Failed to connect to upstream",
    ),
    (
        "Configuration",
        "Environment: production\n\
         Config file: /etc/app/config.toml\n\
         Log level: info\n\
         Workers: 8\n\
         Max connections: 1024",
    ),
];

struct DashboardApp {
    selected: usize,
    start_time: Instant,
}

impl DashboardApp {
    fn new() -> Self {
        Self {
            selected: 0,
            start_time: Instant::now(),
        }
    }

    fn elapsed_display(&self) -> String {
        let elapsed = self.start_time.elapsed();
        let secs = elapsed.as_secs();
        let mins = secs / 60;
        let hrs = mins / 60;
        format!("{:02}:{:02}:{:02}", hrs, mins % 60, secs % 60)
    }
}

enum Message {
    MoveUp,
    MoveDown,
    SelectFirst,
    SelectLast,
    Tick,
    Quit,
}

impl Application for DashboardApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::MoveUp => {
                self.selected = self.selected.saturating_sub(1);
                Cmd::none()
            }
            Message::MoveDown => {
                if self.selected < ITEMS.len() - 1 {
                    self.selected += 1;
                }
                Cmd::none()
            }
            Message::SelectFirst => {
                self.selected = 0;
                Cmd::none()
            }
            Message::SelectLast => {
                self.selected = ITEMS.len() - 1;
                Cmd::none()
            }
            Message::Tick => Cmd::none(),
            Message::Quit => Cmd::Quit,
        }
    }

    fn subscriptions(&self) -> Sub<Message> {
        Sub::every(Duration::from_secs(1), || Message::Tick)
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(key_event) if key_event.modifiers.ctrl => match key_event.key {
                Key::Char('c') => Some(Message::Quit),
                _ => None,
            },
            Event::Key(key_event) => match key_event.key {
                Key::Char('q') => Some(Message::Quit),
                Key::Char('k') | Key::Up => Some(Message::MoveUp),
                Key::Char('j') | Key::Down => Some(Message::MoveDown),
                Key::Char('g') | Key::Home => Some(Message::SelectFirst),
                Key::Char('G') | Key::End => Some(Message::SelectLast),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        let (title, detail) = ITEMS[self.selected];

        // The list draws the selection and tells screen readers which item
        // it is.
        let nav_items: Vec<String> = ITEMS.iter().map(|(name, _)| name.to_string()).collect();

        Column::new()
            .flex_grow(1.0)
            .child(
                Row::new()
                    .gap(2.0)
                    .child(Title::new("Dashboard"))
                    .child(Muted::new(format!(
                        "{} | {} items",
                        self.elapsed_display(),
                        ITEMS.len()
                    ))),
            )
            .child(Divider::horizontal())
            .child(
                Row::new()
                    .flex_grow(1.0)
                    .child(
                        Column::new()
                            .width(Dimension::Percent(30.0))
                            .child(List::new().items(nav_items).selected(self.selected)),
                    )
                    .child(
                        Column::new()
                            .flex_grow(1.0)
                            .padding(1.0)
                            .child(Title::new(title))
                            .child(Text::new(detail.to_string())),
                    ),
            )
            .child(Divider::horizontal())
            .child(Muted::new("j/k navigate · g/G first/last · q quit"))
    }
}

fn main() -> anyhow::Result<()> {
    DashboardApp::new().run()?;
    Ok(())
}
