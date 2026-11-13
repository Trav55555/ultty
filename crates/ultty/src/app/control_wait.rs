//! Shared deferred control-plane wait matching.

#[cfg(test)]
pub(super) use ultty::control::wait::find_ignore_case;
pub(super) use ultty::control::wait::{Condition, PendingControlWait, WaitMatcher};
