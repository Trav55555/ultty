//! The ulttui application that hosts a [`Script`]: maps events to script
//! messages, runs the script's commands, and reloads it when its file changes.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};
use ratatui::layout::Rect;
use ratatui::Frame;
use tokio::sync::mpsc;
use ulttui::event::{Event, Key, KeyEvent};
use ulttui::prelude::*;
use ultty_protocol::{AppMessage, Content, RegionId};

use crate::node::{self, Node, PropValue};
use crate::script::{Command, Reload, Script, ScriptError, ScriptMsg, ScriptOptions};

/// Messages of the host app.
#[derive(Debug)]
pub enum Msg {
    /// First tick of the event loop: start the file watch and the first
    /// load's commands.
    Boot,
    /// Something for the script's `update`.
    Script(ScriptMsg),
    /// The script file changed; the instant is when the change was seen.
    FileChanged(Instant),
}

/// A Luau script running as a ulttui app.
pub struct LuaApp {
    script: Script,
    /// Commands from the first load, issued once the event loop runs.
    pending: Vec<Command>,
    watch_rx: Option<mpsc::UnboundedReceiver<Instant>>,
    /// Kept alive for as long as the app runs.
    _watcher: Option<notify::RecommendedWatcher>,
    booted: bool,
    /// Set by a reload; the next frame logs how long the change took to show.
    reload_mark: Cell<Option<ReloadMark>>,
}

/// When each step of a reload happened, for the latency log.
#[derive(Debug, Clone, Copy)]
struct ReloadMark {
    /// The watch saw the file change.
    seen: Instant,
    /// The app started reloading (after the settle delay and the event
    /// loop's next tick).
    started: Instant,
    /// Compile, `init` and the trial `view` were done.
    loaded: Instant,
}

const BOOT_TIMER: &str = "ulttui-lua/boot";
const WATCH_SUBSCRIPTION: &str = "ulttui-lua/watch";
/// After a change, wait this long for more writes before reloading, so an
/// editor's truncate-then-write does not show a half-written file.
const SETTLE: Duration = Duration::from_millis(10);

impl LuaApp {
    /// Load the script at `path` and watch it for changes.
    pub fn new(path: impl Into<PathBuf>, options: ScriptOptions) -> Result<Self, ScriptError> {
        let path = path.into();
        let (watcher, rx) = watch_file(&path)?;
        let mut app = Self::without_watch(path, options)?;
        app._watcher = Some(watcher);
        app.watch_rx = Some(rx);
        Ok(app)
    }

    /// Load the script without watching it (tests drive reloads directly).
    pub fn without_watch(path: PathBuf, options: ScriptOptions) -> Result<Self, ScriptError> {
        let mut script = Script::new(path, options)?;
        let pending = match script.reload() {
            Reload::Loaded(commands) => commands,
            Reload::Unchanged | Reload::Failed => Vec::new(),
        };
        Ok(Self {
            script,
            pending,
            watch_rx: None,
            _watcher: None,
            booted: false,
            reload_mark: Cell::new(None),
        })
    }

    pub fn script(&self) -> &Script {
        &self.script
    }

    /// Reload the script (as on a file change) and return its commands.
    pub fn reload(&mut self, seen: Instant) -> Vec<Command> {
        let started = Instant::now();
        let outcome = self.script.reload();
        if outcome != Reload::Unchanged {
            self.reload_mark.set(Some(ReloadMark {
                seen,
                started,
                loaded: Instant::now(),
            }));
        }
        match outcome {
            Reload::Loaded(commands) => {
                tracing::info!(script = %self.script.path().display(), "reloaded");
                commands
            }
            Reload::Failed => {
                tracing::info!(
                    script = %self.script.path().display(),
                    error = self.script.error().unwrap_or_default(),
                    "reload failed; keeping the last good version"
                );
                Vec::new()
            }
            Reload::Unchanged => Vec::new(),
        }
    }

