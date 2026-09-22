//! Command and subscription types for side effects.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::screen::Screen;
use crate::toast::Toast;

/// A command representing a side effect to execute after update().
#[derive(Default)]
pub enum Cmd<M> {
    /// No side effect.
    #[default]
    None,
    /// Immediately dispatch another message.
    Message(M),
    /// Execute multiple commands.
    Batch(Vec<Cmd<M>>),
    /// Spawn an async task that produces a message.
    Spawn(Pin<Box<dyn Future<Output = M> + Send + 'static>>),
    /// Spawn an async task that can later be cancelled by `id`.
    ///
    /// Spawning again with the same `id` aborts the previous task first.
    SpawnWithId {
        id: String,
        future: Pin<Box<dyn Future<Output = M> + Send + 'static>>,
    },
    /// Abort a task started with `Cmd::spawn_with_id`. No-op if it already
    /// finished or was never started.
    CancelSpawn {
        id: String,
    },
    /// Dispatch a message after a delay.
    After(Duration, Box<dyn Fn() -> M + Send + 'static>),
    /// Subscribe to a long-lived async stream of messages.
    ///
    /// Each item yielded by the stream is dispatched through `update()`.
    /// The stream runs until it completes, errors, or is cancelled via
    /// `Cmd::Unsubscribe`. If a subscription with the same `id` already
    /// exists, the old one is cancelled before the new one starts.
    Subscribe {
        id: String,
        stream: Pin<Box<dyn futures_core::Stream<Item = M> + Send + 'static>>,
    },
    /// Cancel an active subscription by ID.
    Unsubscribe {
        id: String,
    },
    ShowToast(Toast),
    PushScreen(Box<dyn Screen>),
    PopScreen,
    /// Request clean application shutdown.
    Quit,
}

impl<M> Cmd<M> {
    /// No-op command.
    pub fn none() -> Self {
        Cmd::None
    }

    /// Dispatch a message immediately.
    pub fn message(msg: M) -> Self {
        Cmd::Message(msg)
    }

    /// Execute multiple commands.
    pub fn batch(cmds: Vec<Cmd<M>>) -> Self {
        Cmd::Batch(cmds)
    }

    /// Spawn an async task.
    pub fn spawn<F>(future: F) -> Self
    where
        F: Future<Output = M> + Send + 'static,
    {
        Cmd::Spawn(Box::pin(future))
    }

    /// Spawn an async task that `Cmd::cancel(id)` can abort.
    pub fn spawn_with_id<F>(id: impl Into<String>, future: F) -> Self
    where
        F: Future<Output = M> + Send + 'static,
    {
        Cmd::SpawnWithId {
            id: id.into(),
            future: Box::pin(future),
        }
    }

    /// Cancel a task started with `Cmd::spawn_with_id`.
    pub fn cancel(id: impl Into<String>) -> Self {
        Cmd::CancelSpawn { id: id.into() }
    }

    /// Dispatch a message after a delay.
    pub fn after<F>(duration: Duration, f: F) -> Self
    where
        F: Fn() -> M + Send + 'static,
    {
        Cmd::After(duration, Box::new(f))
    }

    /// Subscribe to a long-lived async stream of messages.
    ///
    /// The stream will be polled continuously and each item dispatched
    /// through `update()`. Use `Cmd::unsubscribe(id)` to cancel.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// // Create a stream from any async source (WebSocket, SSE, file watcher, etc.)
    /// let stream = my_websocket_stream.map(|msg| Msg::StreamData(msg));
    /// Cmd::subscribe("my-stream", stream)
    /// ```
    pub fn subscribe<S>(id: impl Into<String>, stream: S) -> Self
    where
        S: futures_core::Stream<Item = M> + Send + 'static,
    {
        Cmd::Subscribe {
            id: id.into(),
            stream: Box::pin(stream),
        }
    }

    /// Cancel an active subscription.
    pub fn unsubscribe(id: impl Into<String>) -> Self {
        Cmd::Unsubscribe { id: id.into() }
    }
}

impl<M> std::fmt::Debug for Cmd<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Cmd::None => "Cmd::None",
            Cmd::Message(_) => "Cmd::Message",
            Cmd::Batch(_) => "Cmd::Batch",
            Cmd::Spawn(_) => "Cmd::Spawn",
            Cmd::SpawnWithId { .. } => "Cmd::SpawnWithId",
            Cmd::CancelSpawn { .. } => "Cmd::CancelSpawn",
            Cmd::After(_, _) => "Cmd::After",
            Cmd::Subscribe { .. } => "Cmd::Subscribe",
            Cmd::Unsubscribe { .. } => "Cmd::Unsubscribe",
            Cmd::ShowToast(_) => "Cmd::ShowToast",
            Cmd::PushScreen(_) => "Cmd::PushScreen",
            Cmd::PopScreen => "Cmd::PopScreen",
            Cmd::Quit => "Cmd::Quit",
        };
        f.write_str(name)
    }
}

/// A subscription to external events.
#[derive(Default)]
pub enum Sub<M> {
    /// No subscription.
    #[default]
    None,
    /// Tick at a regular interval.
    ///
    /// Timer state (the last tick) is tracked per timer identity across
    /// frames. With an explicit `id` the identity is that id; without one it
    /// is the interval plus the timer's position among unkeyed timers with
    /// the same interval, so a conditional sibling timer appearing or
    /// disappearing does not reset unrelated timers.
    Every {
        id: Option<String>,
        interval: Duration,
        f: Box<dyn Fn() -> M + Send + 'static>,
    },
    /// Subscribe to a WebSocket stream.
    Stream {
        id: String,
        url: String,
        on_message: Box<dyn Fn(String) -> M + Send + 'static>,
    },
    /// Multiple subscriptions.
    Batch(Vec<Sub<M>>),
}

impl<M> Sub<M> {
    pub fn none() -> Self {
        Sub::None
    }

    pub fn every<F>(interval: Duration, f: F) -> Self
    where
        F: Fn() -> M + Send + 'static,
    {
        Sub::Every {
            id: None,
            interval,
            f: Box::new(f),
        }
    }

    /// Tick at a regular interval, with an explicit identity.
    ///
    /// Use this when the same timer may move position in the subscription
    /// tree between frames (for example next to conditional subscriptions)
    /// and must keep its phase.
    pub fn every_with_id<F>(id: impl Into<String>, interval: Duration, f: F) -> Self
    where
        F: Fn() -> M + Send + 'static,
    {
        Sub::Every {
            id: Some(id.into()),
            interval,
            f: Box::new(f),
        }
    }

    /// Subscribe to a WebSocket stream.
    pub fn stream<F>(id: impl Into<String>, url: impl Into<String>, on_message: F) -> Self
    where
        F: Fn(String) -> M + Send + 'static,
    {
        Sub::Stream {
            id: id.into(),
            url: url.into(),
            on_message: Box::new(on_message),
        }
    }

    pub fn batch(subs: Vec<Sub<M>>) -> Self {
        Sub::Batch(subs)
    }
}
