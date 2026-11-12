//! Per-app user overrides keyed by widget id (ADR 0023).
//!
//! A user reshapes an app they did not write with a TOML file:
//!
//! ```toml
//! [widget.status]
//! fg = "yellow"
//! bold = true
//!
//! [widget.sidebar]
//! hidden = true
//!
//! [widget.submit]
//! text = "Send"
//!
//! [keys]
//! "j" = "down"
//! "/" = { focus = "search" }
//! ```
//!
//! [`App`](crate::app::App) finds the file ([`OverrideFile::for_app`]),
//! rereads it when it changes, remaps keys before the app sees them, and
//! makes the overrides active while it renders. The view walks
//! ([`render_view_ansi`](crate::widgets::render_view_ansi) and the native
//! one) apply them to every widget whose [`View::widget_id`] has an entry.
//!
//! A file that does not parse is logged and ignored: overrides never stop
//! an app.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use ultty_protocol::{Color, Content, OverrideFormat, PushedOverride, Style};

use crate::event::{Key, KeyEvent};
use crate::widgets::View;

/// Turns overrides off when set to anything but empty or `0`.
pub const DISABLE_ENV: &str = "ULTTUI_NO_OVERRIDES";
/// A path to read overrides from instead of the app's own file.
pub const PATH_ENV: &str = "ULTTUI_OVERRIDES";

/// What a user changed about one widget.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetOverride {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: Option<bool>,
    /// Not drawn, no room in a column or row, not focusable.
    pub hidden: bool,
    /// Replaces the text of a text-bearing widget.
    pub text: Option<Rc<str>>,
}

impl WidgetOverride {
    fn has_style(&self) -> bool {
        self.fg.is_some() || self.bg.is_some() || self.bold.is_some()
    }

    /// `style` with the overridden fields put over it.
    pub fn apply_style(&self, mut style: Style) -> Style {
        if let Some(fg) = self.fg {
            style.fg = Some(fg);
        }
        if let Some(bg) = self.bg {
            style.bg = Some(bg);
        }
        if let Some(bold) = self.bold {
            style.bold = bold;
        }
        style
    }

    /// `content` with the overridden style put over each rich-text span, so
    /// the override wins over colours the widget chose for its text.
    pub fn apply_content(&self, content: Content) -> Content {
        match content {
            Content::RichText(mut rich) if self.has_style() => {
                for span in &mut rich.spans {
                    span.style = self.apply_style(std::mem::take(&mut span.style));
                }
                Content::RichText(rich)
            }
            other => other,
        }
    }

    /// The ratatui style to paint over the widget's area, if any.
    pub(crate) fn ratatui_style(&self) -> Option<ratatui::style::Style> {
        use ratatui::style::{Modifier, Style as RStyle};
        if !self.has_style() {
            return None;
        }
        let mut style = RStyle::default();
        if let Some(fg) = self.fg {
            style = style.fg(crate::theme::to_ratatui_color(fg));
        }
        if let Some(bg) = self.bg {
            style = style.bg(crate::theme::to_ratatui_color(bg));
        }
        match self.bold {
            Some(true) => style = style.add_modifier(Modifier::BOLD),
            Some(false) => style = style.remove_modifier(Modifier::BOLD),
            None => {}
        }
        Some(style)
    }
}

/// A key with the modifiers it must be pressed with.
///
/// Shift is not compared for character keys: the character already says
/// it (`"A"`, `"?"`), and terminals disagree on whether they report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyChord {
    /// Parse `ctrl+alt+shift+<key>` (modifiers in any order, any subset);
    /// `<key>` is a [`Key::name`].
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut chord = KeyChord {
            key: Key::Enter,
            ctrl: false,
            alt: false,
            shift: false,
        };
        // A lone "+" (or a chord ending in "++") names the plus key.
        let (mods, key) = match spec.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None if spec == "+" => ("", "+"),
            None => spec.rsplit_once('+').unwrap_or(("", spec)),
        };
        for modifier in mods.split('+').filter(|m| !m.is_empty()) {
            match modifier {
                "ctrl" => chord.ctrl = true,
                "alt" => chord.alt = true,
                "shift" => chord.shift = true,
                other => return Err(format!("'{other}' in '{spec}' is not ctrl, alt or shift")),
            }
        }
        chord.key = Key::from_name(key).ok_or_else(|| format!("'{key}' is not a key name"))?;
        if matches!(chord.key, Key::Char(_)) {
            chord.shift = false;
        }
        Ok(chord)
    }

    /// Whether `event` is this chord. A key held with meta (super) is
    /// never matched: no chord can name it.
    fn matches(&self, event: &KeyEvent) -> bool {
        let m = event.modifiers;
        self.key == event.key
            && self.ctrl == m.ctrl
            && self.alt == m.alt
            && !m.meta
            && (matches!(self.key, Key::Char(_)) || self.shift == m.shift)
    }

    /// A character typed without ctrl or alt: text, when an input has focus.
    fn is_typing(&self) -> bool {
        matches!(self.key, Key::Char(_)) && !self.ctrl && !self.alt
    }
}

/// What a remapped key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Deliver this key instead.
    Key(KeyChord),
    /// Move focus to this widget id and swallow the key.
    Focus(String),
}

/// What a remap turned a key event into.
#[derive(Debug, Clone)]
pub enum Remapped {
    Key(KeyEvent),
    Focus(String),
}

