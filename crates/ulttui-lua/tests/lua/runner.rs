//! The ulttui host: events, process commands, the file watch, the frame.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ulttui::app::Application;
use ulttui::cmd::Cmd;
use ulttui::event::{Event, Key, KeyEvent, Modifiers};
use ulttui::widgets::{render_view_messages, View};
use ulttui_lua::runner::{watch_file, Msg, SETTLE};
use ulttui_lua::{ExecGrant, LuaApp, ScriptMsg, ScriptOptions};
use ultty_protocol::AppMessage;

use crate::common::{ansi, texts, write};

fn flatten(cmd: Cmd<Msg>) -> Vec<Cmd<Msg>> {
    match cmd {
        Cmd::None => Vec::new(),
        Cmd::Batch(cmds) => cmds.into_iter().flat_map(flatten).collect(),
        other => vec![other],
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Run every spawned task in `cmd` to completion and return their messages.
fn run_spawns(rt: &tokio::runtime::Runtime, cmd: Cmd<Msg>) -> Vec<Msg> {
    flatten(cmd)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Cmd::SpawnWithId { future, .. } | Cmd::Spawn(future) => Some(rt.block_on(future)),
            _ => None,
        })
        .collect()
}

fn frame_texts(app: &LuaApp) -> Vec<String> {
    texts(&render_view_messages(&app.frame()).unwrap())
}

fn key_event(key: Key) -> Event {
    Event::Key(KeyEvent {
        key,
        modifiers: Modifiers::default(),
        focused_id: Some("inc".into()),
    })
}

fn exec() -> ScriptOptions {
    ScriptOptions {
        exec: ExecGrant::Any,
        ..ScriptOptions::default()
    }
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/lua")
        .join(name)
}

#[test]
fn keys_and_clicks_become_script_messages_but_tab_stays_with_focus() {
    let path = write(
        "events",
        "return app { view = function() return text('x') end }",
    );
    let mut app = LuaApp::new(path, ScriptOptions::default()).unwrap();
    assert!(app.on_event(&key_event(Key::Tab)).is_none());
    assert!(app
        .on_event(&Event::Resize {
            width: 1,
            height: 1
        })
        .is_none());
    match app.on_event(&key_event(Key::PageDown)) {
        Some(Msg::Script(ScriptMsg::Key { key, focused, .. })) => {
            assert_eq!(key, "pagedown");
            assert_eq!(focused.as_deref(), Some("inc"));
        }
        other => panic!("{other:?}"),
    }
    match app.on_event(&key_event(Key::Char(' '))) {
        Some(Msg::Script(ScriptMsg::Key { key, .. })) => assert_eq!(key, "space"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        app.on_event(&Event::Click { id: "go".into() }),
        Some(Msg::Script(ScriptMsg::Click { id })) if id == "go"
    ));
}

#[test]
fn processes_run_on_boot_and_their_output_comes_back() {
    let path = write(
        "exec",
        r#"return app {
  init = function(prev) return prev or { out = "none" }, run { id = "echo", argv = { "sh", "-c", "printf hi; printf oops >&2; exit 3" } } end,
  update = function(s, m)
    if m.type == "done" then s.out = m.id .. ":" .. m.stdout .. ":" .. m.stderr .. ":" .. m.status end
    return s
  end,
  view = function(s) return text(s.out) end }"#,
    );
    let rt = runtime();
    let mut app = LuaApp::new(path, exec()).unwrap();
    assert_eq!(frame_texts(&app), ["none"]);
    let done = run_spawns(&rt, app.init());
    assert_eq!(done.len(), 1);
    for msg in done {
        app.update(msg);
    }
    assert_eq!(frame_texts(&app), ["echo:hi:oops:3"]);
}

#[test]
fn a_program_that_cannot_start_reports_why() {
    let path = write(
        "exec-missing",
        r#"return app {
  init = function() return { out = "" }, run { id = "x", argv = { "/nonexistent/program" } } end,
  update = function(s, m) s.out = tostring(m.status) .. " " .. m.error; return s end,
  view = function(s) return text(s.out) end }"#,
    );
    let rt = runtime();
    let mut app = LuaApp::new(path, exec()).unwrap();
    for msg in run_spawns(&rt, app.init()) {
        app.update(msg);
    }
    let out = frame_texts(&app).join("");
    assert!(out.starts_with("nil /nonexistent/program: "), "{out}");
}

