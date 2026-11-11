//! The Luau sandbox shared by `ulttui-lua` apps and Luau override files
//! (ADR 0023, Luau amendment). Only built with the `lua-overrides` feature.
//!
//! A [`Sandbox`] is a Luau VM with only the libraries that cannot reach
//! outside the process (`string`, `table`, `math`, `utf8`, `bit32`,
//! `buffer`, `coroutine`, and Luau's `os`, which has no `execute`), without
//! `require`, `loadstring`, `getfenv` or `setfenv`, with its globals frozen
//! by Luau's sandbox mode, a memory cap, and a time limit on every call made
//! through [`Sandbox::guarded`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use mlua;
use mlua::{Lua, LuaOptions, StdLib, Table, Value, VmState};

/// The message of the error a call over its time limit fails with.
pub const TOO_LONG: &str = "script took too long (stuck in a loop?)";

/// A sandboxed Luau VM and the deadline its interrupt enforces.
pub struct Sandbox {
    lua: Lua,
    deadline: Rc<Cell<Option<Instant>>>,
}

impl Sandbox {
    /// A new VM capped at `memory_limit` bytes. `install` adds the host's
    /// globals before they are frozen.
    pub fn new(
        memory_limit: usize,
        install: impl FnOnce(&Lua, &Table) -> mlua::Result<()>,
    ) -> mlua::Result<Self> {
        let libs = StdLib::COROUTINE
            | StdLib::TABLE
            | StdLib::STRING
            | StdLib::UTF8
            | StdLib::BIT
            | StdLib::MATH
            | StdLib::BUFFER
            | StdLib::OS;
        let lua = Lua::new_with(libs, LuaOptions::default())?;
        let globals = lua.globals();
        // Luau has no `io` and its `os` cannot run programs; these two would
        // still load other code from disk or from strings.
        globals.raw_set("require", Value::Nil)?;
        globals.raw_set("loadstring", Value::Nil)?;
        // These would hand a script environment tables past its own.
        globals.raw_set("getfenv", Value::Nil)?;
        globals.raw_set("setfenv", Value::Nil)?;
        install(&lua, &globals)?;
        // Freezes the globals and the libraries.
        lua.sandbox(true)?;
        lua.set_memory_limit(memory_limit)?;

        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let watch = deadline.clone();
        // Luau checks at loop back-edges and calls, so a loop that never
        // ends is stopped; a single slow library call can overrun.
        lua.set_interrupt(move |_| match watch.get() {
            Some(deadline) if Instant::now() > deadline => Err(mlua::Error::runtime(TOO_LONG)),
            _ => Ok(VmState::Continue),
        });
        Ok(Self { lua, deadline })
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Run `f` with at most `budget` of Luau time; past it the running Luau
    /// code fails with [`TOO_LONG`].
    pub fn guarded<R>(
        &self,
        budget: Duration,
        f: impl FnOnce() -> mlua::Result<R>,
    ) -> mlua::Result<R> {
        let previous = self.deadline.replace(Some(Instant::now() + budget));
        let result = f();
        self.deadline.set(previous);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_world_is_out_of_reach() {
        let sandbox = Sandbox::new(1 << 20, |_, _| Ok(())).unwrap();
        for name in ["io", "require", "loadstring", "getfenv", "setfenv", "debug"] {
            let value: Value = sandbox.lua().globals().get(name).unwrap();
            assert!(value.is_nil(), "{name} is reachable");
        }
        let execute: Value = sandbox.lua().load("return os.execute").eval().unwrap();
        assert!(execute.is_nil());
    }

    #[test]
    fn a_loop_that_never_ends_is_stopped() {
        let sandbox = Sandbox::new(1 << 20, |_, _| Ok(())).unwrap();
        let started = Instant::now();
        let error = sandbox
            .guarded(Duration::from_millis(20), || {
                sandbox.lua().load("while true do end").exec()
            })
            .unwrap_err();
        assert!(error.to_string().contains(TOO_LONG), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