#[cfg(feature = "lua-overrides")]
mod script;
#[cfg(feature = "lua-overrides")]
pub use script::{OverrideScript, CALL_BUDGET, FRAME_BUDGET};

/// One app's overrides: the TOML file's, and the Luau file's hooks when
/// there is one (with the `lua-overrides` feature). Where both answer,
/// Luau wins: its `widget` fields go over the TOML ones, and its `key`
/// answer is taken before `[keys]`.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    widgets: HashMap<String, WidgetOverride>,
    keys: Vec<(KeyChord, KeyAction)>,
    #[cfg(feature = "lua-overrides")]
    script: Option<Rc<OverrideScript>>,
}

impl PartialEq for Overrides {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(feature = "lua-overrides")]
        let same_script = match (&self.script, &other.script) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        };
        #[cfg(not(feature = "lua-overrides"))]
        let same_script = true;
        self.widgets == other.widgets && self.keys == other.keys && same_script
    }
}

impl Overrides {
    /// Parse an override file. Any mistake (bad TOML, an unknown section or
    /// setting, a colour or key that is not one) fails the whole file.
    pub fn parse(source: &str) -> Result<Self, String> {
        let table: toml::Table = source.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut overrides = Overrides::default();
        for (section, value) in &table {
            match section.as_str() {
                "widget" => {
                    for (id, entry) in as_table(value, "widget")? {
                        let parsed = parse_widget(entry, &format!("widget.{id}"))?;
                        overrides.widgets.insert(id.clone(), parsed);
                    }
                }
                "keys" => {
                    for (spec, action) in as_table(value, "keys")? {
                        let at = format!("keys.\"{spec}\"");
                        let chord = KeyChord::parse(spec).map_err(|e| format!("{at}: {e}"))?;
                        let action = parse_key_action(action).map_err(|e| format!("{at}: {e}"))?;
                        overrides.keys.push((chord, action));
                    }
                }
                other => {
                    return Err(format!(
                        "unknown section '{other}' (expected 'widget' or 'keys')"
                    ))
                }
            }
        }
        Ok(overrides)
    }

    /// Overrides with only the hooks of the Luau override file `source`
    /// (`name` names it in errors).
    #[cfg(feature = "lua-overrides")]
    pub fn parse_luau(source: &str, name: &str) -> Result<Self, String> {
        let script = OverrideScript::load(source, name)?;
        Ok(Self::default().with_script(Some(Rc::new(script))))
    }

    /// These overrides with `script`'s hooks over them.
    #[cfg(feature = "lua-overrides")]
    pub fn with_script(mut self, script: Option<Rc<OverrideScript>>) -> Self {
        self.script = script;
        self
    }

    /// The Luau hooks, if any.
    #[cfg(feature = "lua-overrides")]
    pub fn script(&self) -> Option<&Rc<OverrideScript>> {
        self.script.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "lua-overrides")]
        if self.script.is_some() {
            return false;
        }
        self.widgets.is_empty() && self.keys.is_empty()
    }

    /// The TOML override for widget `id`, if the file has one.
    pub fn widget(&self, id: &str) -> Option<&WidgetOverride> {
        self.widgets.get(id)
    }

    /// The override for `view`: the TOML one, with the Luau `widget` hook's
    /// answer over it.
    fn resolve(&self, view: &dyn View) -> Option<WidgetOverride> {
        let id = view.widget_id()?;
        let base = self.widgets.get(id);
        #[cfg(feature = "lua-overrides")]
        if let Some(patch) = self.script.as_ref().and_then(|s| s.widget(id, view)) {
            return Some(patch.over(base));
        }
        base.cloned()
    }

    /// Start a frame (the Luau hooks' per-frame budget and results).
    fn begin_frame(&self) {
        #[cfg(feature = "lua-overrides")]
        if let Some(script) = &self.script {
            script.begin_frame();
        }
    }

    /// What the user remapped `event` to, if anything. A remapped key keeps
    /// the event's `focused_id`. When `typing` (the focused widget takes
    /// text), plain characters are left alone so they can be typed.
    pub fn remap(&self, event: &KeyEvent, typing: bool) -> Option<Remapped> {
        #[cfg(feature = "lua-overrides")]
        if let Some(action) = self.script.as_ref().and_then(|s| s.key(event, typing)) {
            return Some(Self::remapped(event, &action));
        }
        let (_, action) = self
            .keys
            .iter()
            .find(|(from, _)| from.matches(event) && !(typing && from.is_typing()))?;
        Some(Self::remapped(event, action))
    }

    fn remapped(event: &KeyEvent, action: &KeyAction) -> Remapped {
        match action {
            KeyAction::Focus(id) => Remapped::Focus(id.clone()),
            KeyAction::Key(to) => Remapped::Key(KeyEvent {
                key: to.key.clone(),
                modifiers: crate::event::Modifiers {
                    ctrl: to.ctrl,
                    alt: to.alt,
                    shift: to.shift,
                    meta: false,
                },
                focused_id: event.focused_id.clone(),
            }),
        }
    }
}

fn as_table<'a>(value: &'a toml::Value, at: &str) -> Result<&'a toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{at}: expected a table, got {}", value.type_str()))
}

