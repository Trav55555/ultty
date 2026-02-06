//! Mock backend for testing.

use std::collections::VecDeque;

use crate::event::Event;
use crate::widgets::View;

/// A mock backend that replays pre-programmed events and captures render calls.
pub struct MockBackend {
    events: VecDeque<Option<Event>>,
    render_count: usize,
}

impl MockBackend {
    pub fn new(events: Vec<Option<Event>>) -> Self {
        Self {
            events: events.into(),
            render_count: 0,
        }
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn render(&mut self, _view: &impl View) -> anyhow::Result<()> {
        self.render_count += 1;
        Ok(())
    }

    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        // Small yield to let tokio runtime schedule spawned tasks
        // (mirrors the 16ms poll timeout in the real ANSI backend)
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(self.events.pop_front().unwrap_or(Some(Event::Quit)))
    }

    pub fn render_count(&self) -> usize {
        self.render_count
    }
}
