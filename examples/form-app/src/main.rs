use ulttui::event::{Event, Key};
use ulttui::prelude::*;
use ulttui::widgets::RichText;

// ---------------------------------------------------------------------------
// Field validation
// ---------------------------------------------------------------------------

enum FieldValidator {
    Required,
    Email,
    Numeric,
}

impl FieldValidator {
    fn validate(&self, value: &str) -> Option<String> {
        match self {
            FieldValidator::Required => {
                if value.trim().is_empty() {
                    Some("This field is required".into())
                } else {
                    None
                }
            }
            FieldValidator::Email => {
                if value.trim().is_empty() {
                    Some("Email is required".into())
                } else if !value.contains('@') {
                    Some("Email must contain @".into())
                } else {
                    None
                }
            }
            FieldValidator::Numeric => {
                if value.trim().is_empty() {
                    Some("Age is required".into())
                } else if value.parse::<u32>().is_err() {
                    Some("Age must be a number".into())
                } else {
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Form field
// ---------------------------------------------------------------------------

struct FormField {
    label: String,
    value: String,
    validator: FieldValidator,
    error: Option<String>,
}

impl FormField {
    fn new(label: impl Into<String>, validator: FieldValidator) -> Self {
        Self {
            label: label.into(),
            value: String::new(),
            validator,
            error: None,
        }
    }

    fn revalidate(&mut self) {
        self.error = self.validator.validate(&self.value);
    }

    fn is_valid(&self) -> bool {
        self.validator.validate(&self.value).is_none()
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct FormApp {
    fields: Vec<FormField>,
    active_field: usize,
    submitted: bool,
}

impl FormApp {
    fn new() -> Self {
        Self {
            fields: vec![
                FormField::new("Name", FieldValidator::Required),
                FormField::new("Email", FieldValidator::Email),
                FormField::new("Age", FieldValidator::Numeric),
            ],
            active_field: 0,
            submitted: false,
        }
    }

    fn all_valid(&self) -> bool {
        self.fields.iter().all(|f| f.is_valid())
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

enum Message {
    FocusNext,
    FocusPrev,
    AppendChar(char),
    DeleteChar,
    Submit,
}

// ---------------------------------------------------------------------------
// Application trait
// ---------------------------------------------------------------------------

impl Application for FormApp {
    type Message = Message;

    fn update(&mut self, msg: Message) -> Cmd<Message> {
        match msg {
            Message::FocusNext => {
                if self.active_field < self.fields.len() - 1 {
                    self.fields[self.active_field].revalidate();
                    self.active_field += 1;
                }
                Cmd::none()
            }
            Message::FocusPrev => {
                if self.active_field > 0 {
                    self.fields[self.active_field].revalidate();
                    self.active_field -= 1;
                }
                Cmd::none()
            }
            Message::AppendChar(c) => {
                self.fields[self.active_field].value.push(c);
                self.fields[self.active_field].revalidate();
                Cmd::none()
            }
            Message::DeleteChar => {
                self.fields[self.active_field].value.pop();
                self.fields[self.active_field].revalidate();
                Cmd::none()
            }
            Message::Submit => {
                // Validate every field before submitting.
                for field in &mut self.fields {
                    field.revalidate();
                }
                if self.all_valid() {
                    self.submitted = true;
                }
                Cmd::none()
            }
        }
    }

    fn on_event(&mut self, event: &Event) -> Option<Message> {
        if self.submitted {
            return None;
        }
        match event {
            Event::Key(key_event) => match key_event.key {
                Key::Tab => {
                    if key_event.modifiers.shift {
                        Some(Message::FocusPrev)
                    } else {
                        Some(Message::FocusNext)
                    }
                }
                Key::Down => Some(Message::FocusNext),
                Key::Up => Some(Message::FocusPrev),
                Key::Backspace => Some(Message::DeleteChar),
                Key::Enter => Some(Message::Submit),
                Key::Char(c) => Some(Message::AppendChar(c)),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        if self.submitted {
            return self.summary_view();
        }
        self.form_view()
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

impl FormApp {
    fn form_view(&self) -> Column {
        let mut col = Column::new().padding(1.0).gap(1.0);

        // Title
        col = col.child(RichText::new().span("Form Demo", Style::new().bold().fg(Color::CYAN)));
        col = col.child(Text::new(
            "Fill in the fields below. Tab/Shift+Tab or Up/Down to navigate.",
        ));

        // Fields
        for (i, field) in self.fields.iter().enumerate() {
            let is_active = i == self.active_field;
            col = col.child(self.field_row(field, is_active));
        }

        // Submit hint
        let submit_style = if self.all_valid() {
            Style::new().fg(Color::GREEN)
        } else {
            Style::new().fg(Color::rgb(100, 100, 100))
        };
        col = col.child(RichText::new().span(
            if self.all_valid() {
                "Press Enter to submit"
            } else {
                "Fix errors above before submitting"
            },
            submit_style,
        ));

        // Key hints
        col = col.child(RichText::new().span(
            "Tab/Shift+Tab: navigate | Enter: submit | Ctrl+C: quit",
            Style {
                dim: true,
                ..Style::default()
            },
        ));

        col
    }

    fn field_row(&self, field: &FormField, is_active: bool) -> Column {
        let mut col = Column::new().gap(0.0);

        // Label + input row
        let cursor = if is_active { "|" } else { "" };
        let arrow = if is_active { "> " } else { "  " };

        let label_style = if is_active {
            Style::new().bold().fg(Color::CYAN)
        } else {
            Style::new().fg(Color::WHITE)
        };

        let value_color = match (&field.error, field.value.is_empty()) {
            (Some(_), _) => Color::RED,
            (None, true) => Color::rgb(100, 100, 100),
            (None, false) => Color::GREEN,
        };

        let display_value = if field.value.is_empty() && !is_active {
            "(empty)".to_string()
        } else {
            format!("{}{}", field.value, cursor)
        };

        col = col.child(
            RichText::new()
                .span(format!("{}{}: ", arrow, field.label), label_style)
                .span(display_value, Style::new().fg(value_color)),
        );

        // Error message (if any)
        if let Some(ref error) = field.error {
            col = col.child(RichText::new().span(
                format!("     {} {}", "!", error),
                Style::new().fg(Color::RED),
            ));
        }

        col
    }

    fn summary_view(&self) -> Column {
        let mut col = Column::new().padding(1.0).gap(1.0);

        col = col
            .child(RichText::new().span("Form Submitted!", Style::new().bold().fg(Color::GREEN)));

        col = col.child(Text::new("---"));

        for field in &self.fields {
            col = col.child(
                RichText::new()
                    .span(format!("{}: ", field.label), Style::new().bold())
                    .span(&field.value, Style::new().fg(Color::CYAN)),
            );
        }

        col = col.child(Text::new("---"));
        col = col.child(RichText::new().span(
            "Press Ctrl+C to quit",
            Style {
                dim: true,
                ..Style::default()
            },
        ));

        col
    }
}

fn main() -> anyhow::Result<()> {
    FormApp::new().run()
}