fn parse_widget(value: &toml::Value, at: &str) -> Result<WidgetOverride, String> {
    let mut widget = WidgetOverride::default();
    for (key, value) in as_table(value, at)? {
        let here = format!("{at}.{key}");
        let wrong =
            |expected: &str| format!("{here}: expected {expected}, got {}", value.type_str());
        match key.as_str() {
            "fg" | "bg" => {
                let color = parse_color(value).map_err(|e| format!("{here}: {e}"))?;
                *if key == "fg" {
                    &mut widget.fg
                } else {
                    &mut widget.bg
                } = Some(color);
            }
            "bold" => widget.bold = Some(value.as_bool().ok_or_else(|| wrong("true or false"))?),
            "hidden" => widget.hidden = value.as_bool().ok_or_else(|| wrong("true or false"))?,
            "text" => widget.text = Some(value.as_str().ok_or_else(|| wrong("a string"))?.into()),
            other => {
                return Err(format!(
                    "{at}: unknown setting '{other}' (expected fg, bg, bold, hidden or text)"
                ))
            }
        }
    }
    Ok(widget)
}

fn parse_color(value: &toml::Value) -> Result<Color, String> {
    match value {
        toml::Value::Integer(i) => u8::try_from(*i)
            .map(Color::Indexed)
            .map_err(|_| format!("palette index {i} is outside 0-255")),
        toml::Value::String(s) => crate::theme::parse_color(s),
        other => Err(format!(
            "expected a colour name, \"#rrggbb\" or 0-255, got {}",
            other.type_str()
        )),
    }
}

fn parse_key_action(value: &toml::Value) -> Result<KeyAction, String> {
    match value {
        toml::Value::String(spec) => KeyChord::parse(spec).map(KeyAction::Key),
        toml::Value::Table(table) => match (table.get("focus"), table.len()) {
            (Some(toml::Value::String(id)), 1) => Ok(KeyAction::Focus(id.clone())),
            _ => Err("expected a key or { focus = \"<widget id>\" }".to_string()),
        },
        other => Err(format!(
            "expected a key or {{ focus = \"<widget id>\" }}, got {}",
            other.type_str()
        )),
    }
}

/// One file an app's overrides come from, and when it was last read.
#[derive(Debug)]
struct Source {
    path: PathBuf,
    /// Modification time and length when last read; `None` before the
    /// first read and while the file is missing.
    stamp: Option<(Option<SystemTime>, u64)>,
}

/// What [`Source::poll`] found.
enum Polled {
    Unchanged,
    /// The file is gone (or never was).
    Gone,
    /// The file changed: its text, or why it cannot be read.
    Changed(Result<String, String>),
}

impl Source {
    fn new(path: PathBuf) -> Self {
        Self { path, stamp: None }
    }

    fn poll(&mut self) -> Polled {
        let meta = std::fs::metadata(&self.path).ok();
        let stamp = meta.as_ref().map(|meta| (meta.modified().ok(), meta.len()));
        if stamp == self.stamp {
            return Polled::Unchanged;
        }
        let was_there = self.stamp.is_some();
        self.stamp = stamp;
        match meta {
            None if was_there => Polled::Gone,
            None => Polled::Unchanged,
            Some(meta) => Polled::Changed(read_source(&self.path, meta.is_file())),
        }
    }
}

/// Where an app's overrides come from (a TOML file, a Luau file, or both),
/// the last good version of each, and the override pushed through the
/// terminal over them (ADR 0023 §9).
#[derive(Debug, Default)]
pub struct OverrideFile {
    toml: Option<Source>,
    luau: Option<Source>,
    /// The last good parse of the TOML file.
    parsed: Overrides,
    /// The last good load of the Luau file.
    #[cfg(feature = "lua-overrides")]
    script: Option<Rc<OverrideScript>>,
    /// A pushed TOML override: merged over `parsed` per widget id, its
    /// keys asked first.
    pushed_toml: Option<Overrides>,
    /// A pushed Luau override: replaces `script` while it is in force.
    #[cfg(feature = "lua-overrides")]
    pushed_luau: Option<Rc<OverrideScript>>,
    /// Whether overrides are on at all. Off ([`OverrideFile::disabled`]:
    /// `ULTTUI_NO_OVERRIDES`, no usable app name, a mock backend) refuses
    /// pushes too, as ADR 0023 §2 promises.
    enabled: bool,
    current: Rc<Overrides>,
}

