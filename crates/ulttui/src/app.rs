//! Application trait and runner.

use crate::backend::Backend;
use crate::event::{Event, Key, KeyEvent};
use crate::widgets::View;

/// Main application trait.
pub trait Application: Sized {
    /// Message type for state updates.
    type Message;

    /// Update state based on a message.
    fn update(&mut self, _msg: Self::Message) {}

    /// Handle an event and optionally return a message.
    fn on_event(&mut self, _event: &Event) -> Option<Self::Message> {
        None
    }

    /// Render the UI.
    fn view(&self) -> impl View;

    /// Run the application.
    fn run(self) -> anyhow::Result<()> {
        App::new(self).run()
    }
}

/// Application runner.
pub struct App<A: Application> {
    app: A,
    backend: Backend,
    focus_index: usize,
}

impl<A: Application> App<A> {
    /// Create a new app runner.
    pub fn new(app: A) -> Self {
        Self {
            app,
            backend: Backend::detect(),
            focus_index: 0,
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        self.backend.init()?;

        loop {
            {
                let view = self.app.view();
                self.backend.render(&view)?;
            }

            if let Some(event) = self.backend.poll_event()? {
                match &event {
                    Event::Quit => break,
                    Event::Key(key_event) => {
                        if self.handle_navigation(key_event) {
                            continue;
                        }
                    }
                    _ => {}
                }

                if let Some(msg) = self.app.on_event(&event) {
                    self.app.update(msg);
                }
            }
        }

        self.backend.shutdown()?;
        Ok(())
    }

    fn handle_navigation(&mut self, key_event: &KeyEvent) -> bool {
        match key_event.key {
            Key::Tab if !key_event.modifiers.shift => {
                self.focus_index = self.focus_index.saturating_add(1);
                true
            }
            Key::Tab if key_event.modifiers.shift => {
                self.focus_index = self.focus_index.saturating_sub(1);
                true
            }
            _ => false,
        }
    }
}
