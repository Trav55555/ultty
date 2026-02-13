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
    ShowToast(Toast),
    PushScreen(Box<dyn Screen>),
    PopScreen,
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
}

impl<M> std::fmt::Debug for Cmd<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Cmd::None => "Cmd::None",
            Cmd::Message(_) => "Cmd::Message",
            Cmd::Batch(_) => "Cmd::Batch",
            Cmd::Spawn(_) => "Cmd::Spawn",
            Cmd::After(_, _) => "Cmd::After",
            Cmd::ShowToast(_) => "Cmd::ShowToast",
            Cmd::PushScreen(_) => "Cmd::PushScreen",
            Cmd::PopScreen => "Cmd::PopScreen",
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
}
