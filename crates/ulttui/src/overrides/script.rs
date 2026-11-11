//! Luau override files (ADR 0023, Luau amendment): overrides computed by a
//! script instead of written out. Built with the `lua-overrides` feature.
//!
//! ```lua
//! return {
//!   -- Per widget with an id, per frame (cached): the same fields as a TOML
//!   -- [widget.<id>] table, or nil.
//!   widget = function(id, info)
//!     if id == "count" and tonumber(info.text:match("-?%d+")) < 0 then
//!       return { fg = "red", bold = true }
//!     end
//!   end,
//!   -- Per key: another key, { focus = "<id>" }, or nil.
//!   key = function(chord, focused)
//!     if chord == "j" and focused ~= "search" then return "down" end
//!   end,
//! }
//! ```
//!
//! Every call runs in the shared [`Sandbox`] under a time limit, and a
//! frame stops calling the script once it has spent its budget. A call that
//! fails or runs out of time is logged once and the widget keeps the last
//! result that worked.
//!
//! No Luau code runs outside a time limit: values a hook returns are read
//! only with raw accessors (`pairs` is a raw `next`, keys must be strings,
//! nothing is converted with `tostring`), so a metamethod cannot run after
//! the deadline is disarmed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Table, Value};
use ultty_protocol::{Color, Content, WidgetContent};

use super::{KeyAction, KeyChord, WidgetOverride};
use crate::event::{Key, KeyEvent};
use crate::lua_sandbox::Sandbox;
use crate::widgets::View;

/// Memory an override script may use. Small: it bounds how long a single
/// library call (`string.rep`, a pattern match) can run past the time limit,
/// which the interrupt cannot stop mid-call. `ulttui-lua` apps keep their
/// own, larger cap.
pub const MEMORY_LIMIT: usize = 4 * 1024 * 1024;
/// Time to run the file itself.
const LOAD_BUDGET: Duration = Duration::from_millis(200);
/// Time for one call of `widget` or `key`.
pub const CALL_BUDGET: Duration = Duration::from_millis(2);
/// Luau time one frame may spend in `widget` calls; after it, widgets keep
/// their last results until the next frame.
pub const FRAME_BUDGET: Duration = Duration::from_millis(8);
/// Cached `widget` results kept before the cache (and the last good results)
/// are emptied.
const CACHE_LIMIT: usize = 4096;

/// What a `widget` call said: only the fields it set.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WidgetPatch {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: Option<bool>,
    hidden: Option<bool>,
    text: Option<Rc<str>>,
}

impl WidgetPatch {
    /// `base` (the TOML override, if any) with this patch over it.
    pub(crate) fn over(&self, base: Option<&WidgetOverride>) -> WidgetOverride {
        let mut out = base.cloned().unwrap_or_default();
        if self.fg.is_some() {
            out.fg = self.fg;
        }
        if self.bg.is_some() {
            out.bg = self.bg;
        }
        if self.bold.is_some() {
            out.bold = self.bold;
        }
        if let Some(hidden) = self.hidden {
            out.hidden = hidden;
        }
        if self.text.is_some() {
            out.text = self.text.clone();
        }
        out
    }
}

/// What `widget` is called with; results are cached by it. The text is
/// kept as a 64-bit hash and its length, so an entry's size does not grow
/// with the widget's text (two texts of the same length with the same hash
/// would share a cached result).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WidgetKey {
    id: String,
    role: &'static str,
    text: Option<(u64, usize)>,
    focused: bool,
}

/// A cached `widget` call: what it returned, or that it failed (the widget
/// then keeps its last good result; the hook is not asked again for these
/// inputs until the file is reloaded).
#[derive(Debug, Clone)]
enum Outcome {
    Returned(Option<WidgetPatch>),
    Failed,
}

