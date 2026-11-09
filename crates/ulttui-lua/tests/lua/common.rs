//! Shared helpers: scripts are written under Cargo's per-target temp dir
//! (inside `target/`), one file per test so parallel tests never collide.

use std::path::PathBuf;
use std::rc::Rc;

use ulttui::widgets::{render_view_messages, View};
use ulttui_lua::node::{build, Node};
use ulttui_lua::{Reload, Script, ScriptOptions};
use ultty_protocol::{AppMessage, Content};

pub fn script_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ulttui-lua-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}.luau"))
}

pub fn write(name: &str, source: &str) -> PathBuf {
    let path = script_path(name);
    std::fs::write(&path, source).unwrap();
    path
}

/// Write and load a script; panics unless it loads.
pub fn load(name: &str, source: &str, options: ScriptOptions) -> Script {
    let mut script = Script::new(write(name, source), options).unwrap();
    let outcome = script.reload();
    assert!(
        matches!(outcome, Reload::Loaded(_)),
        "{name} did not load: {:?}",
        script.error()
    );
    script
}

/// The node `expr` builds, or the load error.
pub fn view_of(name: &str, expr: &str) -> Result<Rc<Node>, String> {
    let source = format!("return app {{ view = function(state) return {expr} end }}");
    let mut script = Script::new(write(name, &source), ScriptOptions::default()).unwrap();
    match script.reload() {
        Reload::Loaded(_) => Ok(script.view().expect("a loaded script has a view")),
        _ => Err(script.error().unwrap_or_default()),
    }
}

pub fn messages(node: &Node) -> Vec<AppMessage> {
    render_view_messages(&build(node)).unwrap()
}

/// Every piece of text in the native message tree, in order.
pub fn texts(messages: &[AppMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            AppMessage::SetContent {
                content: Content::Text(t),
                ..
            } => Some(t.text.clone()),
            AppMessage::SetContent {
                content: Content::RichText(r),
                ..
            } => Some(r.spans.iter().map(|s| s.text.as_str()).collect()),
            _ => None,
        })
        .collect()
}

/// Render `view` through the ANSI backend into a `width`x`height` buffer.
pub fn ansi(view: &dyn View, width: u16, height: u16) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| view.render_ansi(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