#[test]
fn saving_the_file_reloads_it_through_the_watch() {
    let path = write(
        "watched",
        r#"return app {
  init = function(prev) return prev or { n = 0 } end,
  update = function(s) s.n += 1; return s end,
  view = function(s) return text("v1 " .. s.n) end }"#,
    );
    let mut app = LuaApp::new(&path, ScriptOptions::default()).unwrap();
    app.update(Msg::Script(ScriptMsg::Tick { name: "x".into() }));

    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = watch_file(&path, move |msg| {
        let _ = tx.send(msg);
    })
    .unwrap();
    // The changes the watch reports for one save, up to the one that says
    // the file is complete.
    let save = || {
        let mut partial = 0;
        loop {
            match rx
                .recv_timeout(Duration::from_secs(5))
                .expect("a change within 5 s")
            {
                Msg::FileTouched(_) => partial += 1,
                complete @ Msg::FileChanged(_) => return (partial, complete),
                other => panic!("{other:?}"),
            }
        }
    };

    // Written in place: the truncate and the write may be partial; the
    // close says it is done, and that reloads.
    let before = Instant::now();
    std::fs::write(
        &path,
        std::fs::read_to_string(&path).unwrap().replace("v1", "v2"),
    )
    .unwrap();
    let (_, msg) = save();
    assert!(matches!(msg, Msg::FileChanged(seen) if seen >= before));
    app.update(msg);
    assert_eq!(frame_texts(&app), ["v2 1"]);

    // A broken save shows the error over the last good view...
    std::fs::write(&path, "return app {").unwrap();
    let (_, msg) = save();
    app.update(msg);
    let shown = frame_texts(&app);
    assert!(shown[0].starts_with("error: "), "{shown:?}");
    assert_eq!(shown.last().unwrap(), "v2 1");

    // ...until the fix is saved. Editors that save by renaming a new file
    // over the old one are covered too, and the rename alone is complete.
    let tmp = path.with_extension("swp");
    std::fs::write(
        &tmp,
        r#"return app {
  init = function(prev) return prev or { n = 0 } end,
  view = function(s) return text("v3 " .. s.n) end }"#,
    )
    .unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    let (partial, msg) = save();
    assert_eq!(partial, 0, "a rename is reported as complete at once");
    app.update(msg);
    assert_eq!(frame_texts(&app), ["v3 1"]);
}

#[test]
fn a_change_that_may_be_partial_reloads_after_the_settle_time() {
    let path = write(
        "settle",
        "return app { view = function() return text('before') end }",
    );
    let rt = runtime();
    let mut app = LuaApp::new(&path, ScriptOptions::default()).unwrap();
    std::fs::write(
        &path,
        "return app { view = function() return text('after') end }",
    )
    .unwrap();

    // A touch does not reload by itself; it schedules a reload.
    let touched = app.update(Msg::FileTouched(Instant::now()));
    assert_eq!(frame_texts(&app), ["before"]);
    let started = Instant::now();
    let msgs = run_spawns(&rt, touched);
    assert!(started.elapsed() >= SETTLE);
    assert!(matches!(msgs[..], [Msg::FileChanged(_)]), "{msgs:?}");
    for msg in msgs {
        app.update(msg);
    }
    assert_eq!(frame_texts(&app), ["after"]);

    // A complete change cancels a settle still waiting.
    let cmds = flatten(app.update(Msg::FileChanged(Instant::now())));
    assert!(
        cmds.iter()
            .any(|cmd| matches!(cmd, Cmd::CancelSpawn { id } if id == "ulttui-lua/settle")),
        "{cmds:?}"
    );
}

#[test]
fn the_error_banner_sits_above_the_view_and_is_named() {
    let path = write(
        "banner",
        "return app { init = function() return {} end, update = function() error('bad key') end,\n view = function() return text('body') end }",
    );
    let mut app = LuaApp::new(path, ScriptOptions::default()).unwrap();
    let msg = app.on_event(&key_event(Key::Char('a'))).unwrap();
    app.update(msg);
    let frame = app.frame();
    let messages = render_view_messages(&frame).unwrap();
    let banner_id = messages.iter().find_map(|m| match m {
        AppMessage::CreateRegion { id, .. } => Some(*id),
        _ => None,
    });
    assert!(banner_id.is_some());
    let shown = texts(&messages);
    assert!(shown[0].starts_with("error: update: "), "{shown:?}");
    assert!(shown[0].ends_with(":1: bad key"), "{shown:?}");
    assert_eq!(shown.last().unwrap(), "body");

    let mut ids = Vec::new();
    frame.for_each_child(&mut |child| ids.extend(child.widget_id().map(str::to_string)));
    assert_eq!(ids, ["ulttui-lua-error"]);

    let screen = ansi(&frame, 60, 8);
    assert!(screen.contains("error: update:"), "{screen}");
    assert!(screen.contains("body"), "{screen}");
}

