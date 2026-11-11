# User overrides for ulttui apps

Any app built with ulttui (including `ulttui-lua` scripts) reads a file of
your changes when it starts, and again whenever the file changes. With it
you can recolour, hide or relabel any widget that has an id, and remap keys,
without the app's source. The file is TOML, or Luau when the changes are
computed (see [Luau overrides](#luau-overrides)). It works the same inside Ultty and in any other
terminal. The design is [ADR 0023](adr/0023-ulttui-overrides.md).

## Where the file goes

`~/.config/ulttui/overrides/<app>.toml`, `<app>.luau`, or both
(`$XDG_CONFIG_HOME` instead of `~/.config` when it is set), where `<app>` is
the name you ran the app by: `todo-app` for `todo-app`, `counter-app` for
`./target/debug/counter-app`. For `ulttui-lua` it is the script's name
without `.luau`: `ulttui-lua examples/lua/hello.luau` reads `hello.toml`
and `hello.luau` from the overrides directory (not the script itself).

| Variable | Effect |
|----------|--------|
| `ULTTUI_OVERRIDES=<path>` | Read this file instead: Luau if it ends in `.luau`, TOML otherwise. |
| `ULTTUI_NO_OVERRIDES=1` | No overrides at all (any value but empty or `0`). Wins over `ULTTUI_OVERRIDES`. |

An app can choose its own name with `App::app_name`, or its own file with
`App::override_file`.

## Example

```toml
# ~/.config/ulttui/overrides/hello.toml

[widget.count]
fg = "yellow"
bold = true

[widget.reset]
hidden = true

[widget.inc]
text = "more"
bg = "#204020"

[keys]
"k" = "up"
"j" = "down"
"ctrl+n" = "down"
"/" = { focus = "inc" }
```

## Widgets

A `[widget.<id>]` table changes the widget whose id is `<id>`. Ids are what
the app passes to `.id(...)` (in Rust) or `id = ...` (in Luau). Inside Ultty,
`ulttyctl regions` lists them.

| Setting | Value | Effect |
|---------|-------|--------|
| `fg` | colour | Text colour over the widget's whole box, borders included, over what the app chose. Not applied while the widget has focus, so its focus colours stay visible (as for `bg`). |
| `bg` | colour | Background of the widget's box. |
| `bold` | `true` / `false` | Bold on or off. |
| `hidden` | `true` / `false` | The widget and everything in it are not drawn. In a column or row the others close up over it; in Ultty the region gets `display: none` and is left out of the accessibility tree. Hidden widgets cannot be focused. |
| `text` | string | New text for `Text`, `RichText`, `Title`, `Body`, `Muted`, `Accent`, or a button's label. The widget resizes to fit. On other widgets (containers, lists, inputs) it does nothing, and it never reaches the widget's children. |

A colour is a name (`black`, `red`, `green`, `yellow`, `blue`, `magenta`,
`cyan`, `white`, `gray`, or `bright_` and one of the first eight), `"#rrggbb"`,
or a palette index 0-255 written as a number. These are the colours
`ulttui-lua` scripts use.

The root of the app's view (if it has an id) takes `fg`, `bg` and `bold`,
which go beneath toasts rather than over them; `hidden` and `text` on the
root are ignored, with a warning in the log. Widgets
without an id cannot be targeted. In Rust apps that includes plain `Text`
and the layout containers; in `ulttui-lua` every constructor takes `id`.

Inside `Stack`, `DockLayout` and `Modal`, a hidden widget leaves blank space
in other terminals (Ultty's layout closes it up).

## Keys

`[keys]` maps a key to another key, or to `{ focus = "<id>" }`:

```toml
[keys]
"j" = "down"               # j acts as Down
"ctrl+p" = "shift+tab"     # ctrl+p moves focus back
"/" = { focus = "search" } # / jumps to the search input
```

A key is a name with optional `ctrl+`, `alt+` and `shift+` in front. Names
are the ones `ulttui-lua` gives `msg.key`: the character itself (`"a"`,
`"?"`, `"+"`), `space`, `enter`, `escape` (or `esc`), `backspace`, `tab`,
`up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `delete`,
`insert`, `f1` to `f12`. `ctrl++` is Ctrl and plus.

- For characters shift is not compared: write `"A"`, not `"shift+a"`.
- For named keys it is: `"tab"` and `"shift+tab"` are different keys.
- While a text input has focus, a character typed without ctrl or alt is
  text, not a command: `"j" = "down"` and `"/" = { focus = "search" }` still
  let you type `j` and `/` into the search box. Chords with ctrl or alt are
  remapped there as everywhere.
- A key held with super (meta) is never remapped.
- A remapped key is not remapped again, so `"j" = "k"` and `"k" = "j"` swap
  them.
- The new key goes everywhere the old one would have: to an open screen, the
  app, and Tab navigation.
- A focus jump uses up the key. If the target is not focusable right now
  (hidden, or not on screen), the key goes to the app unchanged.

## Changes and mistakes

The app checks the file before each frame it draws, so a saved change shows
on the next key, timer tick or update. An idle app does not redraw just
for the file.

A mistake in the file never stops or crashes the app. If the file is not
valid TOML, or has an unknown section or setting, a colour or key that is
not one, or a value of the wrong type, the whole file is ignored and the
reason is logged at `warn` (path, and what is wrong and where, such as
`widget.status.fg: 'purple' is not a colour`). If an edit breaks a file that
was working, the last good version stays in force until the file is fixed.
Deleting the file turns overrides off. A missing file is not an error.
Only a regular file of at most 1 MiB is read: a directory, FIFO or device
(`ULTTUI_OVERRIDES=/dev/zero`) or a bigger file is logged and ignored.

ulttui apps log through `tracing`, so where the message goes is up to the
app; `ulttui-lua` writes it to its `--log` file.

## Luau overrides

A `<app>.luau` file computes overrides instead of listing them. It returns
a table with a `widget` function, a `key` function, or both:

```lua
-- ~/.config/ulttui/overrides/hello.luau
return {
  -- Called for each widget with an id. Return the same fields as a
  -- [widget.<id>] table, or nil to leave the widget alone.
  widget = function(id, info)
    -- info.role: "button", "input", "heading", ..., or "auto"
    -- info.text: the widget's text as the app wrote it (nil if none)
    -- info.focused: whether it has focus
    if id == "count" then
      local n = tonumber(info.text:match("-?%d+") or 0)
      if n < 0 then return { fg = "red", bold = true } end
      if n > 100 then return { fg = "green" } end
    end
    if info.role == "button" and info.text:find("^Reset") then
      return { hidden = true }
    end
  end,

  -- Called for each key. Return another key, { focus = "<id>" }, or nil.
  -- chord is written as in [keys]: "j", "ctrl+n", "shift+tab".
  key = function(chord, focused)
    if chord == "j" and focused ~= "log" then return "down" end
    if chord == "k" and focused ~= "log" then return "up" end
    if chord == "/" then return { focus = "search" } end
  end,
}
```

- With both files, both apply: the TOML file first, then Luau over it. The
  fields `widget` returns replace the same TOML fields and leave the others;
  `key` is asked first and `[keys]` answers when it returns nil.
- The rules of the TOML file hold: no colour while a widget has focus,
  nothing typed into an input reaches `key`, `hidden` and `text` do not apply
  to the root.
- `widget`'s answers are remembered by (id, role, text, focused): it is
  called again only when one of those changes, so write it as a function of
  its arguments.
- The script runs in the same sandbox as `ulttui-lua` scripts: Luau's
  `string`, `table`, `math`, `utf8`, `bit32`, `buffer`, `coroutine` and `os`
  (no `execute`), no `io`, `require` or `loadstring`, and it cannot start
  processes. `print` writes to the app's log.
- Each call has 2 ms and each frame 8 ms, and the script 4 MiB of memory. A
  call that errors, returns something that is not an override, or runs out
  of time is logged once, and the widget keeps the last answer that worked
  (the hook is not asked again for the same inputs until the file is
  reloaded); a failing `key` call falls through to `[keys]`. A loop that
  never ends costs a frame about 8 ms. One slow library call (a huge
  `string.rep`, a pathological pattern) can run past the limit by tens of
  milliseconds.
- Return plain tables with string field names: anything else is an error.
- A file that fails to load (a syntax error, a value other than a table of
  these two functions, more than 200 ms to run) is logged and the last good
  version stays, as for TOML.

Apps get Luau overrides when they are built with ulttui's `lua-overrides`
feature, which links a Luau VM:

```toml
ulttui = { version = "...", features = ["lua-overrides"] }
```

`ulttui-lua` and the examples in this repository have it on. An app built
without it logs that it is ignoring the `.luau` file and still applies its
`.toml` file.

## What it cannot do (yet)

- Change layout (sizes, order, gaps) or a widget's borders on their own.
- Target widgets without an id (a Luau `widget` hook sees only those too).
- Inline cards (`ulttui::inline`) do not read overrides.