/// A loaded Luau override file.
pub struct OverrideScript {
    sandbox: Sandbox,
    widget: Option<Function>,
    key: Option<Function>,
    /// Results by inputs, across frames.
    cache: RefCell<HashMap<WidgetKey, Outcome>>,
    /// Results by id for the frame being drawn, so a widget's inputs are
    /// read once per frame however often the walks ask.
    frame: RefCell<HashMap<String, Option<WidgetPatch>>>,
    /// The last result that worked, by id: what a failed call falls back to.
    /// Emptied with the cache, so ids that come and go do not pile up.
    last_good: RefCell<HashMap<String, Option<WidgetPatch>>>,
    /// Luau time spent in this frame's `widget` calls.
    spent: Cell<Duration>,
    /// Number of `widget` calls made (for tests and measurements).
    calls: Cell<u64>,
    /// Whether a failing call has been logged for this version.
    warned: Cell<bool>,
}

impl std::fmt::Debug for OverrideScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverrideScript")
            .field("widget", &self.widget.is_some())
            .field("key", &self.key.is_some())
            .finish()
    }
}

impl OverrideScript {
    /// Run the file `source` (`name` is for error messages) and keep the
    /// hooks it returns. Fails on a Luau error, a time-out, or a returned
    /// value that is not a table of `widget` and `key` functions.
    pub fn load(source: &str, name: &str) -> Result<Self, String> {
        let sandbox = Sandbox::new(MEMORY_LIMIT, |lua, globals| {
            // `print` goes to the log; the terminal is the app's.
            let print = lua.create_function(|_, args: mlua::MultiValue| {
                let parts: Vec<String> = args
                    .iter()
                    .map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_string()))
                    .collect();
                tracing::info!(target: "ulttui::overrides", "{}", parts.join("\t"));
                Ok(())
            })?;
            globals.raw_set("print", print)
        })
        .map_err(|e| e.to_string())?;
        let returned: Value = sandbox
            .guarded(LOAD_BUDGET, || {
                sandbox
                    .lua()
                    .load(source)
                    .set_name(format!("={name}"))
                    .eval()
            })
            .map_err(|e| e.to_string())?;
        let Value::Table(hooks) = returned else {
            return Err(format!(
                "the file must return a table of hooks ({{ widget = function(id, info) ... end, key = ... }}), got {}",
                returned.type_name()
            ));
        };
        let mut widget = None;
        let mut key = None;
        for pair in hooks.pairs::<Value, Value>() {
            let (field, value) = pair.map_err(|e| e.to_string())?;
            let field = field_name(&field)?;
            let slot = match field.as_str() {
                "widget" => &mut widget,
                "key" => &mut key,
                other => {
                    return Err(format!(
                        "unknown hook '{other}' (expected 'widget' or 'key')"
                    ))
                }
            };
            match value {
                Value::Function(f) => *slot = Some(f),
                other => {
                    return Err(format!(
                        "'{field}' must be a function, got {}",
                        other.type_name()
                    ))
                }
            }
        }
        Ok(Self {
            sandbox,
            widget,
            key,
            cache: RefCell::default(),
            frame: RefCell::default(),
            last_good: RefCell::default(),
            spent: Cell::default(),
            calls: Cell::default(),
            warned: Cell::default(),
        })
    }

    /// Start a frame: per-frame results and the frame's budget reset.
    pub(crate) fn begin_frame(&self) {
        self.frame.borrow_mut().clear();
        self.spent.set(Duration::ZERO);
    }

    /// Number of `widget` calls made so far.
    pub fn widget_calls(&self) -> u64 {
        self.calls.get()
    }

    /// The script's patch for `view` (whose id is `id`), if it has a
    /// `widget` hook and the hook returned one.
    pub(crate) fn widget(&self, id: &str, view: &dyn View) -> Option<WidgetPatch> {
        let hook = self.widget.as_ref()?;
        if let Some(done) = self.frame.borrow().get(id) {
            return done.clone();
        }
        let patch = self.widget_uncached(hook, id, view);
        self.frame
            .borrow_mut()
            .insert(id.to_string(), patch.clone());
        patch
    }

    fn widget_uncached(&self, hook: &Function, id: &str, view: &dyn View) -> Option<WidgetPatch> {
        let inputs = widget_inputs(id, view);
        let key = inputs.key();
        let last_good = || self.last_good.borrow().get(id).cloned().flatten();
        match self.cache.borrow().get(&key) {
            Some(Outcome::Returned(patch)) => return patch.clone(),
            Some(Outcome::Failed) => return last_good(),
            None => {}
        }
        if self.spent.get() >= FRAME_BUDGET {
            return last_good();
        }
        let started = Instant::now();
        self.calls.set(self.calls.get() + 1);
        let result = self
            .sandbox
            .guarded(CALL_BUDGET, || {
                let info = self.sandbox.lua().create_table()?;
                info.raw_set("role", inputs.role)?;
                info.raw_set("text", inputs.text.as_deref())?;
                info.raw_set("focused", inputs.focused)?;
                hook.call::<Value>((id, info))
            })
            .map_err(|e| e.to_string())
            .and_then(|value| parse_patch(&value));
        self.spent.set(self.spent.get() + started.elapsed());
        {
            let mut cache = self.cache.borrow_mut();
            if cache.len() >= CACHE_LIMIT {
                cache.clear();
                self.last_good.borrow_mut().clear();
            }
        }
        match result {
            Ok(patch) => {
                self.cache
                    .borrow_mut()
                    .insert(key, Outcome::Returned(patch.clone()));
                self.last_good
                    .borrow_mut()
                    .insert(id.to_string(), patch.clone());
                patch
            }
            Err(error) => {
                self.cache.borrow_mut().insert(key, Outcome::Failed);
                self.warn_once(&format!("widget(\"{id}\"): {error}"));
                last_good()
            }
        }
    }

    /// What the `key` hook makes of `event`: `None` when there is no hook,
    /// it returned nil, or it failed. Plain characters typed into an input
    /// (`typing`) and keys held with meta are not passed to it.
    pub(crate) fn key(&self, event: &KeyEvent, typing: bool) -> Option<KeyAction> {
        let hook = self.key.as_ref()?;
        let m = event.modifiers;
        let is_char = matches!(event.key, Key::Char(_));
        if m.meta || (typing && is_char && !m.ctrl && !m.alt) {
            return None;
        }
        let mut chord = String::new();
        for (held, name) in [
            (m.ctrl, "ctrl+"),
            (m.alt, "alt+"),
            (m.shift && !is_char, "shift+"),
        ] {
            if held {
                chord.push_str(name);
            }
        }
        chord.push_str(&event.key.name());
        let result = self
            .sandbox
            .guarded(CALL_BUDGET, || {
                hook.call::<Value>((chord.as_str(), event.focused_id.as_deref()))
            })
            .map_err(|e| e.to_string())
            .and_then(|value| parse_key_action(&value));
        match result {
            Ok(action) => action,
            Err(error) => {
                self.warn_once(&format!("key(\"{chord}\"): {error}"));
                None
            }
        }
    }

    fn warn_once(&self, error: &str) {
        if !self.warned.replace(true) {
            tracing::warn!(
                target: "ulttui::overrides",
                %error,
                "override script failed; using the last results that worked (further failures not logged)"
            );
        }
    }
}