#[test]
fn hello_example_counts() {
    let mut app = LuaApp::new(example("hello.luau"), ScriptOptions::default()).unwrap();
    assert_eq!(app.script().error(), None);
    assert!(frame_texts(&app).contains(&"Count: 0".to_string()));
    for key in [Key::Char('+'), Key::Up, Key::Enter] {
        // Enter presses the focused button, "inc".
        let msg = app.on_event(&key_event(key)).unwrap();
        app.update(msg);
    }
    assert!(frame_texts(&app).contains(&"Count: 3".to_string()));
    let msg = app.on_event(&key_event(Key::Char('q'))).unwrap();
    assert!(matches!(app.update(msg), Cmd::Quit));
}

#[test]
fn panes_example_lists_each_pane_and_moves_the_selection() {
    let mut app = LuaApp::new(example("panes.luau"), exec()).unwrap();
    assert_eq!(app.script().error(), None);
    let reply = r#"{"status":"data","type":"panes","panes":[
        {"id":1,"tab_index":0,"active":true,"focused":true,"title":"","foreground":"zsh",
         "pid":10,"cwd":"/home/a","cols":80,"rows":24},
        {"id":4,"tab_index":1,"active":true,"focused":false,"title":"vim x","foreground":null,
         "pid":11,"cwd":null,"cols":80,"rows":24,"status":{"state":"busy","label":"tests"}}]}"#;
    app.update(Msg::Script(ScriptMsg::Done {
        id: "panes".into(),
        stdout: reply.into(),
        stderr: String::new(),
        status: Some(0),
        error: None,
    }));
    let shown = frame_texts(&app);
    let rows: Vec<&String> = shown
        .iter()
        .filter(|t| t.starts_with("* pane") || t.starts_with("  pane"))
        .collect();
    assert_eq!(rows.len(), 2, "{shown:?}");
    assert!(rows[0].starts_with("* pane 1"), "{}", rows[0]);
    assert!(rows[0].contains("zsh") && rows[0].contains("/home/a"));
    assert!(rows[1].contains("vim x") && rows[1].contains("busy: tests"));
    assert!(shown.contains(&"selected: pane 1 in /home/a".to_string()));

    let msg = app.on_event(&key_event(Key::Down)).unwrap();
    app.update(msg);
    assert!(frame_texts(&app).contains(&"selected: pane 4 in ?".to_string()));

    // The refresh timer polls again.
    let cmds = flatten(app.update(Msg::Script(ScriptMsg::Tick {
        name: "refresh".into(),
    })));
    assert!(matches!(cmds.as_slice(), [Cmd::SpawnWithId { id, .. }] if id == "run:panes"));
    assert_eq!(
        app.script().timers(),
        [("refresh".to_string(), Duration::from_secs(2))]
    );
}

#[test]
fn panes_example_without_a_grant_names_the_flag_to_add() {
    let app = LuaApp::new(example("panes.luau"), ScriptOptions::default()).unwrap();
    let error = app.script().error().unwrap();
    assert!(
        error.contains(
            "ulttyctl --json panes is not allowed; open the script with --exec 'ulttyctl --json panes'"
        ),
        "{error}"
    );
    assert!(error.contains("panes.luau:10: run:"), "{error}");
}

fn input_key(key: Key, ctrl: bool, alt: bool, focused: &str) -> Event {
    Event::Key(KeyEvent {
        key,
        modifiers: Modifiers {
            ctrl,
            alt,
            ..Modifiers::default()
        },
        focused_id: Some(focused.into()),
    })
}

/// A script that keeps an input's text and cursor from `input` messages and
/// shows them as `value|cursor`.
fn input_app(name: &str, initial: &str) -> LuaApp {
    let path = write(
        name,
        &format!(
            r#"return app {{
  init = function(prev) return prev or {{ text = "{initial}", cursor = utf8.len("{initial}") }} end,
  update = function(s, m)
    if m.type == "input" and m.id == "name" then
      s.text = m.value
      s.cursor = m.cursor
    elseif m.type == "key" then
      s.last = m.key .. ":" .. tostring(m.focused)
    end
    return s
  end,
  view = function(s)
    return column {{
      input {{ id = "name", value = s.text, cursor = s.cursor }},
      button {{ id = "go", label = "go" }},
      text(s.text .. "|" .. s.cursor .. "|" .. tostring(s.last)),
    }}
  end }}"#
        ),
    );
    LuaApp::new(path, ScriptOptions::default()).unwrap()
}

/// Send `event` to the app, deliver the message it produced (if any), and
/// return the summary line the script renders.
fn press(app: &mut LuaApp, event: Event) -> String {
    // The runtime reads the inputs of the last frame, as it does live.
    let _ = app.frame();
    if let Some(msg) = app.on_event(&event) {
        app.update(msg);
    }
    frame_texts(app).pop().unwrap()
}

