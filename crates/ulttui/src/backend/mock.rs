//! Mock backend for testing.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::event::Event;
use crate::widgets::{render_view_messages, View};
use ultty_protocol::AppMessage;

/// A mock backend that replays pre-programmed events and captures render calls.
///
/// - `poll_event` never sleeps; it calls `std::thread::yield_now()` so spawned
///   tokio tasks get a chance to run between ticks without wall-clock delays.
///   The event loop never sleeps either, unless the backend is
///   [`MockBackend::blocking_when_exhausted`]
/// - Captures the full native message tree of every render for verification
/// - Supports error injection via `inject_error()`
/// - Tracks all events dispatched for assertion
pub struct MockBackend {
    events: VecDeque<Option<Event>>,
    render_count: usize,
    /// Constraints captured from each render call (newest last).
    pub rendered_constraints: Vec<ultty_protocol::Constraints>,
    /// Content captured from each render call (newest last).
    pub rendered_content: Vec<ultty_protocol::Content>,
    /// Native protocol messages produced by walking the whole view tree on
    /// each render call (newest last).
    pub rendered_messages: Vec<Vec<AppMessage>>,
    /// Error to return on next `poll_event` call (one-shot).
    pending_error: Option<String>,
    /// When set, an exhausted event list yields `None` (idle) instead of
    /// `Quit`, so the app under test decides when to stop.
    idle_when_exhausted: bool,
    /// Idle polls returned since the event list ran out.
    idle_polls: usize,
    /// Once the events run out, report no input, so the event loop sleeps
    /// until a timer or a wake.
    blocking: bool,
    /// Every `poll_event` call, shared so a test can read it while the app
    /// runs.
    polls: Arc<AtomicUsize>,
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
            rendered_messages: Vec::new(),
            pending_error: None,
            dispatched_events: Vec::new(),
            idle_when_exhausted: false,
            idle_polls: 0,
            blocking: false,
            polls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Once the scripted events run out, behave like an idle terminal: no
    /// events and nothing to wake the event loop, which then sleeps until a
    /// timer is due or something wakes it. The test ends the run (a
    /// `Cmd::Quit` from the app, perhaps sent through a `Proxy`).
    pub fn blocking_when_exhausted(mut self) -> Self {
        self.blocking = true;
        self
    }

    /// The number of `poll_event` calls so far, readable while the app runs:
    /// one per pass of the event loop.
    pub fn poll_count(&self) -> Arc<AtomicUsize> {
        self.polls.clone()
    }

    /// Whether the event loop should poll again without sleeping.
    pub fn has_pending_events(&self) -> bool {
        !self.blocking || !self.events.is_empty() || self.pending_error.is_some()
    }

    /// Keep polling `None` after the scripted events run out instead of
    /// quitting, so a test can wait for asynchronous work to deliver a
    /// message and let the app quit itself. A safety cap of
    /// [`MockBackend::MAX_IDLE_POLLS`] idle polls still produces `Quit`, so a
    /// test that never quits fails instead of hanging.
    pub fn idle_until_quit(mut self) -> Self {
        self.idle_when_exhausted = true;
        self
    }

    /// Upper bound on idle polls after the event list is exhausted.
    pub const MAX_IDLE_POLLS: usize = 1_000_000;

    pub fn init(&mut self) -> crate::Result<()> {
        Ok(())
    }

    pub fn shutdown(&mut self) -> crate::Result<()> {
        Ok(())
    }

    pub fn render(&mut self, view: &impl View) -> crate::Result<()> {
        self.render_count += 1;
        self.rendered_constraints.push(view.constraints());
        self.rendered_content.push(view.content());
        self.rendered_messages.push(render_view_messages(view)?);
        Ok(())
    }

    pub fn poll_event(&mut self) -> crate::Result<Option<Event>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        // Check for injected error
        if let Some(err) = self.pending_error.take() {
            return Err(crate::Error::Mock(err));
        }

        // Give spawned tokio tasks a chance to run without a wall-clock sleep.
        std::thread::yield_now();

        let event = match self.events.pop_front() {
            Some(event) => event,
            None if self.blocking => None,
            None if self.idle_when_exhausted && self.idle_polls < Self::MAX_IDLE_POLLS => {
                self.idle_polls += 1;
                None
            }
            None => Some(Event::Quit),
        };
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

    /// Get the native message tree of the last render (if any).
    pub fn last_messages(&self) -> Option<&[AppMessage]> {
        self.rendered_messages.last().map(Vec::as_slice)
    }
}