/// What `widget` is told about a widget.
struct WidgetInputs {
    id: String,
    role: &'static str,
    text: Option<String>,
    focused: bool,
}

impl WidgetInputs {
    fn key(&self) -> WidgetKey {
        WidgetKey {
            id: self.id.clone(),
            role: self.role,
            text: self.text.as_ref().map(|text| {
                // Fixed keys: the same text hashes the same in every frame.
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                (hasher.finish(), text.len())
            }),
            focused: self.focused,
        }
    }
}

/// What `widget` is told about `view`: its role, its text, and whether it
/// has focus. Read without any replacement text in scope, so it is the
/// app's own text.
fn widget_inputs(id: &str, view: &dyn View) -> WidgetInputs {
    let (content, role) = super::with_text(None, || (view.content(), view.semantics().role));
    let text = match content {
        Content::Text(text) => Some(text.text),
        Content::RichText(rich) => Some(rich.spans.into_iter().map(|s| s.text).collect()),
        Content::Widget(WidgetContent::Button(button)) => Some(button.label),
        _ => None,
    };
    WidgetInputs {
        id: id.to_string(),
        role: role.as_str(),
        text,
        focused: crate::widgets::is_widget_focused(Some(id)),
    }
}

fn parse_patch(value: &Value) -> Result<Option<WidgetPatch>, String> {
    let table = match value {
        Value::Nil => return Ok(None),
        Value::Table(table) => table,
        other => {
            return Err(format!(
                "must return a table or nil, got {}",
                other.type_name()
            ))
        }
    };
    let mut patch = WidgetPatch::default();
    for pair in table.pairs::<Value, Value>() {
        let (field, value) = pair.map_err(|e| e.to_string())?;
        let field = field_name(&field)?;
        let wrong =
            |expected: &str| format!("{field}: expected {expected}, got {}", value.type_name());
        match field.as_str() {
            "fg" | "bg" => {
                let color = parse_color(&value).map_err(|e| format!("{field}: {e}"))?;
                *if field == "fg" {
                    &mut patch.fg
                } else {
                    &mut patch.bg
                } = Some(color);
            }
            "bold" => patch.bold = Some(value.as_boolean().ok_or_else(|| wrong("a boolean"))?),
            "hidden" => patch.hidden = Some(value.as_boolean().ok_or_else(|| wrong("a boolean"))?),
            "text" => {
                let text = value.as_string().ok_or_else(|| wrong("a string"))?;
                patch.text = Some(text.to_string_lossy().into());
            }
            other => {
                return Err(format!(
                    "unknown field '{other}' (expected fg, bg, bold, hidden or text)"
                ))
            }
        }
    }
    Ok(Some(patch))
}