impl OverrideFile {
    /// No file: overrides are off.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Read overrides from `path`: a Luau file if it ends in `.luau`,
    /// TOML otherwise.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if is_luau(&path) {
            Self::from_paths(None, Some(path))
        } else {
            Self::from_paths(Some(path), None)
        }
    }

    fn from_paths(toml: Option<PathBuf>, luau: Option<PathBuf>) -> Self {
        let mut file = Self {
            toml: toml.map(Source::new),
            luau: luau.map(Source::new),
            enabled: true,
            ..Self::default()
        };
        file.refresh();
        file
    }

    /// The files for app `name`, as the environment says (see the module
    /// docs and ADR 0023): off when `ULTTUI_NO_OVERRIDES` is set, the path
    /// in `ULTTUI_OVERRIDES` when that is set, otherwise `<name>.toml` and
    /// `<name>.luau` in `$XDG_CONFIG_HOME/ulttui/overrides` (or under
    /// `~/.config`).
    pub fn for_app(name: &str) -> Self {
        match resolve_path(name, |key| std::env::var_os(key)) {
            Some(Resolved::One(path)) => Self::at(path),
            Some(Resolved::Both { toml, luau }) => Self::from_paths(Some(toml), Some(luau)),
            None => Self::disabled(),
        }
    }

    /// The files being read.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.toml
            .iter()
            .chain(self.luau.iter())
            .map(|source| source.path.as_path())
    }

    /// The overrides in force: the last good version of each file.
    pub fn overrides(&self) -> &Rc<Overrides> {
        &self.current
    }

    /// Reread each file whose modification time or length changed. A file
    /// that fails to read, parse or load is logged and its last good version
    /// stays; a file that is gone contributes nothing. Returns whether the
    /// overrides changed.
    pub fn refresh(&mut self) -> bool {
        let mut changed = false;
        if let Some(source) = &mut self.toml {
            match source.poll() {
                Polled::Unchanged => {}
                Polled::Gone => {
                    tracing::info!(path = %source.path.display(), "override file removed");
                    self.parsed = Overrides::default();
                    changed = true;
                }
                Polled::Changed(read) => match read.and_then(|text| Overrides::parse(&text)) {
                    Ok(parsed) => {
                        tracing::info!(path = %source.path.display(), "loaded overrides");
                        self.parsed = parsed;
                        changed = true;
                    }
                    Err(error) => {
                        tracing::warn!(path = %source.path.display(), %error, "ignoring override file");
                    }
                },
            }
        }
        if let Some(source) = &mut self.luau {
            match source.poll() {
                Polled::Unchanged => {}
                Polled::Gone => {
                    tracing::info!(path = %source.path.display(), "override script removed");
                    #[cfg(feature = "lua-overrides")]
                    {
                        self.script = None;
                    }
                    changed = true;
                }
                Polled::Changed(read) => {
                    #[cfg(feature = "lua-overrides")]
                    {
                        let name = source.path.display().to_string();
                        match read.and_then(|text| OverrideScript::load(&text, &name)) {
                            Ok(script) => {
                                tracing::info!(path = %name, "loaded override script");
                                self.script = Some(Rc::new(script));
                                changed = true;
                            }
                            Err(error) => {
                                tracing::warn!(path = %name, %error, "ignoring override script");
                            }
                        }
                    }
                    #[cfg(not(feature = "lua-overrides"))]
                    {
                        let _ = read;
                        tracing::warn!(
                            path = %source.path.display(),
                            "this app was built without the lua-overrides feature; ignoring the Luau override file"
                        );
                    }
                }
            }
        }
        if !changed {
            return false;
        }
        self.recombine()
    }

    /// Put `pushed` in force over the files, replacing any earlier push;
    /// `None` clears the pushed layer. The source is parsed as a file of
    /// its format would be, and a source that does not parse changes
    /// nothing and is the error. Returns whether the overrides changed.
    pub fn push(&mut self, pushed: Option<&PushedOverride>) -> Result<bool, String> {
        if !self.enabled {
            return Err(format!(
                "overrides are off in this app ({DISABLE_ENV} is set, or it has no usable app name); nothing pushed"
            ));
        }
        match pushed {
            None => {
                self.pushed_toml = None;
                #[cfg(feature = "lua-overrides")]
                {
                    self.pushed_luau = None;
                }
            }
            Some(PushedOverride {
                format: OverrideFormat::Toml,
                source,
            }) => {
                let parsed = Overrides::parse(source)?;
                self.pushed_toml = Some(parsed);
                #[cfg(feature = "lua-overrides")]
                {
                    self.pushed_luau = None;
                }
            }
            Some(PushedOverride {
                format: OverrideFormat::Luau,
                source,
            }) => {
                #[cfg(feature = "lua-overrides")]
                {
                    let script = OverrideScript::load(source, "pushed override")?;
                    self.pushed_luau = Some(Rc::new(script));
                    self.pushed_toml = None;
                }
                #[cfg(not(feature = "lua-overrides"))]
                {
                    let _ = source;
                    return Err(
                        "this app was built without the lua-overrides feature; it takes TOML overrides only"
                            .to_string(),
                    );
                }
            }
        }
        Ok(self.recombine())
    }

    /// Rebuild the overrides in force from the files and the pushed layer.
    /// Returns whether they changed.
    fn recombine(&mut self) -> bool {
        let mut combined = self.parsed.clone();
        if let Some(pushed) = &self.pushed_toml {
            combined.widgets.extend(
                pushed
                    .widgets
                    .iter()
                    .map(|(id, widget)| (id.clone(), widget.clone())),
            );
            let mut keys = pushed.keys.clone();
            keys.append(&mut combined.keys);
            combined.keys = keys;
        }
        #[cfg(feature = "lua-overrides")]
        let combined =
            combined.with_script(self.pushed_luau.clone().or_else(|| self.script.clone()));
        let differs = *self.current != combined;
        self.current = Rc::new(combined);
        differs
    }
}

fn is_luau(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "luau")
}

/// The largest override file read; a bigger one is ignored.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The text of the override file at `path`. Only a regular file is opened
/// (a FIFO would block, a device such as `/dev/zero` would never end), and
/// at most [`MAX_FILE_BYTES`] of it is read.
fn read_source(path: &Path, is_file: bool) -> Result<String, String> {
    use std::io::Read;
    if !is_file {
        return Err("not a regular file".to_string());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut source = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|e| e.to_string())?;
    if source.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    Ok(source)
}

