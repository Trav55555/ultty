//! The ulttui application that hosts a [`Script`]: maps events to script
//! messages, runs the script's commands, and reloads it when its file changes.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::event::{AccessKind, AccessMode, ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher};
use ratatui::layout::Rect;
use ratatui::Frame;
use ulttui::event::{Event, Key, KeyEvent};
use ulttui::prelude::*;
use ultty_protocol::{AppMessage, Content, RegionId};

use crate::node::{self, Node, PropValue};
use crate::script::{Command, Reload, Script, ScriptError, ScriptMsg, ScriptOptions};

/// Messages of the host app.
#[derive(Debug)]
pub enum Msg {
    /// Something for the script's `update`.
    Script(ScriptMsg),
    /// The script file was written and closed, or renamed into place: it
    /// is complete, so reload now. The instant is when the change was seen.
    FileChanged(Instant),
    /// The script file changed in a way that may be the middle of a write
    /// (a truncate, one write of several): reload once it has been quiet for
    /// [`SETTLE`], unless a [`Msg::FileChanged`] comes first.
    FileTouched(Instant),
}

/// A Luau script running as a ulttui app.
pub struct LuaApp {
    script: Script,
    /// Commands from the first load, issued by `init`.
    pending: Vec<Command>,
    /// Set by a reload; the next frame logs how long the change took to show.
    reload_mark: Cell<Option<ReloadMark>>,
}

/// When each step of a reload happened, for the latency log.
#[derive(Debug, Clone, Copy)]
struct ReloadMark {
    /// The watch saw the file change.
    seen: Instant,
    /// The app started reloading (after the event loop woke, and any
    /// settle delay).
    started: Instant,
    /// Compile, `init` and the trial `view` were done.
    loaded: Instant,
}

/// The task that reloads after a [`Msg::FileTouched`].
const SETTLE_TASK: &str = "ulttui-lua/settle";
/// After a change that may be partial, wait this long for more writes before
/// reloading, so a truncate-then-write does not show a half-written file.
/// Only a fallback: on Linux the close after the write (or the rename into
/// place) reloads at once.
pub const SETTLE: Duration = Duration::from_millis(10);

