//! The script lifecycle: update, timers, commands, sandbox, reload.

use std::time::Duration;

use mlua::Value;
use ulttui_lua::{Command, ExecGrant, Reload, Script, ScriptMsg, ScriptOptions};

use crate::common::{load, messages, texts, write};

fn key(k: &str) -> ScriptMsg {
    ScriptMsg::Key {
        key: k.to_string(),
        ctrl: false,
        alt: false,
        shift: false,
        focused: None,
    }
}

fn int(script: &Script, expr: &str) -> i64 {
    match script.eval_with_state(expr).unwrap() {
        Value::Integer(i) => i,
        Value::Number(n) => n as i64,
        other => panic!("{expr} is {other:?}"),
    }
}

fn string(script: &Script, expr: &str) -> String {
    match script.eval_with_state(expr).unwrap() {
        Value::String(s) => s.to_string_lossy(),
        other => panic!("{expr} is {other:?}"),
    }
}

fn view_texts(script: &Script) -> Vec<String> {
    texts(&messages(&script.view().expect("a view")))
}

const COUNTER: &str = r#"
return app {
  init = function(prev) return prev or { count = 0 } end,
  update = function(state, msg)
    if msg.type == "key" and msg.key == "up" then state.count += 1 end
    return state
  end,
  view = function(state) return column { text("Count: " .. state.count) } end,
}
"#;

#[test]
fn messages_reach_update_with_every_field() {
    let mut script = load(
        "msg-fields",
        r#"
return app {
  init = function() return { seen = {} } end,
  update = function(state, msg)
    local parts = {}
    for _, k in { "type", "key", "ctrl", "alt", "shift", "focused", "id", "name",
                  "stdout", "stderr", "status", "error" } do
      if msg[k] ~= nil then table.insert(parts, k .. "=" .. tostring(msg[k])) end
    end
    table.insert(state.seen, table.concat(parts, " "))
    return state
  end,
  view = function(state) return text(#state.seen) end,
}
"#,
        ScriptOptions::default(),
    );
    script.update(&ScriptMsg::Key {
        key: "x".into(),
        ctrl: true,
        alt: false,
        shift: true,
        focused: Some("inc".into()),
    });
    script.update(&ScriptMsg::Click { id: "go".into() });
    script.update(&ScriptMsg::Tick {
        name: "refresh".into(),
    });
    script.update(&ScriptMsg::Done {
        id: "p".into(),
        stdout: "out".into(),
        stderr: "err".into(),
        status: Some(3),
        error: None,
    });
    assert_eq!(
        string(&script, "table.concat(state.seen, '|')"),
        "type=key key=x ctrl=true alt=false shift=true focused=inc\
         |type=click id=go\
         |type=tick name=refresh\
         |type=done id=p stdout=out stderr=err status=3"
    );
    assert_eq!(script.error(), None);
}

#[test]
fn update_changes_state_and_view() {
    let mut script = load("counter", COUNTER, ScriptOptions::default());
    assert_eq!(view_texts(&script), ["Count: 0"]);
    assert!(script.update(&key("up")).is_empty());
    script.update(&key("up"));
    script.update(&key("down"));
    assert_eq!(int(&script, "state.count"), 2);
    assert_eq!(view_texts(&script), ["Count: 2"]);
}

#[test]
fn update_returning_nil_keeps_the_state_it_mutated() {
    let mut script = load(
        "nil-update",
        r#"return app {
  init = function() return { n = 0 } end,
  update = function(state) state.n += 1 end,
  view = function(state) return text(state.n) end }"#,
        ScriptOptions::default(),
    );
    script.update(&key("a"));
    assert_eq!(int(&script, "state.n"), 1);
}

#[test]
fn timers_come_from_every() {
    let script = load(
        "timers",
        r#"return app { every = { refresh = 2000, blink = 250 },
  view = function() return text("t") end }"#,
        ScriptOptions::default(),
    );
    assert_eq!(
        script.timers(),
        [
            ("blink".to_string(), Duration::from_millis(250)),
            ("refresh".to_string(), Duration::from_secs(2)),
        ]
    );
}

