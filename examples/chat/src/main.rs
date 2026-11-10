//! A chat window whose bot streams its replies word by word.
//!
//! The transcript is a polite live log, and the reply being streamed is a
//! `MarkdownView` in streaming mode: in Ultty its words reach the terminal
//! as appended text, which a screen reader announces as it arrives
//! (ADR 0021).

use std::time::Duration;

use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::RichText;
use ultty_protocol::{Live, Role};

/// How often the streaming reply grows by a word.
const WORD_INTERVAL: Duration = Duration::from_millis(40);

/// The reply to "markdown": blocks and inline styles, streamed like the
/// rest (scripts/e2e/accessibility-chat.sh checks what is announced).
const MARKDOWN_REPLY: &str = "Here is **bold** text and `inline code`.\n\n\
- first item\n\
- second item with [a link](https://example.com)\n\n\
```\nlet x = 1;\nlet y = 2;\n```\n\n\
That was markdown, streamed word by word.";

struct ChatMessage {
    sender: String,
    text: String,
    is_user: bool,
}

/// The bot reply being streamed: all of it, and how much is shown.
struct Streaming {
    text: String,
    shown: usize,
}

struct ChatApp {
    messages: Vec<ChatMessage>,
    input: String,
    reply: Option<Streaming>,
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
            reply: None,
        }
    }

    fn send_message(&mut self) {
        if self.input.is_empty() || self.reply.is_some() {
            return;
        }

        let user_text = std::mem::take(&mut self.input);
        let reply = match user_text.to_lowercase().as_str() {
            s if s.contains("hello") || s.contains("hi") => "Hey there! How can I help?".into(),
            s if s.contains("how are you") => "I'm doing great, thanks for asking!".into(),
            s if s.contains("bye") => "Goodbye! Have a great day!".into(),
            s if s.contains("help") => "I can echo your messages and respond to greetings!".into(),
            s if s.contains("markdown") => MARKDOWN_REPLY.into(),
            _ => format!(
                "You said: {user_text}.\n\nThat is {} characters, streamed to you word by word.",
                user_text.chars().count()
            ),
        };
        self.messages.push(ChatMessage {
            sender: "You".into(),
            text: user_text,
            is_user: true,
        });
        self.reply = Some(Streaming {
            text: reply,
            shown: 0,
        });
    }

    /// Show the next word of the reply; file it once it is all shown.
    fn stream_word(&mut self) {
        let Some(reply) = &mut self.reply else {
            return;
        };
        let rest = &reply.text[reply.shown..];
        let word_start = rest.len() - rest.trim_start().len();
        let word_end = rest[word_start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |end| word_start + end);
        reply.shown += word_end;
        if reply.shown == reply.text.len() {
            let text = std::mem::take(&mut reply.text);
            self.reply = None;
            self.messages.push(ChatMessage {
                sender: "Bot".into(),
                text,
                is_user: false,
            });
        }
    }
}

enum Message {
    Send,
    AppendChar(char),
    DeleteChar,
    StreamWord,
}

impl Application for ChatApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::Send => self.send_message(),
            Message::AppendChar(c) => self.input.push(c),
            Message::DeleteChar => {
                self.input.pop();
            }
            Message::StreamWord => self.stream_word(),
        }
        Cmd::none()
    }

    fn subscriptions(&self) -> Sub<Message> {
        if self.reply.is_some() {
            Sub::every(WORD_INTERVAL, || Message::StreamWord)
        } else {
            Sub::none()
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
            .child(ScrollView::new().child(self.transcript()))
            .child(Divider::horizontal())
            .child(
                Semantic::new(Text::new(format!("> {}|", self.input)))
                    .role(Role::Input)
                    .label("Message"),
            )
            .child(Muted::new("Enter send · Ctrl+C quit"))
    }
}

impl ChatApp {
    /// The conversation: a log, with the reply being streamed last.
    fn transcript(&self) -> Semantic<Column> {
        let mut col = Column::new().flex_grow(1.0).gap(0.5);
        for msg in &self.messages {
            let line = format!("{}: {}", msg.sender, msg.text);
            col = if msg.is_user {
                col.child(RichText::new().span(line, Style::new().fg(Color::GREEN)))
            } else {
                // The same view the reply streamed in, so the region it
                // streamed into stays.
                col.child(MarkdownView::new(line))
            };
        }
        if let Some(reply) = &self.reply {
            col = col.child(
                MarkdownView::new(format!("Bot: {}", &reply.text[..reply.shown]))
                    .streaming(true)
                    .label("Bot reply"),
            );
        }
        Semantic::new(col)
            .role(Role::Log)
            .label("Messages")
            .live(Live::Polite)
    }
}

fn main() -> anyhow::Result<()> {
    ChatApp::new().run()?;
    Ok(())
}
