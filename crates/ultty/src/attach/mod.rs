//! Bounded viewer attachment to a persistent local session owner.
//!
//! This protocol is distinct from application-native transport and privileged
//! local control. Viewer loss never owns or closes application transports.

pub mod auth;
pub mod control;
pub mod host;
pub mod model;
pub mod native;
pub mod remote;
pub mod transport;
pub mod view;
pub mod wire;
