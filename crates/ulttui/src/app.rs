//! Application trait and runner.

use crate::backend::Backend;
use crate::widgets::View;

/// Main application trait.
pub trait Application: Sized {
    /// Message type for state updates.
    type Message;

    /// Update state based on a message.
    fn update(&mut self, _msg: Self::Message) {}

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
}

impl<A: Application> App<A> {
    /// Create a new app runner.
    pub fn new(app: A) -> Self {
        Self {
            app,
            backend: Backend::detect(),
        }
    }

    /// Run the main loop.
    pub fn run(mut self) -> anyhow::Result<()> {
        self.backend.init()?;

        loop {
            // Render
            let view = self.app.view();
            self.backend.render(&view)?;

            // Handle events
            if let Some(event) = self.backend.poll_event()? {
                match event {
                    crate::event::Event::Quit => break,
                    crate::event::Event::Key(_) => {
                        // TODO: Route to focused widget
                    }
                    _ => {}
                }
            }
        }

        self.backend.shutdown()?;
        Ok(())
    }
}