    /// The whole frame: the error banner (if any) over the script's view.
    pub fn frame(&self) -> FrameView {
        let node = self.script.view();
        let mut root = Column::new()
            .height(Dimension::Percent(100.0))
            .flex_grow(1.0);
        if let Some(error) = self.script.error() {
            root = root.child(node::build(&error_banner(&error, self.script.is_loaded())));
        }
        if let Some(node) = node {
            root = root.child(node::build(&node));
        }
        FrameView {
            root,
            mark: self.reload_mark.take(),
        }
    }
}

fn error_banner(error: &str, still_running: bool) -> Node {
    let mut lines = vec![Rc::new(Node {
        props: BTreeMap::from([
            ("fg", PropValue::Color(Color::RED)),
            ("bold", PropValue::Bool(true)),
        ]),
        ..Node::text(format!("error: {error}"))
    })];
    if still_running {
        lines.push(Rc::new(Node::text(
            "still running the last version that loaded; save a fix to reload",
        )));
    }
    Node {
        kind: "column",
        props: BTreeMap::from([
            ("id", PropValue::Str("ulttui-lua-error".to_string())),
            ("border", PropValue::Str("thin".to_string())),
            ("border_color", PropValue::Color(Color::RED)),
            ("shrink", PropValue::Num(0.0)),
        ]),
        text: None,
        children: lines,
    }
}

/// Watch the directory holding `path` and report changes to that one file.
///
/// The directory, not the file, is watched because editors save by writing a
/// new file and renaming it over the old one, which ends a watch on the file.
fn watch_file(
    path: &Path,
) -> Result<(notify::RecommendedWatcher, mpsc::UnboundedReceiver<Instant>), ScriptError> {
    let path = std::fs::canonicalize(path).map_err(|source| ScriptError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    // A canonical path to a file always has a parent.
    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let watch_error = |source| ScriptError::Watch {
        path: path.clone(),
        source,
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let target = path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
            && event.paths.iter().any(|p| p == &target);
        if relevant {
            let _ = tx.send(Instant::now());
        }
    })
    .map_err(watch_error)?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(watch_error)?;
    Ok((watcher, rx))
}

/// A stream of `FileChanged` messages, one per burst of writes.
fn change_stream(
    rx: mpsc::UnboundedReceiver<Instant>,
) -> impl futures_util::Stream<Item = Msg> + Send + 'static {
    futures_util::stream::unfold(rx, |mut rx| async move {
        let first = rx.recv().await?;
        while let Ok(Some(_)) = tokio::time::timeout(SETTLE, rx.recv()).await {}
        Some((Msg::FileChanged(first), rx))
    })
}

fn to_cmd(commands: Vec<Command>) -> Cmd<Msg> {
    let cmds: Vec<Cmd<Msg>> = commands
        .into_iter()
        .map(|command| match command {
            Command::Quit => Cmd::Quit,
            Command::Run { id, argv } => {
                // Same id: the previous run is cancelled (and killed).
                Cmd::spawn_with_id(format!("run:{id}"), run_process(id, argv))
            }
        })
        .collect();
    match cmds.len() {
        0 => Cmd::none(),
        1 => cmds.into_iter().next().unwrap_or_default(),
        _ => Cmd::batch(cmds),
    }
}

async fn run_process(id: String, argv: Vec<String>) -> Msg {
    let output = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    let done = match output {
        Ok(output) => ScriptMsg::Done {
            id,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code(),
            error: None,
        },
        Err(e) => ScriptMsg::Done {
            id,
            stdout: String::new(),
            stderr: String::new(),
            status: None,
            error: Some(format!("{}: {e}", argv[0])),
        },
    };
    Msg::Script(done)
}

