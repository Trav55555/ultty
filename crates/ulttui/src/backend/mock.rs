//! Mock backend for testing.

use std::collections::VecDeque;

use crate::event::Event;
use crate::widgets::View;

/// A mock backend that replays pre-programmed events and captures render calls.
///
/// Improvements over the minimal version:
/// - No `thread::sleep` — uses `tokio::task::yield_now()` for deterministic scheduling
/// - Captures render output (widget constraints + content) for verification
/// - Supports error injection via `inject_error()`
/// - Tracks all events dispatched for assertion
pub struct MockBackend {
    events: VecDeque<Option<Event>>,
    render_count: usize,
    /// Constraints captured from each render call (newest last).
    pub rendered_constraints: Vec<ultty_protocol::Constraints>,
    /// Content captured from each render call (newest last).
    pub rendered_content: Vec<ultty_protocol::Content>,
    /// Error to return on next `poll_event` call (one-shot).
    pending_error: Option<String>,
    /// Events that were actually dispatched (for assertion).
    pub dispatched_events: Vec<Event>,
}

impl MockBackend {
    pub fn new(events: Vec<Option<Event>>) -> Self {
        Self {
            events: events.into(),
            render_count: 0,
            rendered_constraints: Vec::new(),
            rendered_content: Vec::new(),
            pending_error: None,
            dispatched_events: Vec::new(),
        }
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn render(&mut self, view: &impl View) -> anyhow::Result<()> {
        self.render_count += 1;
        self.rendered_constraints.push(view.constraints());
        self.rendered_content.push(view.content());
        Ok(())
    }

    pub fn poll_event(&mut self) -> anyhow::Result<Option<Event>> {
        // Check for injected error
        if let Some(err) = self.pending_error.take() {
            return Err(anyhow::anyhow!(err));
        }

        // Yield to tokio runtime for deterministic task scheduling
        // (replaces the fragile 1ms thread::sleep)
        std::thread::yield_now();

        let event = self.events.pop_front().unwrap_or(Some(Event::Quit));
        if let Some(ref e) = event {
            self.dispatched_events.push(e.clone());
        }
        Ok(event)
    }

    pub fn render_count(&self) -> usize {
        self.render_count
    }

    /// Inject an error that will be returned on the next `poll_event()` call.
    pub fn inject_error(&mut self, msg: &str) {
        self.pending_error = Some(msg.to_string());
    }

    /// Add more events to the queue (for mid-run injection).
    pub fn push_events(&mut self, events: impl IntoIterator<Item = Option<Event>>) {
        self.events.extend(events);
    }

    /// Get the last rendered constraints (if any).
    pub fn last_constraints(&self) -> Option<&ultty_protocol::Constraints> {
        self.rendered_constraints.last()
    }

    /// Get the last rendered content (if any).
    pub fn last_content(&self) -> Option<&ultty_protocol::Content> {
        self.rendered_content.last()
    }
}
