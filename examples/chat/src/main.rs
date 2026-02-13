use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::RichText;

struct ChatMessage {
    sender: String,
    text: String,
    is_user: bool,
}

struct ChatApp {
    messages: Vec<ChatMessage>,
    input: String,
}

impl ChatApp {
    fn new() -> Self {
        Self {
            messages: vec![ChatMessage {
                sender: "Bot".into(),
                text: "Hello! Type a message and press Enter to chat.".into(),
                is_user: false,
            }],
            input: String::new(),
        }
    }

    fn send_message(&mut self) {
        if self.input.is_empty() {
            return;
        }

        let user_text = self.input.clone();
        self.messages.push(ChatMessage {
            sender: "You".into(),
            text: user_text.clone(),
            is_user: true,
        });

        // Simulated bot response
        let reply = match user_text.to_lowercase().as_str() {
            s if s.contains("hello") || s.contains("hi") => "Hey there! How can I help?".into(),
            s if s.contains("how are you") => "I'm doing great, thanks for asking!".into(),
            s if s.contains("bye") => "Goodbye! Have a great day!".into(),
            s if s.contains("help") => "I can echo your messages and respond to greetings!".into(),
            _ => format!("You said: {}", user_text),
        };

        self.messages.push(ChatMessage {
            sender: "Bot".into(),
            text: reply,
            is_user: false,
        });

        self.input.clear();
    }
}

enum Message {
    Send,
    AppendChar(char),
    DeleteChar,
}

impl Application for ChatApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::Send => {
                self.send_message();
                Cmd::none()
            }
            Message::AppendChar(c) => {
                self.input.push(c);
                Cmd::none()
            }
            Message::DeleteChar => {
                self.input.pop();
                Cmd::none()
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        match event {
            Event::Key(key_event) => match key_event.key {
                Key::Enter => Some(Message::Send),
                Key::Backspace => Some(Message::DeleteChar),
                Key::Char(c) => Some(Message::AppendChar(c)),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .flex_grow(1.0)
            .padding(1.0)
            .gap(1.0)
            .child(Title::new("Chat Demo"))
            .child(Divider::horizontal())
            .child(ScrollView::new().child(self.message_list()))
            .child(Divider::horizontal())
            .child(Text::new(format!("> {}|", self.input)))
            .child(Muted::new("Enter send · Ctrl+C quit"))
    }
}

impl ChatApp {
    fn message_list(&self) -> Column {
        let mut col = Column::new().flex_grow(1.0).gap(0.5);
        for msg in &self.messages {
            let color = if msg.is_user {
                Color::GREEN
            } else {
                Color::BLUE
            };
            col = col.child(RichText::new().span(
                format!("{}: {}", msg.sender, msg.text),
                Style::new().fg(color),
            ));
        }
        col
    }
}

fn main() -> anyhow::Result<()> {
    ChatApp::new().run()
}