/// Name of a key as scripts see it.
pub fn key_name(key: &Key) -> String {
    match key {
        Key::Char(' ') => "space".to_string(),
        Key::Char(c) => c.to_string(),
        Key::Enter => "enter".to_string(),
        Key::Escape => "escape".to_string(),
        Key::Backspace => "backspace".to_string(),
        Key::Tab => "tab".to_string(),
        Key::Up => "up".to_string(),
        Key::Down => "down".to_string(),
        Key::Left => "left".to_string(),
        Key::Right => "right".to_string(),
        Key::Home => "home".to_string(),
        Key::End => "end".to_string(),
        Key::PageUp => "pageup".to_string(),
        Key::PageDown => "pagedown".to_string(),
        Key::Delete => "delete".to_string(),
        Key::Insert => "insert".to_string(),
        Key::F(n) => format!("f{n}"),
    }
}

fn key_msg(event: &KeyEvent) -> ScriptMsg {
    ScriptMsg::Key {
        key: key_name(&event.key),
        ctrl: event.modifiers.ctrl,
        alt: event.modifiers.alt,
        shift: event.modifiers.shift,
        focused: event.focused_id.clone(),
    }
}

impl Application for LuaApp {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Boot => {
                self.booted = true;
                let mut cmds = vec![to_cmd(std::mem::take(&mut self.pending))];
                if let Some(rx) = self.watch_rx.take() {
                    cmds.push(Cmd::subscribe(WATCH_SUBSCRIPTION, change_stream(rx)));
                }
                Cmd::batch(cmds)
            }
            Msg::Script(msg) => to_cmd(self.script.update(&msg)),
            Msg::FileChanged(seen) => to_cmd(self.reload(seen)),
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        let mut subs: Vec<Sub<Msg>> = self
            .script
            .timers()
            .into_iter()
            .map(|(name, interval)| {
                Sub::every_with_id(format!("timer:{name}"), interval, move || {
                    Msg::Script(ScriptMsg::Tick { name: name.clone() })
                })
            })
            .collect();
        if !self.booted {
            // ulttui has no init hook; a zero-interval timer fires on the
            // first tick and is dropped once `Boot` is handled.
            subs.push(Sub::every_with_id(BOOT_TIMER, Duration::ZERO, || Msg::Boot));
        }
        Sub::batch(subs)
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            // Tab and Shift+Tab stay with ulttui's focus navigation.
            Event::Key(key) if key.key == Key::Tab => None,
            Event::Key(key) => Some(Msg::Script(key_msg(key))),
            Event::Click { id } => Some(Msg::Script(ScriptMsg::Click { id: id.clone() })),
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        self.frame()
    }
}

/// The root view of a frame. When the frame follows a reload, dropping it
/// (which the runner does right after the backend has drawn it) logs how long
/// the change took to reach the screen.
pub struct FrameView {
    root: Column,
    mark: Option<ReloadMark>,
}

impl Drop for FrameView {
    fn drop(&mut self) {
        let Some(mark) = self.mark else { return };
        let now = Instant::now();
        let unix_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros());
        let us = |from: Instant, to: Instant| to.saturating_duration_since(from).as_micros() as u64;
        tracing::info!(
            target: "ulttui_lua::reload",
            seen_to_frame_us = us(mark.seen, now),
            wait_us = us(mark.seen, mark.started),
            reload_us = us(mark.started, mark.loaded),
            render_us = us(mark.loaded, now),
            frame_unix_us = unix_us as u64,
            "frame after reload"
        );
    }
}

impl View for FrameView {
    fn constraints(&self) -> ultty_protocol::Constraints {
        self.root.constraints()
    }

    fn style(&self) -> ultty_protocol::Style {
        self.root.style()
    }

    fn content(&self) -> Content {
        self.root.content()
    }

    fn children_count(&self) -> usize {
        View::children_count(&self.root)
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> ulttui::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> ulttui::Result<Option<RegionId>> {
        self.root
            .render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        self.root.render_ansi(frame, area)
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        self.root.for_each_child(visit)
    }
}
