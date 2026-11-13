//! The Luau side: a sandboxed VM holding the running script, its state, and
//! the last version that loaded cleanly.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use mlua::{Function, Lua, Table, UserData, Value};
use ulttui::lua_sandbox::Sandbox;

use crate::node::{self, located, Node};

/// Errors setting up the VM or the file watch. Errors in the script itself
/// are not errors here: they are shown in the app (see [`Script::error`]).
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// mlua's error is not `Send` without its `error-send` feature, so only
    /// the message is kept.
    #[error("Luau setup failed: {0}")]
    Lua(String),
    #[error("cannot watch {path}: {source}")]
    Watch {
        path: PathBuf,
        source: notify::Error,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl From<mlua::Error> for ScriptError {
    fn from(error: mlua::Error) -> Self {
        ScriptError::Lua(error.to_string())
    }
}

/// How a script may run.
#[derive(Debug, Clone)]
pub struct ScriptOptions {
    /// Which programs `run{}` may start.
    pub exec: ExecGrant,
    /// Longest one call into the script (load, init, update, view) may take
    /// before it is stopped with an error.
    pub call_budget: Duration,
}

impl Default for ScriptOptions {
    fn default() -> Self {
        Self {
            exec: ExecGrant::Deny,
            call_budget: Duration::from_secs(1),
        }
    }
}

/// What `run{}` may start (ADR 0024 §3). Fixed for the life of the script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExecGrant {
    /// Nothing: every `run{}` is an error.
    #[default]
    Deny,
    /// Command lines starting with one of these word lists, compared to
    /// `argv` exactly as the script wrote it (`--exec 'ulttyctl --json
    /// panes'`). A single word grants the program with any arguments, which
    /// for `git` is broad; for a program whose job is to run other programs
    /// it would be [`ExecGrant::Any`], so [`ExecGrant::parse_grant`] refuses
    /// those ([`PROGRAM_RUNNERS`]).
    Only(Vec<Vec<String>>),
    /// Anything (`--allow-exec`).
    Any,
}

pub use ultty_protocol::exec_grant::{NEED_SUBCOMMAND, PROGRAM_RUNNERS};

impl ExecGrant {
    /// The words of one `--exec` grant, or why it is refused: the shared
    /// [`ultty_protocol::exec_grant::parse`], which `ulttyctl lua open`
    /// also checks before opening a pane.
    pub fn parse_grant(spec: &str) -> Result<Vec<String>, String> {
        ultty_protocol::exec_grant::parse(spec)
    }

    /// Whether `run{}` may start `argv` (as written).
    pub fn allows(&self, argv: &[String]) -> bool {
        match self {
            ExecGrant::Deny => false,
            ExecGrant::Only(prefixes) => prefixes.iter().any(|p| argv.starts_with(p)),
            ExecGrant::Any => true,
        }
    }

    /// The error `run{}` raises for a command line this grant does not
    /// allow, naming the flag that would.
    fn denial(&self, argv: &[String]) -> String {
        let mut message = format!(
            "{} is not allowed; open the script with --exec {}",
            argv.join(" "),
            quote_words(argv)
        );
        let mut notes = Vec::new();
        if argv.len() > 1 {
            notes.push("or a shorter prefix".to_string());
        }
        if let ExecGrant::Only(prefixes) = self {
            if !prefixes.is_empty() {
                let allowed: Vec<String> = prefixes.iter().map(|p| quote_words(p)).collect();
                notes.push(format!("allowed: {}", allowed.join(", ")));
            }
        }
        if !notes.is_empty() {
            message.push_str(&format!(" ({})", notes.join("; ")));
        }
        message
    }
}

/// `words` as one shell argument: bare for a single word, quoted otherwise.
fn quote_words(words: &[String]) -> String {
    match words {
        [one] => one.clone(),
        many => format!("'{}'", many.join(" ")),
    }
}

/// A side effect the script asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run a process; its result comes back as a `done` message with `id`.
    Run { id: String, argv: Vec<String> },
    /// Exit the app.
    Quit,
}

#[derive(Debug, Clone)]
struct CommandHandle(Command);

impl UserData for CommandHandle {}