#[test]
fn a_script_receives_the_edited_value_and_cursor_from_the_focused_input() {
    let mut app = input_app("input-edit", "héllo wörld");
    let name = |key, ctrl, alt| input_key(key, ctrl, alt, "name");

    assert_eq!(
        press(&mut app, name(Key::Left, false, false)),
        "héllo wörld|10|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Left, true, false)),
        "héllo wörld|6|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Char('X'), false, false)),
        "héllo Xwörld|7|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Backspace, false, false)),
        "héllo wörld|6|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Delete, false, false)),
        "héllo örld|6|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Home, false, false)),
        "héllo örld|0|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Right, false, true)),
        "héllo örld|5|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Char('e'), true, false)),
        "héllo örld|10|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Char('w'), true, false)),
        "héllo |6|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::Char('a'), true, false)),
        "héllo |0|nil"
    );
    assert_eq!(
        press(&mut app, name(Key::End, false, false)),
        "héllo |6|nil"
    );
    assert_eq!(press(&mut app, name(Key::Char('u'), true, false)), "|0|nil");
}

#[test]
fn typed_characters_arrive_one_by_one_at_the_cursor_as_a_paste_would() {
    let mut app = input_app("input-paste", "ad");
    let name = |key| input_key(key, false, false, "name");
    press(&mut app, name(Key::Left));
    let mut last = String::new();
    for c in "bc\u{1F600}".chars() {
        last = press(&mut app, name(Key::Char(c)));
    }
    assert_eq!(last, "abc\u{1F600}d|4|nil");
}

#[test]
fn edits_never_split_an_emoji() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let mut app = input_app("input-emoji", &format!("a{family}b"));
    let name = |key| input_key(key, false, false, "name");
    assert_eq!(
        press(&mut app, name(Key::Left)),
        format!("a{family}b|2|nil")
    );
    assert_eq!(press(&mut app, name(Key::Backspace)), "ab|1|nil");
}

#[test]
fn keys_that_do_not_edit_stay_key_messages_and_no_ops_send_nothing() {
    let mut app = input_app("input-keys", "ab");
    let _ = app.frame();
    // Enter in an input, and typing while a button has focus, are keys.
    assert_eq!(
        press(&mut app, input_key(Key::Enter, false, false, "name")),
        "ab|2|enter:name"
    );
    assert_eq!(
        press(&mut app, input_key(Key::Char('z'), false, false, "go")),
        "ab|2|z:go"
    );
    // Right at the end changes nothing, so the script hears nothing.
    let _ = app.frame();
    assert!(app
        .on_event(&input_key(Key::Right, false, false, "name"))
        .is_none());
}

#[test]
fn edit_false_and_meta_chords_leave_keys_to_the_script() {
    let path = write(
        "input-no-edit",
        r#"return app {
  init = function(prev) return prev or { text = "ab", last = "" } end,
  update = function(s, m)
    if m.type == "key" then s.last = m.key .. ":" .. tostring(m.ctrl) .. ":" .. tostring(m.focused) end
    if m.type == "input" then s.text = m.value end
    return s
  end,
  view = function(s)
    return column {
      input { id = "raw", value = s.text, edit = false },
      input { id = "cooked", value = s.text },
      text(s.text .. "|" .. s.last),
    }
  end }"#,
    );
    let mut app = LuaApp::new(path, ScriptOptions::default()).unwrap();
    // edit = false: Ctrl+A and a letter are plain key messages.
    assert_eq!(
        press(&mut app, input_key(Key::Char('a'), true, false, "raw")),
        "ab|a:true:raw"
    );
    assert_eq!(
        press(&mut app, input_key(Key::Char('x'), false, false, "raw")),
        "ab|x:false:raw"
    );
    // With editing on, a Meta chord is still a key, not text.
    let meta = Event::Key(KeyEvent {
        key: Key::Char('v'),
        modifiers: Modifiers {
            meta: true,
            ..Modifiers::default()
        },
        focused_id: Some("cooked".into()),
    });
    assert_eq!(press(&mut app, meta), "ab|v:false:cooked");
    assert_eq!(
        press(&mut app, input_key(Key::Char('x'), false, false, "cooked")),
        "abx|v:false:cooked"
    );
}

#[test]
fn two_focusable_views_with_one_id_are_a_view_error() {
    let path = write(
        "dup-ids",
        r#"return app { view = function()
  return column { input { id = "a" }, button { id = "a", label = "go" } }
end }"#,
    );
    let app = LuaApp::new(path, ScriptOptions::default()).unwrap();
    let _ = app.frame();
    let error = app.script().error().expect("the view fails");
    assert!(
        error.contains("id \"a\"") && error.contains("unique"),
        "{error}"
    );
}