impl LuaApp {
    /// Load the script at `path`. [`watch_file`] reports its changes.
    pub fn new(path: impl Into<PathBuf>, options: ScriptOptions) -> Result<Self, ScriptError> {
        let mut script = Script::new(path.into(), options)?;
        let pending = match script.reload() {
            Reload::Loaded(commands) => commands,
            Reload::Unchanged | Reload::Failed => Vec::new(),
        };
        Ok(Self {
            script,
            pending,
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

    /// What a key does to the focused `input`, if it is an editing key.
    /// `None` means the key is not for an input (the script gets it as a
    /// `key` message); `Some(None)` means it is, but changes nothing, such as
    /// Left at the start.
    fn input_edit(&self, key: &KeyEvent) -> Option<Option<ScriptMsg>> {
        let id = key.focused_id.as_deref()?;
        let (value, cursor) = self.script.input_state(id)?;
        let op = ulttui::edit::op_for_key(key)?;
        let (new_value, new_cursor) = ulttui::edit::apply(&value, cursor, &op);
        if new_value == value && new_cursor == cursor {
            return Some(None);
        }
        Some(Some(ScriptMsg::Input {
            id: id.to_string(),
            value: new_value,
            cursor: new_cursor,
        }))
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

/// A running watch on the script's file; dropping it stops the watch.
pub struct FileWatch {
    _watcher: notify::RecommendedWatcher,
}

/// Watch the directory holding `path` and hand each change to that one file
/// to `on_change`, on the watcher's thread, as [`Msg::FileChanged`] or
/// [`Msg::FileTouched`]. Pass it to the running app with a
/// [`Proxy`](ulttui::app::Proxy), which wakes the event loop.
///
/// The directory, not the file, is watched because editors save by writing a
/// new file and renaming it over the old one, which ends a watch on the file.
pub fn watch_file(
    path: &Path,
    on_change: impl Fn(Msg) + Send + 'static,
) -> Result<FileWatch, ScriptError> {
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
    let target = path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if !event.paths.iter().any(|p| p == &target) {
            return;
        }
        let seen = Instant::now();
        match change_kind(&event.kind) {
            Some(Change::Complete) => on_change(Msg::FileChanged(seen)),
            Some(Change::Partial) => on_change(Msg::FileTouched(seen)),
            None => {}
        }
    })
    .map_err(watch_error)?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(watch_error)?;
    Ok(FileWatch { _watcher: watcher })
}

#[derive(Debug, PartialEq)]
enum Change {
    /// The file's new contents are all there.
    Complete,
    /// More writes may follow.
    Partial,
}

fn change_kind(kind: &EventKind) -> Option<Change> {
    match kind {
        // Written and closed (inotify's IN_CLOSE_WRITE), or renamed into
        // place by an editor's save.
        EventKind::Access(AccessKind::Close(AccessMode::Write))
        | EventKind::Modify(ModifyKind::Name(
            RenameMode::To | RenameMode::Both | RenameMode::Any,
        )) => Some(Change::Complete),
        // Renamed away: nothing to load until something takes its place.
        EventKind::Modify(ModifyKind::Name(_)) => None,
        // A create is followed by its writes; a modify may be one of
        // several. Platforms without close events only ever send these.
        EventKind::Create(_) | EventKind::Modify(_) => Some(Change::Partial),
        _ => None,
    }
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

fn key_msg(event: &KeyEvent) -> ScriptMsg {
    ScriptMsg::Key {
        key: event.key.name(),
        ctrl: event.modifiers.ctrl,
        alt: event.modifiers.alt,
        shift: event.modifiers.shift,
        focused: event.focused_id.clone(),
    }
}

impl Application for LuaApp {
    type Message = Msg;

    fn init(&mut self) -> Cmd<Msg> {
        to_cmd(std::mem::take(&mut self.pending))
    }

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Script(msg) => to_cmd(self.script.update(&msg)),
            Msg::FileChanged(seen) => {
                // A settle still pending would only find the file unchanged.
                Cmd::batch(vec![Cmd::cancel(SETTLE_TASK), to_cmd(self.reload(seen))])
            }
            Msg::FileTouched(seen) => {
                // A later touch replaces this task, restarting the quiet time.
                Cmd::spawn_with_id(SETTLE_TASK, async move {
                    tokio::time::sleep(SETTLE).await;
                    Msg::FileChanged(seen)
                })
            }
        }
    }

    fn subscriptions(&self) -> Sub<Msg> {
        Sub::batch(
            self.script
                .timers()
                .into_iter()
                .map(|(name, interval)| {
                    Sub::every_with_id(format!("timer:{name}"), interval, move || {
                        Msg::Script(ScriptMsg::Tick { name: name.clone() })
                    })
                })
                .collect(),
        )
    }

    fn supports_action(&self, widget_id: &str, action: ulttui::SemanticActionKind) -> bool {
        action == ulttui::SemanticActionKind::Activate && self.script.supports_activation(widget_id)
    }

    fn on_action(&mut self, widget_id: &str, action: ulttui::SemanticActionKind) -> Option<Msg> {
        self.supports_action(widget_id, action).then(|| {
            Msg::Script(ScriptMsg::Action {
                id: widget_id.to_owned(),
                action,
            })
        })
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            // Tab and Shift+Tab stay with ulttui's focus navigation.
            Event::Key(key) if key.key == Key::Tab => None,
            Event::Key(key) => match self.input_edit(key) {
                Some(edit) => edit.map(Msg::Script),
                None => Some(Msg::Script(key_msg(key))),
            },
            Event::Paste { text, focused_id } => {
                let id = focused_id.as_deref()?;
                let (value, cursor) = self.script.input_state(id)?;
                // Lua input nodes are single-line.
                let text = ulttui::widgets::TextInput::normalize_paste(text, false);
                let (value, cursor) =
                    ulttui::edit::apply(&value, cursor, &ulttui::edit::EditOp::Insert(text));
                Some(Msg::Script(ScriptMsg::Input {
                    id: id.to_owned(),
                    value,
                    cursor,
                }))
            }
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

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};

    use super::*;

    #[test]
    fn closes_and_renames_into_place_are_complete_other_writes_are_not() {
        let complete = [
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
        ];
        for kind in complete {
            assert_eq!(change_kind(&kind), Some(Change::Complete), "{kind:?}");
        }
        let partial = [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            EventKind::Modify(ModifyKind::Any),
        ];
        for kind in partial {
            assert_eq!(change_kind(&kind), Some(Change::Partial), "{kind:?}");
        }
        let ignored = [
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
            EventKind::Remove(RemoveKind::File),
        ];
        for kind in ignored {
            assert_eq!(change_kind(&kind), None, "{kind:?}");
        }
    }
}