/// A message for the script's `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptMsg {
    Key {
        key: String,
        ctrl: bool,
        alt: bool,
        shift: bool,
        focused: Option<String>,
    },
    /// The focused `input` was edited: the key moved its cursor or changed
    /// its text. `cursor` counts grapheme clusters before the cursor.
    Input {
        id: String,
        value: String,
        cursor: usize,
    },
    /// An explicitly declared semantic action was dispatched.
    Action {
        id: String,
        action: ulttui::SemanticActionKind,
    },
    /// A widget was clicked (native mode).
    Click { id: String },
    /// A named timer from the app's `every` table fired.
    Tick { name: String },
    /// A process started with `run{}` finished.
    Done {
        id: String,
        stdout: String,
        stderr: String,
        /// Exit code; `None` if it was killed by a signal or never started.
        status: Option<i32>,
        /// Why it could not be started.
        error: Option<String>,
    },
}

/// The functions of a loaded app table.
struct AppFns {
    init: Option<Function>,
    update: Option<Function>,
    view: Function,
    every: Vec<(String, Duration)>,
    actions: std::collections::HashSet<String>,
}

const APP_KEYS: &[&str] = &["init", "update", "view", "every", "actions"];

/// What a reload did.
#[derive(Debug, PartialEq, Eq)]
pub enum Reload {
    /// The new version is running; these are the commands its `init` returned.
    Loaded(Vec<Command>),
    /// The file has not changed since the last attempt.
    Unchanged,
    /// The new version failed; the previous one (if any) keeps running and
    /// [`Script::error`] says why.
    Failed,
}

/// A Luau app script.
pub struct Script {
    lua: Lua,
    path: PathBuf,
    chunk_name: String,
    app: Option<AppFns>,
    state: Value,
    /// Source of the last load attempt, good or bad.
    source: Option<String>,
    /// Why the last load failed. Cleared by the next good load.
    load_error: Option<String>,
    /// Why the last update failed. Cleared by the next update that works
    /// and by a good load.
    update_error: Option<String>,
    /// Why the last view call failed; cleared by the next one that works.
    view_error: RefCell<Option<String>>,
    /// The last view that built, shown while `view` is failing.
    last_view: RefCell<Option<Rc<Node>>>,
    /// The frozen global table: what every version's environment reads
    /// through to.
    base: Table,
    /// The VM `lua` belongs to, and its time limit.
    sandbox: Sandbox,
    budget: Duration,
}