/// A table key as a field name. Only strings: converting anything else
/// (`tostring`) could run a `__tostring` metamethod outside the time limit.
fn field_name(key: &Value) -> Result<String, String> {
    match key {
        Value::String(name) => Ok(name.to_string_lossy()),
        other => Err(format!(
            "field names must be strings, got {}",
            other.type_name()
        )),
    }
}

fn parse_color(value: &Value) -> Result<Color, String> {
    match value {
        Value::Integer(i) => u8::try_from(*i)
            .map(Color::Indexed)
            .map_err(|_| format!("palette index {i} is outside 0-255")),
        Value::String(s) => crate::theme::parse_color(&s.to_string_lossy()),
        other => Err(format!(
            "expected a colour name, \"#rrggbb\" or 0-255, got {}",
            other.type_name()
        )),
    }
}

fn parse_key_action(value: &Value) -> Result<Option<KeyAction>, String> {
    match value {
        Value::Nil => Ok(None),
        Value::String(spec) => {
            KeyChord::parse(&spec.to_string_lossy()).map(|c| Some(KeyAction::Key(c)))
        }
        Value::Table(table) => focus_target(table).map(|id| Some(KeyAction::Focus(id))),
        other => Err(format!(
            "must return a key, {{ focus = \"<widget id>\" }} or nil, got {}",
            other.type_name()
        )),
    }
}

