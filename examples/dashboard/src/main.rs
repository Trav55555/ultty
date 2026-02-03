use std::time::Instant;

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
}

impl Application for DashboardApp {
    type Message = Message;

    fn update(&mut self, msg: Message) {
        match msg {
            Message::MoveUp => {
                self.selected = self.selected.saturating_sub(1);
            }
            Message::MoveDown => {
                if self.selected < ITEMS.len() - 1 {
                    self.selected += 1;
                }
            }
            Message::SelectFirst => {
                self.selected = 0;
            }
            Message::SelectLast => {
                self.selected = ITEMS.len() - 1;
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(key_event) => match key_event.key {
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

        // Build navigation list items
        let nav_items: Vec<String> = ITEMS
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                if i == self.selected {
                    format!(" > {} ", name)
                } else {
                    format!("   {} ", name)
                }
            })
            .collect();

        Column::new()
            .child(
                // Header
                Row::new()
                    .child(Text::new(format!(
                        " Dashboard | {} | {} items",
                        self.elapsed_display(),
                        ITEMS.len()
                    ))),
            )
            .child(
                // Main content: navigation + detail
                Row::new()
                    .child(
                        // Left: navigation list
                        List::new().items(nav_items),
                    )
                    .child(
                        // Right: detail view
                        Column::new()
                            .padding(1.0)
                            .child(Text::new(format!("# {}", title)))
                            .child(Text::new(""))
                            .child(Text::new(detail.to_string())),
                    ),
            )
            .child(
                // Footer
                Text::new(" j/k: navigate | g/G: first/last | Ctrl+C: quit"),
            )
    }
}

fn main() -> anyhow::Result<()> {
    DashboardApp::new().run()
}