impl Script {
    /// Set up a VM for the script at `path`. Nothing is loaded until
    /// [`Script::reload`].
    pub fn new(path: impl Into<PathBuf>, options: ScriptOptions) -> Result<Self, ScriptError> {
        let path = path.into();
        // The sandbox (ulttui's, shared with Luau override files) freezes the
        // globals once the API is in: scripts write to their own environment
        // (see `try_load`).
        let sandbox = Sandbox::new(256 * 1024 * 1024, |lua, globals| {
            install_api(lua, globals, options.exec)
        })?;
        let lua = sandbox.lua().clone();
        let globals = lua.globals();

        Ok(Self {
            lua,
            chunk_name: format!("={}", path.display()),
            path,
            app: None,
            state: Value::Nil,
            source: None,
            load_error: None,
            update_error: None,
            view_error: RefCell::new(None),
            last_view: RefCell::new(None),
            base: globals,
            sandbox,
            budget: options.call_budget,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a version of the script has loaded.
    pub fn is_loaded(&self) -> bool {
        self.app.is_some()
    }

    /// The current error, if the last load or update failed or the view is
    /// failing.
    pub fn error(&self) -> Option<String> {
        self.load_error
            .clone()
            .or_else(|| self.update_error.clone())
            .or_else(|| self.view_error.borrow().clone())
    }

    /// The app's state value.
    pub fn state(&self) -> &Value {
        &self.state
    }

    /// The app's named timers.
    pub fn timers(&self) -> Vec<(String, Duration)> {
        self.app
            .as_ref()
            .map(|app| app.every.clone())
            .unwrap_or_default()
    }

    /// Read the file and, if it changed, swap in the new version.
    ///
    /// The new version must evaluate to an app table, `init(prev_state)`
    /// must return a state, and `view(state)` must return a view; otherwise
    /// the old version keeps running with its state.
    pub fn reload(&mut self) -> Reload {
        let source = match std::fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(e) => {
                self.load_error = Some(format!("{}: {e}", self.path.display()));
                return Reload::Failed;
            }
        };
        if self.source.as_deref() == Some(source.as_str()) {
            return Reload::Unchanged;
        }
        let result = self.try_load(&source);
        self.source = Some(source);
        match result {
            Ok((app, state, commands, view)) => {
                self.app = Some(app);
                self.state = state;
                self.load_error = None;
                self.update_error = None;
                *self.view_error.borrow_mut() = None;
                *self.last_view.borrow_mut() = Some(view);
                Reload::Loaded(commands)
            }
            Err(message) => {
                self.load_error = Some(message);
                Reload::Failed
            }
        }
    }

    fn try_load(&self, source: &str) -> Result<(AppFns, Value, Vec<Command>, Rc<Node>), String> {
        // A fresh environment per load, reading through to the frozen
        // globals, so globals one version set do not leak into the next.
        // `__metatable` hides the metatable from `getmetatable`.
        let env = self.lua.create_table().map_err(|e| e.to_string())?;
        let meta = self.lua.create_table().map_err(|e| e.to_string())?;
        meta.set("__index", self.base.clone())
            .map_err(|e| e.to_string())?;
        meta.set("__metatable", false).map_err(|e| e.to_string())?;
        env.set_metatable(Some(meta)).map_err(|e| e.to_string())?;

        let value: Value = self
            .guarded(|| {
                self.lua
                    .load(source)
                    .set_name(&self.chunk_name)
                    .set_environment(env)
                    .eval()
            })
            .map_err(|e| describe_error(&e))?;
        let app = app_fns(value).map_err(|e| format!("{}: {e}", self.path.display()))?;

        let (state, commands) = match &app.init {
            Some(init) => {
                // A copy, so an init that changes `prev` and then fails (or
                // whose view fails) leaves the running version's state alone.
                let prev = deep_copy(&self.lua, &self.state, &mut HashMap::new())
                    .map_err(|e| e.to_string())?;
                let (state, cmds): (Value, Value) = self
                    .guarded(|| init.call(prev))
                    .map_err(|e| format!("init: {}", describe_error(&e)))?;
                if state.is_nil() {
                    return Err("init: returned nil; it must return the state".to_string());
                }
                (
                    state,
                    commands_from(cmds).map_err(|e| format!("init: {e}"))?,
                )
            }
            None if self.state.is_nil() => (
                Value::Table(self.lua.create_table().map_err(|e| e.to_string())?),
                Vec::new(),
            ),
            None => (self.state.clone(), Vec::new()),
        };
        let view = self.call_view(&app.view, &state)?;
        Ok((app, state, commands, view))
    }

    /// Whether the loaded app explicitly declares activation for this ID.
    pub fn supports_activation(&self, widget_id: &str) -> bool {
        self.app
            .as_ref()
            .is_some_and(|app| app.actions.contains(widget_id))
    }

    /// Run `update(state, msg)`. On error the state value is kept; changes
    /// the update made to it in place before failing stay.
    pub fn update(&mut self, msg: &ScriptMsg) -> Vec<Command> {
        let Some(update) = self.app.as_ref().and_then(|app| app.update.clone()) else {
            return Vec::new();
        };
        let result = msg_table(&self.lua, msg).and_then(|msg| {
            self.guarded(|| update.call::<(Value, Value)>((self.state.clone(), msg)))
        });
        match result {
            Ok((state, cmds)) => {
                if !state.is_nil() {
                    self.state = state;
                }
                match commands_from(cmds) {
                    Ok(commands) => {
                        self.update_error = None;
                        commands
                    }
                    Err(e) => {
                        self.update_error = Some(format!("update: {e}"));
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                self.update_error = Some(format!("update: {}", describe_error(&e)));
                Vec::new()
            }
        }
    }

    /// Build the current view. If `view` fails, the error is recorded and the
    /// last view that worked is returned.
    pub fn view(&self) -> Option<Rc<Node>> {
        let app = self.app.as_ref()?;
        match self.call_view(&app.view, &self.state) {
            Ok(node) => {
                *self.view_error.borrow_mut() = None;
                *self.last_view.borrow_mut() = Some(node.clone());
                Some(node)
            }
            Err(message) => {
                *self.view_error.borrow_mut() = Some(message);
                self.last_view.borrow().clone()
            }
        }
    }

    fn call_view(&self, view: &Function, state: &Value) -> Result<Rc<Node>, String> {
        let value: Value = self
            .guarded(|| view.call(state.clone()))
            .map_err(|e| format!("view: {}", describe_error(&e)))?;
        let node = match value {
            Value::UserData(ud) => node::node_from_userdata(&ud)
                .ok_or_else(|| "view: returned a userdata that is not a view".to_string())?,
            Value::String(s) => Rc::new(Node::text(s.to_string_lossy())),
            other => {
                return Err(format!(
                    "view: returned {}; it must return a view such as column{{...}}",
                    other.type_name()
                ))
            }
        };
        match node.duplicate_focus_id() {
            Some(id) => Err(format!(
                "view: more than one button or input has the id \"{id}\"; \
                 their ids must be unique"
            )),
            None => Ok(node),
        }
    }

    /// Run `f` with the per-call time budget armed.
    fn guarded<R>(&self, f: impl FnOnce() -> mlua::Result<R>) -> mlua::Result<R> {
        self.sandbox.guarded(self.budget, f)
    }

    /// The text and cursor of the input `id` in the last view that built.
    pub fn input_state(&self, id: &str) -> Option<(String, usize)> {
        self.last_view.borrow().as_ref()?.input_state(id)
    }

    /// Evaluate `expr` against the state, for tests and tools.
    pub fn eval_with_state(&self, expr: &str) -> mlua::Result<Value> {
        let f: Function = self
            .lua
            .load(format!("return function(state) return {expr} end"))
            .eval()?;
        f.call(self.state.clone())
    }
}

/// Copy tables recursively (sharing and cycles preserved; metatables and
/// non-table values are shared).
fn deep_copy(
    lua: &Lua,
    value: &Value,
    seen: &mut HashMap<*const std::ffi::c_void, Table>,
) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value.clone());
    };
    if let Some(copy) = seen.get(&table.to_pointer()) {
        return Ok(Value::Table(copy.clone()));
    }
    let copy = lua.create_table()?;
    seen.insert(table.to_pointer(), copy.clone());
    for pair in table.pairs::<Value, Value>() {
        let (k, v) = pair?;
        copy.raw_set(deep_copy(lua, &k, seen)?, deep_copy(lua, &v, seen)?)?;
    }
    copy.set_metatable(table.metatable())?;
    Ok(Value::Table(copy))
}

fn app_fns(value: Value) -> Result<AppFns, String> {
    let Value::Table(table) = value else {
        return Err(format!(
            "the script returned {}; it must end with `return app {{ ... }}`",
            value.type_name()
        ));
    };
    check_app_table(&table)?;
    let function = |name: &str| -> Result<Option<Function>, String> {
        match table.get::<Value>(name).map_err(|e| e.to_string())? {
            Value::Nil => Ok(None),
            Value::Function(f) => Ok(Some(f)),
            other => Err(format!(
                "app.{name} must be a function, got {}",
                other.type_name()
            )),
        }
    };
    let view = function("view")?.ok_or("the app has no `view` function")?;
    let mut every = Vec::new();
    match table.get::<Value>("every").map_err(|e| e.to_string())? {
        Value::Nil => {}
        Value::Table(timers) => {
            for pair in timers.pairs::<Value, Value>() {
                let (name, ms) = pair.map_err(|e| e.to_string())?;
                let ms = match ms {
                    Value::Integer(i) => Some(i as f64),
                    Value::Number(n) => Some(n),
                    _ => None,
                };
                let (Value::String(name), Some(ms)) = (&name, ms) else {
                    return Err(
                        "app.every maps timer names to milliseconds, like { refresh = 2000 }"
                            .to_string(),
                    );
                };
                if !(ms.is_finite() && ms >= 10.0) {
                    return Err(format!(
                        "app.every.{}: {ms} ms is too short (at least 10)",
                        name.to_string_lossy()
                    ));
                }
                every.push((name.to_string_lossy(), Duration::from_secs_f64(ms / 1000.0)));
            }
            every.sort();
        }
        other => {
            return Err(format!(
                "app.every must be a table, got {}",
                other.type_name()
            ))
        }
    }
    let mut actions = std::collections::HashSet::new();
    match table
        .get::<Value>("actions")
        .map_err(|error| error.to_string())?
    {
        Value::Nil => {}
        Value::Table(declarations) => {
            for pair in declarations.pairs::<String, String>() {
                let (widget_id, action) = pair.map_err(|error| error.to_string())?;
                if !ultty_protocol::valid_action_widget_id(&widget_id)
                    || action != "activate"
                    || actions.len() >= ultty_protocol::MAX_REGION_COUNT
                {
                    return Err("app.actions must map bounded widget IDs to 'activate'".into());
                }
                actions.insert(widget_id);
            }
        }
        _ => return Err("app.actions must be a table mapping widget IDs to 'activate'".into()),
    }
    Ok(AppFns {
        init: function("init")?,
        update: function("update")?,
        view,
        every,
        actions,
    })
}

fn check_app_table(table: &Table) -> Result<(), String> {
    for pair in table.pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|e| e.to_string())?;
        let known =
            matches!(&key, Value::String(k) if APP_KEYS.contains(&k.to_string_lossy().as_str()));
        if !known {
            let key = match &key {
                Value::String(k) => k.to_string_lossy(),
                other => other.type_name().to_string(),
            };
            return Err(format!(
                "unknown app field '{key}' (expected one of: {})",
                APP_KEYS.join(", ")
            ));
        }
    }
    Ok(())
}