#[test]
fn bad_app_tables_are_reported() {
    let cases = [
        ("return 42", "it must end with `return app { ... }`"),
        ("return {}", "the app has no `view` function"),
        (
            "return { view = function() end, updaet = 1 }",
            "unknown app field 'updaet'",
        ),
        (
            "return { view = function() return text('x') end, every = { fast = 1 } }",
            "app.every.fast: 1 ms is too short",
        ),
        (
            "return app { view = 1 }",
            "app.view must be a function, got integer",
        ),
    ];
    for (i, (source, expected)) in cases.into_iter().enumerate() {
        let mut script = Script::new(
            write(&format!("bad-app-{i}"), source),
            ScriptOptions::default(),
        )
        .unwrap();
        assert_eq!(script.reload(), Reload::Failed, "{source}");
        let error = script.error().unwrap();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn app_rejects_unknown_fields_at_the_call() {
    let path = write(
        "app-unknown",
        "return app {\n view = function() end,\n veiw = 1 }",
    );
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    script.reload();
    assert_eq!(
        script.error().unwrap(),
        format!(
            "{}:1: app: unknown app field 'veiw' (expected one of: init, update, view, every)",
            path.display()
        )
    );
}

const RUNNER: &str = r#"
return app {
  init = function() return { n = 0 } end,
  update = function(state, msg)
    if msg.key == "r" then
      return state, run { id = "ls", argv = { "ls", "-l" } }
    elseif msg.key == "m" then
      return state, { run { id = "a", argv = { "true" } }, quit() }
    end
    return state
  end,
  view = function() return text("x") end,
}
"#;

#[test]
fn run_is_refused_without_a_grant() {
    let mut script = load("run-off", RUNNER, ScriptOptions::default());
    assert!(script.update(&key("r")).is_empty());
    let error = script.error().unwrap();
    let path = crate::common::script_path("run-off");
    assert_eq!(
        error,
        format!(
            "update: {}:6: run: ls -l is not allowed; open the script with --exec 'ls -l' (or a shorter prefix)",
            path.display()
        )
    );
}

#[test]
fn run_allows_only_the_granted_programs() {
    let options = ScriptOptions {
        exec: ExecGrant::Only(vec![words("ls"), words("ulttyctl --json panes")]),
        ..ScriptOptions::default()
    };
    let mut script = load("run-only", RUNNER, options);
    assert_eq!(
        script.update(&key("r")),
        [Command::Run {
            id: "ls".into(),
            argv: vec!["ls".into(), "-l".into()]
        }]
    );
    assert_eq!(script.error(), None);
    // `m` runs `true`, which was not granted: nothing runs, not even `quit()`.
    assert!(script.update(&key("m")).is_empty());
    let path = crate::common::script_path("run-only");
    assert_eq!(
        script.error().unwrap(),
        format!(
            "update: {}:8: run: true is not allowed; open the script with --exec true (allowed: ls, 'ulttyctl --json panes')",
            path.display()
        )
    );
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

#[test]
fn exec_grants_are_command_line_prefixes_compared_as_written() {
    let grant = ExecGrant::Only(vec![words("git status")]);
    assert!(grant.allows(&words("git status")));
    assert!(grant.allows(&words("git status --short")));
    assert!(!grant.allows(&words("git")));
    assert!(!grant.allows(&words("git commit")));
    assert!(!grant.allows(&words("/usr/bin/git status")));
    assert!(!grant.allows(&words("env git status")));
    assert!(!grant.allows(&words("gi")));
    // A bare program name allows any arguments.
    let bare = ExecGrant::Only(vec![words("git")]);
    assert!(bare.allows(&words("git")));
    assert!(bare.allows(&words("git -c alias.x=!sh x")));
    assert!(ExecGrant::Any.allows(&words("/usr/bin/git")));
    assert!(!ExecGrant::Deny.allows(&words("git")));
}

#[test]
fn grants_on_program_runners_are_refused_and_point_to_allow_exec() {
    // One of each class, by name and by path.
    for runner in [
        "sh",
        "/bin/sh",
        "bash -c",
        "zsh",
        "env",
        "/usr/bin/env FOO=1",
        "xargs -0",
        "sudo",
        "timeout 5",
        "nohup",
        "ssh host",
        "python3 -c",
        "/usr/bin/python3",
        "node",
        "awk",
        "busybox sh",
        "ulttyctl",
        "/opt/ultty/ulttyctl",
        "ulttui-lua",
    ] {
        let error = ulttui_lua::ExecGrant::parse_grant(runner).unwrap_err();
        assert!(error.contains("--allow-exec"), "{runner}: {error}");
        assert!(error.contains("--exec"), "{runner}: {error}");
    }
    assert_eq!(
        ulttui_lua::ExecGrant::parse_grant("/bin/sh").unwrap_err(),
        "--exec \"/bin/sh\": sh runs whatever it is told to, so this grant would allow anything; say so with --allow-exec"
    );
    assert_eq!(
        ulttui_lua::ExecGrant::parse_grant("ulttyctl").unwrap_err(),
        "--exec \"ulttyctl\": a bare ulttyctl can open panes running anything; grant one command such as --exec 'ulttyctl --json panes', or allow anything with --allow-exec"
    );
    // A subcommand makes ulttyctl one fixed command; other programs are
    // allowed bare, broad as git is.
    for allowed in [
        "ulttyctl --json panes",
        "ulttyctl panes",
        "git status",
        "git",
        "ls -l",
        "cargo test",
        "shellcheck",
        "envsubst",
    ] {
        assert!(
            ulttui_lua::ExecGrant::parse_grant(allowed).is_ok(),
            "{allowed} should be allowed"
        );
    }
    assert!(ulttui_lua::PROGRAM_RUNNERS.contains(&"sh"));
    assert!(ulttui_lua::NEED_SUBCOMMAND.contains(&"ulttyctl"));
}

#[test]
fn exec_grants_split_on_whitespace_with_quotes() {
    assert_eq!(
        ExecGrant::parse_grant("ulttyctl --json panes").unwrap(),
        words("ulttyctl --json panes")
    );
    assert_eq!(
        ExecGrant::parse_grant(r#"  git  log --format="%h %s" 'a b'  "#).unwrap(),
        ["git", "log", "--format=%h %s", "a b"]
    );
    assert_eq!(
        ExecGrant::parse_grant(r#"touch """#).unwrap(),
        ["touch", ""]
    );
    assert!(ExecGrant::parse_grant("").is_err());
    assert!(ExecGrant::parse_grant("   ").is_err());
    assert!(ExecGrant::parse_grant("git 'log").is_err());
}

#[test]
fn a_script_can_catch_a_denied_run() {
    let source = r#"
return app {
  init = function() return { err = "" } end,
  update = function(state, msg)
    local ok, err = pcall(run, { id = "x", argv = { "rm", "-rf", "/" } })
    state.err = ok and "ran" or tostring(err)
    return state
  end,
  view = function() return text("x") end,
}
"#;
    let mut script = load("run-pcall", source, ScriptOptions::default());
    script.update(&key("a"));
    // Caught: no banner, and the message is the script's to show.
    assert_eq!(script.error(), None);
    let caught = string(&script, "state.err");
    assert!(
        caught.contains("run: rm -rf / is not allowed; open the script with --exec 'rm -rf /'"),
        "{caught}"
    );
}

#[test]
fn run_and_quit_become_commands_with_exec_any() {
    let options = ScriptOptions {
        exec: ExecGrant::Any,
        ..ScriptOptions::default()
    };
    let mut script = load("run-on", RUNNER, options);
    assert_eq!(
        script.update(&key("r")),
        [Command::Run {
            id: "ls".into(),
            argv: vec!["ls".into(), "-l".into()]
        }]
    );
    assert_eq!(
        script.update(&key("m")),
        [
            Command::Run {
                id: "a".into(),
                argv: vec!["true".into()]
            },
            Command::Quit
        ]
    );
    assert_eq!(script.error(), None);
}

#[test]
fn malformed_run_calls_are_errors() {
    let options = ScriptOptions {
        exec: ExecGrant::Any,
        ..ScriptOptions::default()
    };
    let cases = [
        (r#"run { argv = { "ls" } }"#, "run: missing id"),
        (
            r#"run { id = "x", argv = {} }"#,
            "run: argv must name a program",
        ),
        (
            r#"run { id = "x", argv = { 1 } }"#,
            "run: argv[1] is a integer",
        ),
        (
            r#"run { id = "x", argv = { "ls" }, shell = true }"#,
            "run: unknown field 'shell'",
        ),
        (r#"run("ls")"#, "run: expected a table"),
    ];
    for (i, (call, expected)) in cases.into_iter().enumerate() {
        let source = format!(
            "return app {{ update = function(s) return s, {call} end, view = function() return text('x') end }}"
        );
        let mut script = load(&format!("bad-run-{i}"), &source, options.clone());
        script.update(&key("a"));
        let error = script.error().unwrap();
        assert!(error.contains(expected), "{call}: {error}");
    }
}

#[test]
fn init_can_start_commands() {
    let options = ScriptOptions {
        exec: ExecGrant::Any,
        ..ScriptOptions::default()
    };
    let mut script = Script::new(
        write(
            "init-cmd",
            r#"return app { init = function() return {}, run { id = "p", argv = { "true" } } end,
  view = function() return text("x") end }"#,
        ),
        options,
    )
    .unwrap();
    assert_eq!(
        script.reload(),
        Reload::Loaded(vec![Command::Run {
            id: "p".into(),
            argv: vec!["true".into()]
        }])
    );
}

#[test]
fn the_sandbox_has_no_way_out() {
    let script = load(
        "sandbox",
        r#"
local function blocked(f) local ok = pcall(f); return not ok end
return app {
  init = function()
    return {
      io = io == nil,
      execute = os.execute == nil,
      require = require == nil,
      loadstring = loadstring == nil,
      debug = debug == nil,
      getenv = os.getenv == nil,
      frozen_lib = blocked(function() string.upper = nil end),
      clock = type(os.clock()) == "number",
    }
  end,
  view = function() return text("x") end,
}
"#,
        ScriptOptions::default(),
    );
    for check in [
        "io",
        "execute",
        "require",
        "loadstring",
        "debug",
        "getenv",
        "frozen_lib",
        "clock",
    ] {
        assert!(
            matches!(
                script.eval_with_state(&format!("state.{check}")).unwrap(),
                Value::Boolean(true)
            ),
            "sandbox check {check} failed"
        );
    }
}

#[test]
fn globals_do_not_leak_from_one_version_to_the_next() {
    let path = write(
        "globals",
        "leaked = 1\nreturn app { view = function() return text(tostring(leaked)) end }",
    );
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    script.reload();
    assert_eq!(view_texts(&script), ["1"]);
    std::fs::write(
        &path,
        "return app { view = function() return text(tostring(leaked)) end }",
    )
    .unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    assert_eq!(view_texts(&script), ["nil"]);
}

#[test]
fn a_version_cannot_reach_past_its_environment() {
    let path = write(
        "env-escape",
        r#"
local reached = {}
if getfenv then reached.getfenv = pcall(function() getfenv(0).leaked = "yes" end) end
local meta = getmetatable(getfenv and getfenv(1) or {})
if type(meta) == "table" and meta.__index then
  reached.index = pcall(function() meta.__index.text = function() return "hijacked" end end)
end
return app { view = function() return text(tostring(reached.getfenv) .. tostring(reached.index)) end }
"#,
    );
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    script.reload();
    assert_eq!(script.error(), None);
    assert_eq!(view_texts(&script), ["nilnil"]);
    std::fs::write(
        &path,
        "return app { view = function() return text(tostring(leaked) .. ' ' .. 'b') end }",
    )
    .unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    assert_eq!(view_texts(&script), ["nil b"]);
}

#[test]
fn a_failed_reload_does_not_touch_the_running_state() {
    let path = write("init-mutates", COUNTER);
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    script.reload();
    script.update(&key("up"));
    std::fs::write(
        &path,
        "return app { init = function(p) p.count = 99; return p end,\n view = function() error('no') end }",
    )
    .unwrap();
    assert_eq!(script.reload(), Reload::Failed);
    assert_eq!(int(&script, "state.count"), 1);
    assert_eq!(view_texts(&script), ["Count: 1"]);
}

#[test]
fn an_update_error_clears_on_the_next_good_update() {
    let mut script = load(
        "update-recovers",
        r#"return app { init = function() return { n = 0 } end,
  update = function(s, m) if m.key == "x" then error("bad") end s.n += 1 return s end,
  view = function(s) return text(s.n) end }"#,
        ScriptOptions::default(),
    );
    script.update(&key("x"));
    assert!(script.error().unwrap().contains("bad"));
    script.update(&key("a"));
    assert_eq!(script.error(), None);
    assert_eq!(view_texts(&script), ["1"]);
}

#[test]
fn json_decode() {
    let script = load(
        "json",
        r#"
local v = json.decode('{"panes":[{"id":3,"cwd":null,"ok":true,"r":1.5}],"n":"x"}')
local bad = pcall(json.decode, "{oops")
return app {
  init = function()
    local p = v.panes[1]
    return { id = p.id, cwd_nil = p.cwd == nil, ok = p.ok, r = p.r, n = v.n, len = #v.panes,
             bad = not bad }
  end,
  view = function() return text("x") end }
"#,
        ScriptOptions::default(),
    );
    assert_eq!(int(&script, "state.id"), 3);
    assert_eq!(int(&script, "state.len"), 1);
    assert_eq!(string(&script, "state.n"), "x");
    assert_eq!(string(&script, "tostring(state.r)"), "1.5");
    for flag in ["cwd_nil", "ok", "bad"] {
        assert!(matches!(
            script.eval_with_state(&format!("state.{flag}")).unwrap(),
            Value::Boolean(true)
        ));
    }
}

#[test]
fn a_runaway_loop_is_stopped() {
    let mut script = load(
        "runaway",
        r#"return app { update = function(s) while true do end end,
  view = function() return text("still here") end }"#,
        ScriptOptions {
            call_budget: Duration::from_millis(50),
            ..ScriptOptions::default()
        },
    );
    script.update(&key("a"));
    assert!(
        script.error().unwrap().contains("script took too long"),
        "{:?}",
        script.error()
    );
    assert_eq!(view_texts(&script), ["still here"]);
}

#[test]
fn update_errors_keep_the_state_and_show_file_and_line() {
    let mut script = load(
        "update-error",
        "return app {\n init = function() return { n = 1 } end,\n update = function(s, m)\n  local next = { n = 99 }\n  error('boom')\n  return next\n end,\n view = function(s) return text(s.n) end }",
        ScriptOptions::default(),
    );
    script.update(&key("a"));
    let path = crate::common::script_path("update-error");
    assert_eq!(
        script.error().unwrap(),
        format!("update: {}:5: boom", path.display())
    );
    assert_eq!(int(&script, "state.n"), 1);
    assert_eq!(view_texts(&script), ["1"]);
}

#[test]
fn hot_reload_keeps_state_and_recovers_from_errors() {
    let path = write("reload", COUNTER);
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    script.update(&key("up"));
    script.update(&key("up"));

    // Same bytes: nothing to do.
    assert_eq!(script.reload(), Reload::Unchanged);

    // A new label: the count survives through init(prev).
    std::fs::write(&path, COUNTER.replace("Count: ", "Total: ")).unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    assert_eq!(view_texts(&script), ["Total: 2"]);

    // A syntax error: the last good version keeps running with its state.
    std::fs::write(
        &path,
        "return app {\n view = function(state)\n  return text(\n end }",
    )
    .unwrap();
    assert_eq!(script.reload(), Reload::Failed);
    let error = script.error().unwrap();
    assert!(
        error.starts_with(&format!("{}:4:", path.display())),
        "{error}"
    );
    script.update(&key("up"));
    assert_eq!(view_texts(&script), ["Total: 3"]);

    // A version whose init fails is also kept out.
    std::fs::write(
        &path,
        "return app {\n init = function() error('nope') end,\n view = function() return text('x') end }",
    )
    .unwrap();
    assert_eq!(script.reload(), Reload::Failed);
    assert_eq!(
        script.error().unwrap(),
        format!("init: {}:2: nope", path.display())
    );
    assert_eq!(view_texts(&script), ["Total: 3"]);

    // So is one whose view fails on the carried-over state.
    std::fs::write(
        &path,
        "return app {\n view = function(s) return text(s.count .. nil) end }",
    )
    .unwrap();
    assert_eq!(script.reload(), Reload::Failed);
    assert!(script.error().unwrap().starts_with("view: "));

    // The fix loads, clears the error and still has the count.
    std::fs::write(&path, COUNTER.replace("Count: ", "Fixed: ")).unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    assert_eq!(script.error(), None);
    assert_eq!(view_texts(&script), ["Fixed: 3"]);
}

#[test]
fn a_failing_view_falls_back_to_the_last_one_that_worked() {
    let mut script = load(
        "view-fallback",
        r#"return app {
  init = function() return { n = 1 } end,
  update = function(s, m) if m.key == "x" then s.n = nil else s.n = 2 end return s end,
  view = function(s) return text("n=" .. s.n) end }"#,
        ScriptOptions::default(),
    );
    assert_eq!(view_texts(&script), ["n=1"]);
    script.update(&key("x"));
    assert_eq!(view_texts(&script), ["n=1"]);
    assert!(script.error().unwrap().starts_with("view: "));
    script.update(&key("y"));
    assert_eq!(view_texts(&script), ["n=2"]);
    assert_eq!(script.error(), None);
}

#[test]
fn the_first_load_can_fail_and_a_later_one_succeed() {
    let path = write("first-fails", "return app {");
    let mut script = Script::new(&path, ScriptOptions::default()).unwrap();
    assert_eq!(script.reload(), Reload::Failed);
    assert!(!script.is_loaded());
    assert!(script.view().is_none());
    std::fs::write(&path, COUNTER).unwrap();
    assert!(matches!(script.reload(), Reload::Loaded(_)));
    assert_eq!(view_texts(&script), ["Count: 0"]);
}

#[test]
fn print_goes_to_the_log_not_the_terminal() {
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        load(
            "print",
            "print(\"hello\", 42, nil, true)\nreturn app { view = function() return text(\"x\") end }",
            ScriptOptions::default(),
        );
    });
    let logged = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("hello\t42\tnil\ttrue"), "{logged}");
    assert!(logged.contains("print.luau:1"), "{logged}");
}