/// Where an app's overrides are read from.
#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    /// One file named by `ULTTUI_OVERRIDES`.
    One(PathBuf),
    /// The app's TOML and Luau files, either of which may be missing.
    Both { toml: PathBuf, luau: PathBuf },
}

/// The override paths for app `name` given environment lookup `env`.
fn resolve_path(name: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<Resolved> {
    let set = |key: &str| env(key).filter(|v| !v.is_empty());
    if set(DISABLE_ENV).is_some_and(|v| v != "0") {
        return None;
    }
    if let Some(path) = set(PATH_ENV) {
        return Some(Resolved::One(PathBuf::from(path)));
    }
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        tracing::warn!(name, "not a usable app name for overrides; overrides off");
        return None;
    }
    let config = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let dir = config.join("ulttui").join("overrides");
    Some(Resolved::Both {
        toml: dir.join(format!("{name}.toml")),
        luau: dir.join(format!("{name}.luau")),
    })
}

/// The app name overrides are looked up by: the base name of `argv[0]`.
pub fn default_app_name() -> String {
    std::env::args_os()
        .next()
        .map(PathBuf::from)
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

thread_local! {
    /// The overrides the view walks apply, while a frame renders.
    static ACTIVE: RefCell<Option<Rc<Overrides>>> = const { RefCell::new(None) };
    /// The replacement text of the widget being rendered.
    static TEXT: RefCell<Option<Rc<str>>> = const { RefCell::new(None) };
}

/// Run `f` (a render) with `overrides` applied to the views it walks.
pub fn with_overrides<R>(overrides: Rc<Overrides>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<Overrides>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            ACTIVE.with(|active| *active.borrow_mut() = previous);
        }
    }
    overrides.begin_frame();
    let active = (!overrides.is_empty()).then_some(overrides);
    let _restore = Restore(ACTIVE.with(|slot| slot.replace(active)));
    f()
}

/// The override for `view` as it applies now, or `None` when it has none
/// or no overrides are active. The outer `None` means no overrides are
/// active at all.
///
/// While the widget has focus its `fg` and `bg` are left out, so the
/// widget's own focus colours stay visible.
pub(crate) fn for_view(view: &dyn View) -> Option<Option<WidgetOverride>> {
    let mut user = lookup(view)?;
    if let Some(user) = &mut user {
        if (user.fg.is_some() || user.bg.is_some())
            && crate::widgets::is_widget_focused(view.widget_id())
        {
            user.fg = None;
            user.bg = None;
        }
    }
    Some(user)
}

/// The override of an app's root view `root`: its style only. Hiding or
/// relabelling the whole app is not supported; a file that asks for it is
/// logged (once per process) and those settings are ignored, in both modes.
pub(crate) fn root_override(root: &dyn View) -> Option<WidgetOverride> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let mut user = for_view(root).flatten()?;
    if (user.hidden || user.text.is_some()) && !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            id = root.widget_id().unwrap_or_default(),
            "hidden and text do not apply to the app's root view; ignoring them"
        );
    }
    user.hidden = false;
    user.text = None;
    Some(user)
}

/// Whether `view` is hidden by an override.
pub(crate) fn is_hidden(view: &dyn View) -> bool {
    matches!(lookup(view), Some(Some(user)) if user.hidden)
}

/// `view`'s override, if overrides are active (the outer `None` when not).
fn lookup(view: &dyn View) -> Option<Option<WidgetOverride>> {
    // The active overrides are taken out of the slot's borrow before the
    // Luau hook runs: it reads the view, which may look overrides up again.
    let overrides = ACTIVE.with(|active| active.borrow().clone())?;
    Some(overrides.resolve(view))
}

/// Run `f` with `text` as the replacement text that [`text_or`] gives.
pub(crate) fn with_text<R>(text: Option<Rc<str>>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<str>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            TEXT.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    let _restore = Restore(TEXT.with(|slot| slot.replace(text)));
    f()
}

/// The replacement text of the widget being rendered, if it has one.
pub(crate) fn replacement_text() -> Option<Rc<str>> {
    TEXT.with(|slot| slot.borrow().clone())
}

/// The text a text-bearing widget shows: `own`, unless the widget being
/// rendered has replacement text.
pub(crate) fn text_or(own: &str) -> Cow<'_, str> {
    match replacement_text() {
        Some(text) => Cow::Owned(text.to_string()),
        None => Cow::Borrowed(own),
    }
}