/// Commands from the second value `init`/`update` returned: nothing, one
/// command, or an array of them.
fn commands_from(value: Value) -> Result<Vec<Command>, String> {
    fn one(value: &Value) -> Option<Command> {
        match value {
            Value::UserData(ud) => ud.borrow::<CommandHandle>().ok().map(|c| c.0.clone()),
            _ => None,
        }
    }
    match &value {
        Value::Nil => Ok(Vec::new()),
        Value::Table(list) => {
            let mut commands = Vec::new();
            for item in list.sequence_values::<Value>() {
                let item = item.map_err(|e| e.to_string())?;
                commands.push(one(&item).ok_or_else(|| {
                    format!(
                        "command list item {} is a {}; expected run{{...}} or quit()",
                        commands.len() + 1,
                        item.type_name()
                    )
                })?);
            }
            Ok(commands)
        }
        other => one(other).map(|c| vec![c]).ok_or_else(|| {
            format!(
                "the second return value is a {}; expected run{{...}}, quit() or a list of them",
                other.type_name()
            )
        }),
    }
}

fn msg_table(lua: &Lua, msg: &ScriptMsg) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    match msg {
        ScriptMsg::Key {
            key,
            ctrl,
            alt,
            shift,
            focused,
        } => {
            t.set("type", "key")?;
            t.set("key", key.as_str())?;
            t.set("ctrl", *ctrl)?;
            t.set("alt", *alt)?;
            t.set("shift", *shift)?;
            t.set("focused", focused.as_deref())?;
        }
        ScriptMsg::Input { id, value, cursor } => {
            t.set("type", "input")?;
            t.set("id", id.as_str())?;
            t.set("value", value.as_str())?;
            t.set("cursor", *cursor)?;
        }
        ScriptMsg::Action { id, action } => {
            t.set("type", "action")?;
            t.set("id", id.as_str())?;
            t.set("action", action.as_str())?;
        }
        ScriptMsg::Click { id } => {
            t.set("type", "click")?;
            t.set("id", id.as_str())?;
        }
        ScriptMsg::Tick { name } => {
            t.set("type", "tick")?;
            t.set("name", name.as_str())?;
        }
        ScriptMsg::Done {
            id,
            stdout,
            stderr,
            status,
            error,
        } => {
            t.set("type", "done")?;
            t.set("id", id.as_str())?;
            t.set("stdout", stdout.as_str())?;
            t.set("stderr", stderr.as_str())?;
            t.set("status", *status)?;
            t.set("error", error.as_deref())?;
        }
    }
    Ok(t)
}