fn focus_target(table: &Table) -> Result<String, String> {
    let focus: Value = table.raw_get("focus").map_err(|e| e.to_string())?;
    match (focus, table.pairs::<Value, Value>().count()) {
        (Value::String(id), 1) => Ok(id.to_string_lossy()),
        _ => Err("expected { focus = \"<widget id>\" }".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::super::{for_view, is_hidden, with_overrides, OverrideFile, Overrides, Remapped};
    use crate::event::{Key, KeyEvent, Modifiers};
    use crate::widgets::{render_view_messages, Button, Column, View};
    use ultty_protocol::{AppMessage, Color, Content, NamedColor, WidgetContent};

    fn luau(source: &str) -> Rc<Overrides> {
        Rc::new(Overrides::parse_luau(source, "test.luau").expect("script loads"))
    }

    fn key_event(key: Key, ctrl: bool, focused: Option<&str>) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                ..Default::default()
            },
            focused_id: focused.map(str::to_string),
        }
    }

    /// The (label, fg) of each button the native walk sends.
    fn buttons(view: &dyn View) -> Vec<(String, Option<Color>)> {
        let mut out = Vec::new();
        let mut fg = None;
        for message in render_view_messages(view).unwrap() {
            match message {
                AppMessage::CreateRegion { style, .. } => fg = style.fg,
                AppMessage::SetContent {
                    content: Content::Widget(WidgetContent::Button(b)),
                    ..
                } => out.push((b.label, fg)),
                _ => {}
            }
        }
        out
    }

    const RED: Option<Color> = Some(Color::Named(NamedColor::Red));

    #[test]
    fn widget_hook_computes_from_what_the_widget_shows() {
        let overrides = luau(
            r#"
            return {
              widget = function(id, info)
                if info.role == "button" and info.text:find("^Delete") then
                  return { fg = "red", text = info.text:upper() }
                end
              end,
            }
            "#,
        );
        let view = Column::new()
            .child(Button::new("Delete all").id("del"))
            .child(Button::new("Keep").id("keep"));
        let shown = with_overrides(overrides, || buttons(&view));
        assert_eq!(shown[0], ("DELETE ALL".to_string(), RED));
        assert_eq!(shown[1].0, "Keep");
        assert_ne!(shown[1].1, RED);
    }

    #[test]
    fn luau_fields_go_over_the_toml_ones() {
        let toml = Overrides::parse("[widget.a]\nfg = \"blue\"\nbold = true\n").unwrap();
        let script = luau(
            "return { widget = function(id) if id == 'a' then return { fg = 'red' } end end }",
        );
        let combined = Rc::new(toml.with_script(script.script().cloned()));
        let user = with_overrides(combined, || for_view(&Button::new("x").id("a")))
            .flatten()
            .unwrap();
        assert_eq!(user.fg, RED);
        assert_eq!(user.bold, Some(true), "TOML fields Luau leaves alone stay");
    }

    #[test]
    fn key_hook_answers_before_the_toml_keys() {
        let toml = Overrides::parse("[keys]\n\"j\" = \"up\"\n\"k\" = \"up\"\n").unwrap();
        let script = luau(
            r#"
            return {
              key = function(chord, focused)
                if chord == "j" then return "down" end
                if chord == "ctrl+f" then return { focus = "search" } end
                if chord == "x" then return 42 end
              end,
            }
            "#,
        );
        let overrides = toml.with_script(script.script().cloned());

        let Some(Remapped::Key(down)) =
            overrides.remap(&key_event(Key::Char('j'), false, Some("list")), false)
        else {
            panic!("j is remapped");
        };
        assert_eq!(
            (down.key, down.focused_id.as_deref()),
            (Key::Down, Some("list"))
        );
        assert!(matches!(
            overrides.remap(&key_event(Key::Char('f'), true, None), false),
            Some(Remapped::Focus(id)) if id == "search"
        ));
        // nil falls through to [keys].
        let Some(Remapped::Key(up)) =
            overrides.remap(&key_event(Key::Char('k'), false, None), false)
        else {
            panic!("k falls through to TOML");
        };
        assert_eq!(up.key, Key::Up);
        // A bad answer is logged and falls through too.
        assert!(overrides
            .remap(&key_event(Key::Char('x'), false, None), false)
            .is_none());
        // Characters typed into an input reach neither.
        assert!(overrides
            .remap(&key_event(Key::Char('j'), false, Some("search")), true)
            .is_none());
    }

    #[test]
    fn an_endless_hook_does_not_stall_the_frame() {
        let overrides = luau(
            "return { widget = function() while true do end end, key = function() while true do end end }",
        );
        let view = (0..50).fold(Column::new(), |column, i| {
            column.child(Button::new("A").id(format!("w{i}")))
        });
        let started = Instant::now();
        let shown = with_overrides(overrides.clone(), || buttons(&view));
        let took = started.elapsed();
        assert_eq!(shown.len(), 50, "the frame is drawn, without overrides");
        assert!(shown.iter().all(|(label, _)| label == "A"));
        // Each call stops at its budget, and the frame stops calling once it
        // has spent its own: about FRAME_BUDGET / CALL_BUDGET calls.
        let calls = overrides.script().unwrap().widget_calls();
        assert!((1..=10).contains(&calls), "{calls} calls");
        assert!(took < Duration::from_millis(500), "frame took {took:?}");
        assert!(overrides
            .remap(&key_event(Key::Char('q'), false, None), false)
            .is_none());
    }

    #[test]
    fn a_failing_call_keeps_the_last_result_that_worked() {
        let overrides = luau(
            r#"
            return {
              widget = function(id, info)
                if info.text == "boom" then error("no") end
                return { text = "seen " .. info.text }
              end,
            }
            "#,
        );
        let first = with_overrides(overrides.clone(), || {
            buttons(&Column::new().child(Button::new("ok").id("a")))
        });
        assert_eq!(first[0].0, "seen ok");
        let second = with_overrides(overrides, || {
            buttons(&Column::new().child(Button::new("boom").id("a")))
        });
        assert_eq!(second[0].0, "seen ok", "the last good result is used");
    }

    #[test]
    fn results_are_cached_by_what_the_hook_is_given() {
        let overrides = luau(
            r#"
            local n = 0
            return { widget = function(id, info) n += 1; return { text = info.text .. " " .. n } end }
            "#,
        );
        let frame = |label: &str| {
            with_overrides(overrides.clone(), || {
                buttons(&Column::new().child(Button::new(label).id("a")))
            })
        };
        assert_eq!(frame("x")[0].0, "x 1");
        assert_eq!(frame("x")[0].0, "x 1", "same inputs: cached");
        assert_eq!(overrides.script().unwrap().widget_calls(), 1);
        assert_eq!(frame("y")[0].0, "y 2", "new inputs: called again");
    }

    /// Run with `--nocapture` (and `--release`) for the numbers.
    #[test]
    fn two_hundred_widgets_call_luau_once_each_then_not_at_all() {
        let overrides = luau(
            r#"
            return {
              widget = function(id, info)
                if info.text:sub(-1) == "7" then return { fg = "yellow", bold = true } end
              end,
            }
            "#,
        );
        let view = (0..200).fold(Column::new(), |column, i| {
            column.child(Button::new(format!("item {i}")).id(format!("w{i}")))
        });
        let frame = || {
            let started = Instant::now();
            let shown = with_overrides(overrides.clone(), || buttons(&view));
            (started.elapsed(), shown)
        };
        let (cold, shown) = frame();
        let yellow = shown
            .iter()
            .filter(|(_, fg)| *fg == Some(Color::Named(NamedColor::Yellow)))
            .count();
        assert_eq!(yellow, 20);
        let _ = frame();
        assert_eq!(
            overrides.script().unwrap().widget_calls(),
            200,
            "the warm frame calls Luau for nothing"
        );

        // Median frame time over 101 frames each: no overrides, TOML only
        // (one entry), a Luau file with only a `key` hook (widgets are never
        // asked), and the warm `widget` hook (its inputs are read per id).
        let median = |overrides: Option<Rc<Overrides>>| {
            let mut times: Vec<Duration> = (0..101)
                .map(|_| {
                    let started = Instant::now();
                    match &overrides {
                        Some(o) => drop(with_overrides(o.clone(), || buttons(&view))),
                        None => drop(buttons(&view)),
                    }
                    started.elapsed()
                })
                .collect();
            times.sort();
            times[50]
        };
        let none = median(None);
        let toml = median(Some(Rc::new(
            Overrides::parse("[widget.w7]\nfg = \"yellow\"\n").unwrap(),
        )));
        let key_only = luau("return { key = function() end }");
        let key_only_time = median(Some(key_only.clone()));
        let warm = median(Some(overrides.clone()));
        assert_eq!(key_only.script().unwrap().widget_calls(), 0);
        eprintln!(
            "200 widgets, native walk, median of 101: no overrides {none:?}, TOML {toml:?}, \
             Luau key hook only {key_only_time:?}, Luau widget hook warm {warm:?} (cold frame {cold:?})"
        );
    }

    #[test]
    fn the_documented_example_loads_and_works() {
        let docs = include_str!("../../../../docs/overrides.md");
        let start = docs.find("```lua\n").expect("docs have a Luau example") + "```lua\n".len();
        let source = &docs[start..start + docs[start..].find("```").unwrap()];
        let overrides = luau(source);
        let view = Column::new()
            .child(Button::new("Count: -3").id("count"))
            .child(Button::new("Reset").id("reset"));
        let shown = with_overrides(overrides.clone(), || buttons(&view));
        assert_eq!(shown, [("Count: -3".to_string(), RED)]);
        assert!(matches!(
            overrides.remap(&key_event(Key::Char('/'), false, None), false),
            Some(Remapped::Focus(id)) if id == "search"
        ));
    }

    /// A table whose `__tostring` never returns, as Luau source.
    const LOOPING_TOSTRING: &str =
        "setmetatable({}, { __tostring = function() while true do end end })";

    #[test]
    fn metamethods_in_returned_values_cannot_run_without_a_time_limit() {
        // A key with a looping __tostring, in a widget hook's answer...
        let overrides = luau(&format!(
            "return {{ widget = function() return {{ [{LOOPING_TOSTRING}] = 'red' }} end }}"
        ));
        let started = Instant::now();
        let shown = with_overrides(overrides, || {
            buttons(&Column::new().child(Button::new("A").id("a")))
        });
        assert_eq!(shown[0].0, "A", "the bad answer is ignored");
        assert!(started.elapsed() < Duration::from_millis(500));

        // ...in the hooks table itself...
        let started = Instant::now();
        let error = Overrides::parse_luau(
            &format!("return {{ [{LOOPING_TOSTRING}] = function() end }}"),
            "test.luau",
        )
        .unwrap_err();
        assert!(error.contains("field names must be strings"), "{error}");
        assert!(started.elapsed() < Duration::from_millis(500));

        // ...and as the value a hook raises as its error.
        let overrides = luau(&format!(
            "return {{ widget = function() error({LOOPING_TOSTRING}) end, key = function() error({LOOPING_TOSTRING}) end }}"
        ));
        let started = Instant::now();
        let _ = with_overrides(overrides.clone(), || {
            buttons(&Column::new().child(Button::new("A").id("a")))
        });
        assert!(overrides
            .remap(&key_event(Key::Char('q'), false, None), false)
            .is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_huge_allocation_fails_at_the_memory_cap() {
        let overrides =
            luau("return { widget = function() return { text = string.rep('x', 15e6) } end }");
        let started = Instant::now();
        let shown = with_overrides(overrides, || {
            buttons(&Column::new().child(Button::new("A").id("a")))
        });
        assert_eq!(shown[0].0, "A");
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_failure_is_remembered_and_not_retried_for_the_same_inputs() {
        let overrides = luau("return { widget = function() error('always') end }");
        let view = Column::new().child(Button::new("A").id("a"));
        for _ in 0..3 {
            let _ = with_overrides(overrides.clone(), || buttons(&view));
        }
        assert_eq!(overrides.script().unwrap().widget_calls(), 1);
    }

    #[test]
    fn bad_scripts_fail_to_load_and_say_why() {
        for (source, expected) in [
            ("return {", "test.luau"),
            ("return 3", "must return a table of hooks"),
            ("return { view = function() end }", "unknown hook 'view'"),
            ("return { widget = 3 }", "'widget' must be a function"),
            ("while true do end", "took too long"),
            ("os.execute('true')", "attempt to call a nil value"),
            ("return require('x')", "attempt to call a nil value"),
        ] {
            let error = Overrides::parse_luau(source, "test.luau").unwrap_err();
            assert!(error.contains(expected), "{source:?}: {error}");
        }
    }

    #[test]
    fn a_broken_save_keeps_the_last_good_script() {
        let dir = std::env::temp_dir().join(format!("ulttui-luau-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.luau");
        std::fs::write(
            &path,
            "return { widget = function() return { hidden = true } end }",
        )
        .unwrap();
        let mut file = OverrideFile::at(&path);
        let hidden = |file: &OverrideFile| {
            with_overrides(file.overrides().clone(), || {
                is_hidden(&Button::new("x").id("a"))
            })
        };
        assert!(hidden(&file));

        // A different length, so the change is seen within the same second.
        std::fs::write(&path, "return { widget = function( }").unwrap();
        assert!(!file.refresh());
        assert!(hidden(&file), "the last good script stays");

        std::fs::remove_file(&path).unwrap();
        assert!(file.refresh());
        assert!(!hidden(&file));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
