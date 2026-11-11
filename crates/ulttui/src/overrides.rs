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

use ultty_protocol::{Color, Content, Style};

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

/// One app's overrides, parsed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    widgets: HashMap<String, WidgetOverride>,
    keys: Vec<(KeyChord, KeyAction)>,
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

    pub fn is_empty(&self) -> bool {
        self.widgets.is_empty() && self.keys.is_empty()
    }

    /// The override for widget `id`, if the file has one.
    pub fn widget(&self, id: &str) -> Option<&WidgetOverride> {
        self.widgets.get(id)
    }

    /// What the user remapped `event` to, if anything. A remapped key keeps
    /// the event's `focused_id`. When `typing` (the focused widget takes
    /// text), plain characters are left alone so they can be typed.
    pub fn remap(&self, event: &KeyEvent, typing: bool) -> Option<Remapped> {
        let (_, action) = self
            .keys
            .iter()
            .find(|(from, _)| from.matches(event) && !(typing && from.is_typing()))?;
        Some(match action {
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
        })
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

/// Where an app's overrides come from, and the last good parse of it.
#[derive(Debug, Default)]
pub struct OverrideFile {
    path: Option<PathBuf>,
    /// Modification time and length when last read; `None` before the
    /// first read and while the file is missing.
    stamp: Option<(Option<SystemTime>, u64)>,
    current: Rc<Overrides>,
}

impl OverrideFile {
    /// No file: overrides are off.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Read overrides from `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let mut file = Self {
            path: Some(path.into()),
            ..Self::default()
        };
        file.refresh();
        file
    }

    /// The file for app `name`, as the environment says (see the module
    /// docs and ADR 0023): off when `ULTTUI_NO_OVERRIDES` is set, the path
    /// in `ULTTUI_OVERRIDES` when that is set, otherwise
    /// `$XDG_CONFIG_HOME/ulttui/overrides/<name>.toml` (or under
    /// `~/.config`).
    pub fn for_app(name: &str) -> Self {
        match resolve_path(name, |key| std::env::var_os(key)) {
            Some(path) => Self::at(path),
            None => Self::disabled(),
        }
    }

    /// The file being read, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The overrides in force: the last good parse of the file.
    pub fn overrides(&self) -> &Rc<Overrides> {
        &self.current
    }

    /// Reread the file if its modification time or length changed. A file
    /// that fails to read or parse is logged and the last good overrides
    /// stay; a file that is gone means no overrides. Returns whether the
    /// overrides changed.
    pub fn refresh(&mut self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let stamp = std::fs::metadata(path)
            .ok()
            .map(|meta| (meta.modified().ok(), meta.len()));
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        if stamp.is_none() {
            if self.current.is_empty() {
                return false;
            }
            tracing::info!(path = %path.display(), "override file removed; overrides off");
            self.current = Rc::default();
            return true;
        }
        let parsed = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|source| Overrides::parse(&source));
        match parsed {
            Ok(overrides) => {
                tracing::info!(path = %path.display(), "loaded overrides");
                let changed = *self.current != overrides;
                self.current = Rc::new(overrides);
                changed
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "ignoring override file");
                false
            }
        }
    }
}

/// The override path for app `name` given environment lookup `env`.
fn resolve_path(name: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |key: &str| env(key).filter(|v| !v.is_empty());
    if set(DISABLE_ENV).is_some_and(|v| v != "0") {
        return None;
    }
    if let Some(path) = set(PATH_ENV) {
        return Some(PathBuf::from(path));
    }
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        tracing::warn!(name, "not a usable app name for overrides; overrides off");
        return None;
    }
    let config = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(
        config
            .join("ulttui")
            .join("overrides")
            .join(format!("{name}.toml")),
    )
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
    let active = (!overrides.is_empty()).then_some(overrides);
    let _restore = Restore(ACTIVE.with(|slot| slot.replace(active)));
    f()
}

/// The override for `view`, or `None` when it has none or no overrides
/// are active. The outer `None` means no overrides are active at all.
pub(crate) fn for_view(view: &dyn View) -> Option<Option<WidgetOverride>> {
    lookup(view, |user| user.cloned())
}

/// Whether `view` is hidden by an override.
pub(crate) fn is_hidden(view: &dyn View) -> bool {
    lookup(view, |user| user.is_some_and(|u| u.hidden)) == Some(true)
}

/// `f` of `view`'s override, if overrides are active.
fn lookup<R>(view: &dyn View, f: impl FnOnce(Option<&WidgetOverride>) -> R) -> Option<R> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let overrides = active.as_ref()?;
        Some(f(view.widget_id().and_then(|id| overrides.widget(id))))
    })
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
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config/ulttui/overrides/todo.toml"))
        );
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x")])),
            Some(PathBuf::from("/x/ulttui/overrides/todo.toml"))
        );
        assert_eq!(
            resolve_path("todo", env(&[("HOME", "/h"), (PATH_ENV, "/tmp/o.toml")])),
            Some(PathBuf::from("/tmp/o.toml"))
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