/// `view`'s constraints as its replacement text sizes them, for the
/// layouts that size a widget from its text.
pub(crate) fn constraints_of(view: &dyn View) -> ultty_protocol::Constraints {
    match for_view(view) {
        None => view.constraints(),
        Some(user) => with_text(user.and_then(|u| u.text), || view.constraints()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;
    use ultty_protocol::NamedColor;

    fn key(key: Key, ctrl: bool, alt: bool, shift: bool) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                alt,
                shift,
                meta: false,
            },
            focused_id: Some("list".into()),
        }
    }

    #[test]
    fn parses_every_widget_setting() {
        let overrides = Overrides::parse(
            r##"
            [widget.status]
            fg = "yellow"
            bg = "#102030"
            bold = false
            [widget.sidebar]
            hidden = true
            [widget.submit]
            text = "Send"
            fg = 208
            "##,
        )
        .unwrap();
        assert_eq!(
            overrides.widget("status"),
            Some(&WidgetOverride {
                fg: Some(Color::Named(NamedColor::Yellow)),
                bg: Some(Color::Rgb(0x10, 0x20, 0x30)),
                bold: Some(false),
                ..Default::default()
            })
        );
        assert!(overrides.widget("sidebar").unwrap().hidden);
        let submit = overrides.widget("submit").unwrap();
        assert_eq!(submit.text.as_deref(), Some("Send"));
        assert_eq!(submit.fg, Some(Color::Indexed(208)));
        assert_eq!(overrides.widget("other"), None);
    }

    #[test]
    fn mistakes_fail_the_whole_file_and_say_where() {
        for (source, expected) in [
            (
                "[widget.a]\nfg = \"purple\"",
                "widget.a.fg: 'purple' is not a colour",
            ),
            (
                "[widget.a]\nfg = 300",
                "widget.a.fg: palette index 300 is outside 0-255",
            ),
            (
                "[widget.a]\ncolour = \"red\"",
                "widget.a: unknown setting 'colour'",
            ),
            (
                "[widget.a]\nhidden = \"yes\"",
                "widget.a.hidden: expected true or false",
            ),
            ("[widget.a]\ntext = 3", "widget.a.text: expected a string"),
            ("[widgets.a]\nhidden = true", "unknown section 'widgets'"),
            (
                "[keys]\n\"hyper+a\" = \"b\"",
                "keys.\"hyper+a\": 'hyper' in 'hyper+a'",
            ),
            (
                "[keys]\n\"a\" = \"nope\"",
                "keys.\"a\": 'nope' is not a key name",
            ),
            (
                "[keys]\n\"a\" = { go = \"x\" }",
                "keys.\"a\": expected a key or",
            ),
            ("[widget.a\n", "TOML parse error"),
        ] {
            let error = Overrides::parse(source).unwrap_err();
            assert!(error.contains(expected), "{source:?}: {error}");
        }
    }

    #[test]
    fn key_chords_parse_modifiers_and_plus() {
        let chord = KeyChord::parse("ctrl+alt+n").unwrap();
        assert_eq!(
            (chord.key, chord.ctrl, chord.alt),
            (Key::Char('n'), true, true)
        );
        assert_eq!(KeyChord::parse("+").unwrap().key, Key::Char('+'));
        let plus = KeyChord::parse("ctrl++").unwrap();
        assert_eq!((plus.key, plus.ctrl), (Key::Char('+'), true));
        let back_tab = KeyChord::parse("shift+tab").unwrap();
        assert_eq!((back_tab.key, back_tab.shift), (Key::Tab, true));
        assert!(!KeyChord::parse("shift+a").unwrap().shift);
    }

    #[test]
    fn remaps_keys_and_focus_jumps() {
        let overrides = Overrides::parse(
            r#"
            [keys]
            "j" = "down"
            "ctrl+n" = "ctrl+down"
            "/" = { focus = "search" }
            "shift+tab" = "up"
            "#,
        )
        .unwrap();

        let Some(Remapped::Key(down)) =
            overrides.remap(&key(Key::Char('j'), false, false, false), false)
        else {
            panic!("j should remap to a key");
        };
        assert_eq!(down.key, Key::Down);
        assert_eq!(down.modifiers, Modifiers::default());
        assert_eq!(down.focused_id.as_deref(), Some("list"));

        // Shift is not compared for characters.
        assert!(overrides
            .remap(&key(Key::Char('j'), false, false, true), false)
            .is_some());
        // Ctrl and alt are.
        assert!(overrides
            .remap(&key(Key::Char('j'), true, false, false), false)
            .is_none());
        assert!(overrides
            .remap(&key(Key::Char('j'), false, true, false), false)
            .is_none());

        let Some(Remapped::Key(ctrl_down)) =
            overrides.remap(&key(Key::Char('n'), true, false, false), false)
        else {
            panic!("ctrl+n should remap to a key");
        };
        assert_eq!((ctrl_down.key, ctrl_down.modifiers.ctrl), (Key::Down, true));

        assert!(matches!(
            overrides.remap(&key(Key::Char('/'), false, false, false), false),
            Some(Remapped::Focus(id)) if id == "search"
        ));
        // While an input has focus, plain characters are typed, not remapped;
        // chords with ctrl or alt still are.
        assert!(overrides
            .remap(&key(Key::Char('j'), false, false, false), true)
            .is_none());
        assert!(overrides
            .remap(&key(Key::Char('n'), true, false, false), true)
            .is_some());
        // A key held with meta (super) matches no chord.
        let mut super_j = key(Key::Char('j'), false, false, false);
        super_j.modifiers.meta = true;
        assert!(overrides.remap(&super_j, false).is_none());
        // Shift is compared for named keys.
        assert!(overrides
            .remap(&key(Key::Tab, false, false, false), false)
            .is_none());
        assert!(overrides
            .remap(&key(Key::Tab, false, false, true), false)
            .is_some());
    }

    #[test]
    fn style_goes_over_the_widget_and_its_spans() {
        let widget = WidgetOverride {
            fg: Some(Color::Named(NamedColor::Red)),
            bold: Some(true),
            ..Default::default()
        };
        let style = widget.apply_style(Style::new().fg(Color::Named(NamedColor::Blue)));
        assert_eq!(style.fg, Some(Color::Named(NamedColor::Red)));
        assert!(style.bold);

        let rich = ultty_protocol::RichText::new()
            .span("a", Style::new().fg(Color::Named(NamedColor::Green)))
            .span(
                "b",
                Style {
                    italic: true,
                    ..Style::new()
                },
            );
        let Content::RichText(rich) = widget.apply_content(Content::RichText(rich)) else {
            panic!("rich text stays rich text");
        };
        for span in &rich.spans {
            assert_eq!(span.style.fg, Some(Color::Named(NamedColor::Red)));
            assert!(span.style.bold);
        }
        assert!(rich.spans[1].style.italic, "fields not overridden are kept");
    }

    #[test]
    fn path_follows_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let both = |dir: &str| Resolved::Both {
            toml: PathBuf::from(format!("{dir}/todo.toml")),
            luau: PathBuf::from(format!("{dir}/todo.luau")),
        };
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/home/u")])),
            Some(both("/home/u/.config/ulttui/overrides"))
        );
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x")])),
            Some(both("/x/ulttui/overrides"))
        );
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/h"), (PATH_ENV, "/tmp/o.luau")])),
            Some(Resolved::One(PathBuf::from("/tmp/o.luau")))
        );
        assert_eq!(
            resolve_path(
                "todo",
                env(&[("HOME", "/h"), (PATH_ENV, "/o.toml"), (DISABLE_ENV, "1")])
            ),
            None,
            "disabling wins over an explicit path"
        );
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/h"), (DISABLE_ENV, "yes")])),
            None
        );
        assert!(resolve_path("todo", env(&[("HOME", "/h"), (DISABLE_ENV, "0")])).is_some());
        assert!(resolve_path("todo", env(&[("HOME", "/h"), (DISABLE_ENV, "")])).is_some());
        for bad in ["", ".hidden", "a/b"] {
            assert_eq!(resolve_path(bad, env(&[("HOME", "/h")])), None, "{bad:?}");
        }
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ulttui-overrides-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_broken_file_is_ignored_and_a_broken_reload_keeps_the_last_good() {
        let dir = scratch_dir("reload");
        let path = dir.join("app.toml");

        std::fs::write(&path, "[widget.a\nhidden = true").unwrap();
        let mut file = OverrideFile::at(&path);
        assert!(
            file.overrides().is_empty(),
            "a broken file gives no overrides"
        );

        std::fs::write(&path, "[widget.a]\nhidden = true\n").unwrap();
        assert!(file.refresh());
        assert!(file.overrides().widget("a").unwrap().hidden);

        std::fs::write(&path, "[widget.a]\nhidden = maybe\n").unwrap();
        assert!(!file.refresh());
        assert!(
            file.overrides().widget("a").unwrap().hidden,
            "a broken save keeps the last good overrides"
        );

        std::fs::remove_file(&path).unwrap();
        assert!(file.refresh());
        assert!(
            file.overrides().is_empty(),
            "a removed file means no overrides"
        );
        assert!(!file.refresh());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_regular_files_up_to_the_size_cap_are_read() {
        let dir = scratch_dir("special");
        let mut directory = OverrideFile::at(&dir);
        assert!(directory.overrides().is_empty());
        assert!(!directory.refresh());

        // A device that never ends: ignored without being opened.
        if Path::new("/dev/zero").exists() {
            assert!(OverrideFile::at("/dev/zero").overrides().is_empty());
        }

        let big = dir.join("big.toml");
        let entry = "[widget.a]\nhidden = true\n";
        let padding = "#".repeat(MAX_FILE_BYTES as usize) + "\n";
        std::fs::write(&big, format!("{entry}{padding}")).unwrap();
        assert!(
            OverrideFile::at(&big).overrides().is_empty(),
            "a file over the cap is ignored"
        );
        std::fs::write(&big, entry).unwrap();
        assert!(OverrideFile::at(&big).overrides().widget("a").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without the `lua-overrides` feature a Luau file is logged and
    /// ignored, and the TOML file beside it still applies.
    #[cfg(not(feature = "lua-overrides"))]
    #[test]
    fn without_the_feature_a_luau_file_is_ignored() {
        let dir = scratch_dir("no-luau");
        let toml = dir.join("app.toml");
        let luau = dir.join("app.luau");
        std::fs::write(&toml, "[widget.a]\nhidden = true\n").unwrap();
        std::fs::write(
            &luau,
            "return { widget = function() return { text = 'x' } end }",
        )
        .unwrap();
        let file = OverrideFile::from_paths(Some(toml), Some(luau.clone()));
        assert!(file.overrides().widget("a").unwrap().hidden);
        assert!(OverrideFile::at(&luau).overrides().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pushed(format: OverrideFormat, source: &str) -> PushedOverride {
        PushedOverride {
            format,
            source: source.to_string(),
        }
    }

    /// A pushed TOML override merges over the file per widget id and its
    /// keys are asked first; a second push replaces the first; a clear
    /// leaves the file's overrides as they were. The file keeps reloading
    /// underneath.
    #[test]
    fn a_pushed_toml_override_layers_over_the_file_and_clears() {
        let dir = scratch_dir("push");
        let path = dir.join("app.toml");
        std::fs::write(
            &path,
            "[widget.a]\nhidden = true\n[widget.b]\ntext = \"file\"\n[keys]\n\"j\" = \"down\"\n",
        )
        .unwrap();
        let mut file = OverrideFile::at(&path);
        let down = key(Key::Char('j'), false, false, false);

        let changed = file
            .push(Some(&pushed(
                OverrideFormat::Toml,
                "[widget.b]\nbold = true\n[keys]\n\"j\" = \"up\"\n",
            )))
            .unwrap();
        assert!(changed);
        let now = file.overrides();
        assert!(
            now.widget("a").unwrap().hidden,
            "untouched ids keep the file's"
        );
        let b = now.widget("b").unwrap();
        assert_eq!(
            (b.bold, b.text.clone()),
            (Some(true), None),
            "a pushed id replaces the file's entry"
        );
        assert!(
            matches!(now.remap(&down, false), Some(Remapped::Key(e)) if e.key == Key::Up),
            "pushed keys are asked first"
        );

        // The file reloads underneath the push.
        std::fs::write(&path, "[widget.c]\nhidden = true\n").unwrap();
        assert!(file.refresh());
        let now = file.overrides();
        assert!(now.widget("a").is_none() && now.widget("c").unwrap().hidden);
        assert_eq!(now.widget("b").unwrap().bold, Some(true));

        // A second push replaces the first.
        file.push(Some(&pushed(
            OverrideFormat::Toml,
            "[widget.d]\nhidden = true\n",
        )))
        .unwrap();
        let now = file.overrides();
        assert!(now.widget("b").is_none() && now.widget("d").unwrap().hidden);
        assert!(now.remap(&down, false).is_none());

        // A push that does not parse changes nothing and is the error.
        let err = file
            .push(Some(&pushed(
                OverrideFormat::Toml,
                "[widget.d]\nhidden = \"maybe\"\n",
            )))
            .unwrap_err();
        assert!(err.contains("widget.d.hidden"), "{err}");
        assert!(file.overrides().widget("d").unwrap().hidden);

        assert!(file.push(None).unwrap());
        let now = file.overrides();
        assert!(now.widget("d").is_none() && now.widget("c").unwrap().hidden);
        assert!(
            !file.push(None).unwrap(),
            "clearing nothing changes nothing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(feature = "lua-overrides"))]
    #[test]
    fn without_the_feature_a_pushed_luau_override_is_refused() {
        let dir = scratch_dir("push-no-luau");
        let mut file = OverrideFile::at(dir.join("absent.toml"));
        let err = file
            .push(Some(&pushed(OverrideFormat::Luau, "return {}")))
            .unwrap_err();
        assert!(err.contains("lua-overrides"), "{err}");
        assert!(file.overrides().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Overrides that are off (`ULTTUI_NO_OVERRIDES`, no app name) stay off
    /// for pushes too; the push is refused with a reason, not applied.
    #[test]
    fn a_push_into_disabled_overrides_is_refused() {
        let mut file = OverrideFile::disabled();
        let err = file
            .push(Some(&pushed(
                OverrideFormat::Toml,
                "[widget.a]\nhidden = true\n",
            )))
            .unwrap_err();
        assert!(err.contains("overrides are off"), "{err}");
        assert!(file.overrides().is_empty());
        let err = file.push(None).unwrap_err();
        assert!(err.contains("overrides are off"), "{err}");

        // The environment switch is what disables them.
        let resolved = resolve_path("app", |key| {
            (key == DISABLE_ENV).then(|| OsString::from("1"))
        });
        assert_eq!(resolved, None);
    }

    #[cfg(feature = "lua-overrides")]
    #[test]
    fn a_pushed_luau_override_replaces_the_scripts_hooks_and_clears() {
        let dir = scratch_dir("push-luau");
        let luau = dir.join("app.luau");
        std::fs::write(
            &luau,
            "return { key = function(chord) if chord == 'j' then return 'down' end end }",
        )
        .unwrap();
        let mut file = OverrideFile::at(&luau);
        let j = key(Key::Char('j'), false, false, false);
        assert!(
            matches!(file.overrides().remap(&j, false), Some(Remapped::Key(e)) if e.key == Key::Down)
        );

        assert!(file
            .push(Some(&pushed(
                OverrideFormat::Luau,
                "return { key = function(chord) if chord == 'j' then return 'up' end end }",
            )))
            .unwrap());
        assert!(
            matches!(file.overrides().remap(&j, false), Some(Remapped::Key(e)) if e.key == Key::Up)
        );

        let err = file
            .push(Some(&pushed(OverrideFormat::Luau, "return {")))
            .unwrap_err();
        assert!(err.contains("pushed override"), "{err}");
        assert!(
            matches!(file.overrides().remap(&j, false), Some(Remapped::Key(e)) if e.key == Key::Up)
        );

        assert!(file.push(None).unwrap());
        assert!(
            matches!(file.overrides().remap(&j, false), Some(Remapped::Key(e)) if e.key == Key::Down)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = scratch_dir("missing");
        let mut file = OverrideFile::at(dir.join("absent.toml"));
        assert!(file.overrides().is_empty());
        assert!(!file.refresh());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_scope_nests_and_restores() {
        assert_eq!(text_or("own"), "own");
        with_text(Some("outer".into()), || {
            assert_eq!(text_or("own"), "outer");
            with_text(None, || assert_eq!(text_or("own"), "own"));
            assert_eq!(text_or("own"), "outer");
        });
        assert_eq!(text_or("own"), "own");
    }
}
