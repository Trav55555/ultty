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
    Every {
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

#[cfg(test)]
mod tests {
    use super::{Cmd, Sub};
    use std::time::Duration;

    #[test]
    fn cmd_none_is_default() {
        let cmd: Cmd<()> = Cmd::default();
        assert!(matches!(cmd, Cmd::None));
    }

    #[test]
    fn cmd_message_wraps_value() {
        let cmd = Cmd::message(7);
        match cmd {
            Cmd::Message(value) => assert_eq!(value, 7),
            _ => panic!("expected Cmd::Message"),
        }
    }

    #[test]
    fn cmd_batch_holds_multiple() {
        let cmd = Cmd::batch(vec![Cmd::none(), Cmd::message(1)]);
        match cmd {
            Cmd::Batch(cmds) => assert_eq!(cmds.len(), 2),
            _ => panic!("expected Cmd::Batch"),
        }
    }

    #[test]
    fn sub_none_is_default() {
        let sub: Sub<()> = Sub::default();
        assert!(matches!(sub, Sub::None));
    }

    #[test]
    fn sub_every_stores_interval() {
        let interval = Duration::from_millis(250);
        let sub = Sub::every(interval, || ());
        match sub {
            Sub::Every {
                interval: stored, ..
            } => assert_eq!(stored, interval),
            _ => panic!("expected Sub::Every"),
        }
    }

    #[test]
    fn sub_batch_holds_multiple() {
        let sub: Sub<()> = Sub::batch(vec![Sub::none(), Sub::none()]);
        match sub {
            Sub::Batch(subs) => assert_eq!(subs.len(), 2),
            _ => panic!("expected Sub::Batch"),
        }
    }

    #[test]
    fn cmd_subscribe_stores_id() {
        let stream = futures_util::stream::iter(vec![1, 2, 3]);
        let cmd = Cmd::subscribe("test-sub", stream);
        match cmd {
            Cmd::Subscribe { id, .. } => assert_eq!(id, "test-sub"),
            _ => panic!("expected Cmd::Subscribe"),
        }
    }

    #[test]
    fn cmd_unsubscribe_stores_id() {
        let cmd: Cmd<()> = Cmd::unsubscribe("my-sub");
        match cmd {
            Cmd::Unsubscribe { id } => assert_eq!(id, "my-sub"),
            _ => panic!("expected Cmd::Unsubscribe"),
        }
    }

    #[test]
    fn sub_stream_stores_id_and_url() {
        let sub: Sub<String> = Sub::stream("ws-sub", "ws://localhost:9090", |msg| {
            format!("got: {}", msg)
        });
        match sub {
            Sub::Stream { id, url, .. } => {
                assert_eq!(id, "ws-sub");
                assert_eq!(url, "ws://localhost:9090");
            }
            _ => panic!("expected Sub::Stream"),
        }
    }
}
