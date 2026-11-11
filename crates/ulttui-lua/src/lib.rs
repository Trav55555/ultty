//! Run ulttui apps written in Luau, with hot reload.
//!
//! A script returns an Elm-style app table (`init`, `update`, `view`,
//! `every`) built from the view constructors in [`node::CONSTRUCTORS`].
//! [`runner::LuaApp`] hosts it as a ulttui
//! [`Application`](ulttui::app::Application), so it renders natively inside
//! Ultty and as ANSI elsewhere. See `docs/lua.md` for the scripting API.

pub mod node;
pub mod runner;
pub mod script;

pub use runner::LuaApp;
pub use script::{Command, ExecGrant, Reload, Script, ScriptError, ScriptMsg, ScriptOptions};