/// The message of an error without Rust-side wrapping: Luau already puts
/// `file:line:` in front of runtime and syntax errors, and `located` does the
/// same for errors raised by the API.
pub fn describe_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, .. } => describe_error(cause),
        // Luau appends a traceback; the first line already says where.
        mlua::Error::RuntimeError(message) => message
            .split_once("\nstack traceback:")
            .map_or(message.as_str(), |(head, _)| head)
            .to_string(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(message) => format!("out of memory: {message}"),
        other => other.to_string(),
    }
}

fn install_api(lua: &Lua, globals: &Table, exec: ExecGrant) -> mlua::Result<()> {
    node::register(lua, globals)?;

    // The terminal is the UI: `print` goes to the log, with where it was called.
    globals.set(
        "print",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let parts = args
                .iter()
                .map(|v| v.to_string())
                .collect::<mlua::Result<Vec<_>>>()?;
            let place = lua
                .inspect_stack(1, |debug| {
                    let source = debug.source().short_src.map(|s| s.into_owned());
                    format!(
                        "{}:{}",
                        source.unwrap_or_default(),
                        debug.current_line().unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            tracing::info!(target: "ulttui_lua::print", at = %place, "{}", parts.join("\t"));
            Ok(())
        })?,
    )?;

    globals.set(
        "app",
        lua.create_function(|lua, table: Value| match &table {
            Value::Table(t) => {
                check_app_table(t).map_err(|e| located(lua, format!("app: {e}")))?;
                Ok(table)
            }
            other => Err(located(
                lua,
                format!("app: expected a table, got {}", other.type_name()),
            )),
        })?,
    )?;

    globals.set(
        "run",
        lua.create_function(move |lua, spec: Value| {
            let command = parse_run(spec).map_err(|e| located(lua, format!("run: {e}")))?;
            if let Command::Run { argv, .. } = &command {
                if !exec.allows(argv) {
                    return Err(located(lua, format!("run: {}", exec.denial(argv))));
                }
            }
            Ok(CommandHandle(command))
        })?,
    )?;

    globals.set(
        "quit",
        lua.create_function(|_, ()| Ok(CommandHandle(Command::Quit)))?,
    )?;

    let json = lua.create_table()?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::LuaString| {
            let parsed: serde_json::Value = serde_json::from_slice(&text.as_bytes())
                .map_err(|e| located(lua, format!("json.decode: {e}")))?;
            json_to_lua(lua, &parsed)
        })?,
    )?;
    globals.set("json", json)?;
    Ok(())
}

