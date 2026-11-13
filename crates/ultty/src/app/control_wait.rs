//! Shared deferred control-plane wait matching.

pub(super) use ultty::control::wait::{
    find_ignore_case, Condition, PendingControlWait, WaitMatcher,
};
