# Luau apps (`ulttui-lua`)

`ulttui-lua` runs a terminal UI written in [Luau](https://luau.org) and
reloads it every time the file is saved. Inside Ultty it draws natively;
everywhere else it draws with ANSI, like any other ulttui app.

```sh
cargo run -p ulttui-lua -- examples/lua/hello.luau
cargo run -p ulttui-lua -- --allow-exec examples/lua/panes.luau
```

Options:

| Option         | Meaning                                                        |
|----------------|----------------------------------------------------------------|
| `--allow-exec` | Let the script start processes with `run{}`. Off by default.   |
| `--log FILE`   | Write logs to `FILE` (the terminal is the UI, so never stderr). |

This is a prototype. The API below is small on purpose and may change.

## The app table

A script returns an app table, Elm style:

```lua
return app {
  init = function(prev) return prev or { count = 0 } end,
  update = function(state, msg)
    if msg.type == "key" and msg.key == "up" then state.count += 1 end
    return state
  end,
  view = function(state)
    return column { gap = 1, text("Count: " .. state.count), button { id = "inc", label = "+1" } }
  end,
  every = { refresh = 2000 },
}
```

| Field    | Required | Meaning |
|----------|----------|---------|
| `init`   | no  | `init(prev)` returns the state, and optionally commands. `prev` is the state of the version that was running before a reload, `nil` on the first load. Without `init` the state starts as `{}` and is kept across reloads. |
| `update` | no  | `update(state, msg)` returns the new state and optionally commands. Returning `nil` keeps the state (handy when it was changed in place). |
| `view`   | yes | `view(state)` returns a view built from the constructors below (a plain string is shown as text). |
| `every`  | no  | Named timers: `{ name = milliseconds }`. Each fires `{ type = "tick", name = name }`. At least 10 ms. |

`app { ... }` only checks the table (unknown fields are an error at the line
that wrote them); returning a plain table works too.

Commands are the second return value of `init` or `update`: one command, or
an array of them.

## Messages

`msg.type` says what happened:

| `type`  | Fields | When |
|---------|--------|------|
| `key`   | `key`, `ctrl`, `alt`, `shift`, `focused` | A key was pressed. `key` is the character (`"a"`, `"+"`) or a name: `space`, `enter`, `escape`, `backspace`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `delete`, `insert`, `f1`..`f12`. `focused` is the id of the focused widget, if any, so Enter on a button arrives as `key = "enter", focused = "<button id>"`. |
| `click` | `id` | A widget was clicked (native mode). |
| `input` | `id`, `value`, `cursor` | A key edited the focused `input` (see [Text input](#text-input)). |
| `tick`  | `name` | A timer from `every` fired. |
| `done`  | `id`, `stdout`, `stderr`, `status`, `error` | A process started with `run{}` finished. `status` is the exit code (`nil` if it was killed by a signal or never started); `error` says why it could not start. |

Tab and Shift+Tab move focus between buttons and inputs and are not passed
to the script. Ctrl+C quits.

## Commands

| Command | Meaning |
|---------|---------|
| `run { id = "panes", argv = { "ulttyctl", "--json", "panes" } }` | Start a process (no shell, stdin closed). Its output arrives as a `done` message with the same `id`. Running an `id` again cancels and kills the previous run (the process itself, not processes it started in the background). Needs `--allow-exec`: without it `run` raises an error naming the flag. |
| `quit()` | Exit. |

## Views

Every constructor takes a table of props; containers take their children in
the array part. Children may be views, strings or numbers (shown as text),
or arrays of children (spliced in, so a list built with `table.insert` can be
passed directly). `nil` and `false` children are skipped, so
`cond and text("x")` works. An unknown prop, a value of the wrong type, or a
missing required prop is an error that names the file and line.

| Constructor | Array part | Props (besides the layout props) | Notes |
|-------------|------------|----------------------------------|-------|
| `text`     | one string | `fg`, `bg`, `bold` | `text("hi")` is short for `text { "hi" }`. |
| `column`   | children | `gap`, `padding`, `border`, `border_color`, `bg`, `align`, `justify` | Children top to bottom. |
| `row`      | children | same as `column` | Children side by side. |
| `card`     | children | `title` | ulttui `Card`: a raised, bordered panel. |
| `scroll`   | children | | ulttui `ScrollView` around a column of the children. |
| `list`     | | `items` (required), `selected` | Bordered list of strings; `selected` is 1-based. |
| `progress` | | `value` (required, 0 to 1), `label` | |
| `spinner`  | | `frame`, `label` | Without `frame` the spinner follows the clock, so it moves whenever the view redraws (use a timer to redraw). |
| `button`   | | `label` (required), `variant` | Focusable when it has an `id`. `variant`: `default`, `primary`, `danger`, `success`. |
| `input`    | | `value`, `cursor`, `placeholder` | Focusable when it has an `id`. The script owns the text and cursor; see [Text input](#text-input). |

Layout props, accepted by every constructor:

| Prop | Value |
|------|-------|
| `id` | String. The widget's id: the focus target for buttons and inputs, and the region's `id` metadata (shown by `ulttyctl regions`). |
| `width`, `height`, `min_width`, `max_width`, `min_height`, `max_height` | A size: a number of cells, a percentage string such as `"50%"`, or `"auto"`. |
| `grow`, `shrink` | Flex grow and shrink factors. |

Value types of the other props:

| Prop | Value |
|------|-------|
| `fg`, `bg`, `border_color` | A colour: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, the `bright_` forms of the first eight, `"#rrggbb"`, or a 0-255 palette index. |
| `bold` | Boolean. |
| `gap`, `padding` | Cells. A bordered box keeps its children inside the border, so the padding is measured from the border inwards. |
| `border` | `none`, `thin`, `thick`, `double`, `rounded`, `dashed`, `ascii`. |
| `align` | Cross-axis alignment of children: `start`, `center`, `end`, `stretch`. Native mode only. |
| `justify` | Main-axis distribution: `start`, `center`, `end`, `between`, `around`, `evenly`. Native mode only. |
| `title`, `label`, `value` (input), `placeholder` | String. |
| `items` | Array of strings. |
| `value` (progress), `selected`, `frame` | Number. |

The mapping lives in one table, `ulttui_lua::node::CONSTRUCTORS`; a test
checks that every constructor and prop in it is named on this page.

## Text input

An `input` with an `id` is edited by the runtime, and the script keeps the
result. When a key arrives while that input has focus, the runtime applies it
to the `value` and `cursor` the input showed in the last view, and sends the
script an `input` message instead of a `key` message:

```lua
update = function(state, msg)
  if msg.type == "input" and msg.id == "name" then
    state.name, state.cursor = msg.value, msg.cursor
  end
  return state
end,
view = function(state)
  return input { id = "name", value = state.name, cursor = state.cursor }
end,
```

`cursor` is the number of grapheme clusters before the cursor, so `0` is the
start and the number of clusters in `value` is the end; edits never
split a cluster such as `e` plus an accent or a family emoji. Without a
`cursor` prop the cursor is at the end. A key that changes neither text nor
cursor (Left at the start) sends nothing.

| Keys | Edit |
|------|------|
| printable characters | Insert at the cursor. |
| Left, Right | Move one cluster. |
| Ctrl+Left / Ctrl+Right, Alt+Left / Alt+Right, Alt+B / Alt+F | Move to the start of the previous word, or the end of the next. A word is a run of non-space characters. |
| Home, End, Ctrl+A, Ctrl+E | Move to the start or end. |
| Backspace, Delete | Delete the cluster before or at the cursor. |
| Ctrl+W, Alt+Backspace | Delete back to the start of the previous word. |
| Ctrl+U, Ctrl+K | Delete everything before, or from, the cursor. |

Every other key (Enter, Escape, Up, Down, other Ctrl or Alt combinations),
and any key while another widget has focus, still arrives as a `key`
message. The framework has no paste event yet: pasted text arrives as
typed characters, each inserted at the cursor in turn.

A focused input shows its cursor: in ANSI mode as the reversed cell under
it, in native mode as a caret the terminal draws (none from a terminal that lacks `CAP_INPUT_CURSOR`).

## Other globals

- `print(...)` writes to the log (see `--log`) with the file and line it was
  called from; the terminal is the UI, so nothing is printed to it.
- `json.decode(string)` turns JSON into Lua values: objects and arrays become
  tables (arrays 1-based), `null` becomes `nil`. Bad JSON is an error.
- The Luau standard library minus anything that reaches outside: `string`,
  `table`, `math`, `utf8`, `bit32`, `buffer`, `coroutine`, and `os` (which in
  Luau only has `clock`, `date`, `difftime` and `time`).

## Sandbox

Scripts run in Luau's sandbox mode: the globals and libraries are read-only,
and each loaded version gets its own global environment, so a global set by
one version is gone in the next. There is no `io`, no `os.execute`, no
`require`, no `loadstring`, no `getfenv`/`setfenv` and no `debug`. The
only way to affect the system is `run{}`, and only with `--allow-exec`. A
call into the script (load, `init`, `update`, `view`) that runs Luau code for
more than a second is stopped with an error; the check runs at loops and
calls, so a single slow library call (a pathological `string.find` pattern)
can overrun it. The VM is capped at 256 MiB.

## Hot reload and errors

`ulttui-lua` watches the script's file (through its directory, so editors
that save by renaming a new file over the old one are seen) and nothing
else. A change that says the file is complete (it was written and closed,
or renamed into place) reloads at once; any other change (a create, a
truncate, one write of several, which is all some platforms report) reloads
after 10 ms with no further change. The watch hands each change straight to
the event loop, waking it. A reload reads the file and, if the bytes differ,
loads it: the new chunk must return an app table, its
`init(prev)` must return a state, and its `view` must build a view. If all of
that works, the new version replaces the old one, keeping the state that
`init` returned. If anything fails, the last good version keeps running with
its state (the new `init` gets a copy of it, so changes it made are
dropped), and a red banner (region id `ulttui-lua-error`) above it shows the
error with its file and line. The next good load clears it.

Errors at run time are shown the same way. A failing `update` keeps the
state value it was given, but changes it had already made to that table in
place stay; its banner clears on the next `update` that works. A failing
`view` shows the last view that built until `view` works again.

With `--log FILE`, each frame drawn after a reload logs `seen_to_frame_us`
(from the watch seeing the change to the backend returning from drawing the
frame), split into `wait_us` (until the event loop starts the reload,
including any settle delay), `reload_us` (compile, `init` and the trial
`view`) and `render_us`, and `frame_unix_us` (wall clock, to compare with
the time the file was written).

Measured with `scripts/e2e/lua_reload_latency.py` on `hello.luau` (debug
build, 20 saves by rename, file write to frame), 2026-11-10:

| Mode | Median | p90 | Max | Where the time goes (medians) |
|------|--------|-----|-----|-------------------------------|
| ANSI (pty) | 0.8 ms | 1.1 ms | 3.3 ms | wait 0.0, reload 0.3, render 0.2 |
| Native (headless Ultty) | 1.2 ms | 7.1 ms | 10.9 ms | wait 0.0, reload 0.3, render 0.3 |

Before the event loop slept on wakes (it polled every 16 ms, waited 10 ms
after every change, and waited out a 16 ms read for the terminal's reply
after each native frame) the same runs took 23.4 ms (ANSI) and 38.1 ms
(native) at the median. The native tail is the occasional frame whose
socket write waits for a busy terminal.

## Example: a counter

[`examples/lua/hello.luau`](../examples/lua/hello.luau):

```lua
return app {
  init = function(prev) return prev or { count = 0 } end,
  update = function(state, msg)
    if msg.type ~= "key" then return state end
    local key = msg.key
    if key == "enter" then
      key = ({ inc = "+", dec = "-", reset = "r" })[msg.focused] or key
    end
    if key == "+" or key == "=" or key == "up" then return { count = state.count + 1 }
    elseif key == "-" or key == "down" then return { count = state.count - 1 }
    elseif key == "r" then return { count = 0 }
    elseif key == "q" then return state, quit() end
    return state
  end,
  view = function(state)
    local colour = state.count > 0 and "green" or state.count < 0 and "red" or nil
    return column {
      gap = 1, padding = 1,
      text { "Hello from Luau", bold = true },
      text { "Count: " .. state.count, id = "count", fg = colour },
      row { gap = 2,
        button { id = "dec", label = "-1" },
        button { id = "inc", label = "+1", variant = "primary" },
        button { id = "reset", label = "Reset" } },
    }
  end,
}
```

## Example: a live pane list

[`examples/lua/panes.luau`](../examples/lua/panes.luau) runs
`ulttyctl --json panes` when it starts and every 2 seconds, and lists each
pane's id, tab, program, status and working directory. Up and down (or `k`
and `j`) move the selection, `g` refreshes now. It needs `--allow-exec`;
without it the banner says so.

```lua
local function poll()
  return run { id = "panes", argv = { "ulttyctl", "--json", "panes" } }
end

return app {
  init = function(prev)
    return prev or { panes = {}, selected = 1 }, poll()
  end,
  every = { refresh = 2000 },
  update = function(state, msg)
    if msg.type == "tick" then return state, poll() end
    if msg.type == "done" and msg.status == 0 then
      state.panes = json.decode(msg.stdout).panes or {}
    end
    -- ... keys move state.selected ...
    return state
  end,
  view = function(state)
    local rows = {}
    for _, pane in state.panes do table.insert(rows, describe(pane)) end
    return column { text { "Ultty panes", bold = true },
                    list { id = "panes", items = rows, selected = state.selected, grow = 1 } }
  end,
}
```

## Testing

- `cargo test -p ulttui-lua`: constructors and props, messages, timers, the
  `--allow-exec` gate, the sandbox, and reloads (including a broken save and
  its fix) driven through the library with script files under `target/`.
- `scripts/e2e/lua-panes.sh`: runs `panes.luau` in a headless Ultty, checks
  `ulttyctl regions` lists a row per pane, edits a label in a copy of the
  script and checks the change reaches `regions` within a second, checks a
  broken save shows the banner over the running app, measures the native
  reload latency of `hello.luau` in a second tab; then runs
  `hello.luau` under a plain pty and checks its ANSI output.
- `scripts/e2e/lua_reload_latency.py`: measures the time from writing the file to
  the next frame over 20 reloads (ANSI with `--spawn`; the e2e script runs it
  natively).