fn parse_run(spec: Value) -> Result<Command, String> {
    let Value::Table(spec) = spec else {
        return Err(format!(
            "expected a table like {{ id = \"x\", argv = {{ \"ls\" }} }}, got {}",
            spec.type_name()
        ));
    };
    let mut id = None;
    let mut argv = None;
    for pair in spec.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|e| e.to_string())?;
        let key = match &key {
            Value::String(k) => k.to_string_lossy(),
            other => return Err(format!("unexpected key of type {}", other.type_name())),
        };
        match (key.as_str(), value) {
            ("id", Value::String(s)) => id = Some(s.to_string_lossy()),
            ("argv", Value::Table(list)) => {
                let mut items = Vec::new();
                for item in list.sequence_values::<Value>() {
                    match item.map_err(|e| e.to_string())? {
                        Value::String(s) => items.push(s.to_string_lossy()),
                        other => {
                            return Err(format!(
                                "argv[{}] is a {}; expected a string",
                                items.len() + 1,
                                other.type_name()
                            ))
                        }
                    }
                }
                argv = Some(items);
            }
            ("id", other) | ("argv", other) => {
                return Err(format!("{key} has the wrong type ({})", other.type_name()))
            }
            (other, _) => return Err(format!("unknown field '{other}' (expected id, argv)")),
        }
    }
    let id = id.ok_or("missing id")?;
    let argv = argv
        .filter(|a| !a.is_empty())
        .ok_or("argv must name a program")?;
    Ok(Command::Run { id, argv })
}

/// JSON to Lua: objects and arrays become tables, `null` becomes `nil`.
fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        serde_json::Value::Object(fields) => {
            let t = lua.create_table_with_capacity(0, fields.len())?;
            for (k, v) in fields {
                t.raw_set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}
