# ADR 0023: Per-App User Overrides for ulttui Apps

## Status

Proposed 2026-11-10. Amended 2026-11-11 to add Luau override files
(§8), which v1 had left out.

## Context

ulttui's direction is malleable software for the terminal: a user should be
able to reshape an app they use without its source. `ulttui-lua` covers
writing a new app. It does not help with an app someone else wrote in Rust.

Every ulttui app already names the widgets that matter. `View::widget_id` is
the focus key and the region's `id` metadata (`ulttyctl regions` shows it), and
apps keep ids stable across frames because focus depends on it. That gives a
user-facing handle on any widget without the app's cooperation.

What a user most often wants to change in someone else's TUI:

- a colour they cannot read, or something that should stand out;
- a panel they never look at;
- a label (language, jargon, length);
- a key binding (vim keys, a key their terminal eats, a jump to a field).

## Decision

### 1. One file per app, TOML

The file is `$XDG_CONFIG_HOME/ulttui/overrides/<app>.toml`, or
`~/.config/ulttui/overrides/<app>.toml` when `XDG_CONFIG_HOME` is unset.

```toml
[widget.status]
fg = "yellow"
bold = true

[widget.sidebar]
hidden = true

[widget.submit]
text = "Send"

[keys]
"j" = "down"
"ctrl+n" = "down"
"/" = { focus = "search" }
```

TOML, not Luau, for v1. Everything above is static data, and TOML says it in
a form a user can write without knowing a language. Luau would make every
ulttui app link `mlua` and a Luau VM (a C++ build) to read four kinds of
setting, and would bring the questions a script raises (time limits, what
it may call, what happens when it fails mid-frame) without a use for them yet.
Overrides that compute (a colour from the widget's text, a key that runs
something) are the case for Luau; when that comes it can be a second file
type that `ulttui-lua`'s sandbox (no `io`, no `require`, no exec without
`--allow-exec`, the one-second call limit) runs, keyed the same way. `toml`
is already in the lockfile (Ultty's config uses it), so this adds no new
crate to the build.

### 2. The app name

The app name is the base name of `argv[0]`, the name the user typed. An app
can set its own with `App::app_name` (`ulttui-lua` uses the script's file
stem, so each script has its own file rather than all sharing
`ulttui-lua.toml`).

Two environment variables:

- `ULTTUI_OVERRIDES=<path>` reads that file instead. It is a path rather than
  a name so a user can try a file anywhere, or point several apps at one.
- `ULTTUI_NO_OVERRIDES` set to anything but empty or `0` turns overrides off,
  whatever the file says.

### 3. What can be overridden

Per widget id:

| Key | Effect |
|-----|--------|
| `fg`, `bg` | Foreground and background colour of the widget's box, over what the app set. Same colour syntax as `ulttui-lua`. Not applied while the widget has focus, so its focus colours stay visible. |
| `bold` | Bold on or off. |
| `hidden` | The widget and everything in it are not drawn, take no room in a column or row, and are not focusable. |
| `text` | Replaces the text of a text-bearing widget: `Text`, `RichText` (one span in the first span's style), `Title`, `Body`, `Muted`, `Accent`, and a `Button`'s label (also its accessible name). On other widgets it does nothing. |

Per key, in `[keys]`: a key (`ctrl+n`, `alt+x`, `shift+tab`, `j`, `enter`;
the names are `ulttui-lua`'s) maps to another key, or to `{ focus = "<id>" }`,
which moves focus to that widget and swallows the key. Remapping is one
lookup: a key a remap produces is not remapped again. While a widget that
takes text (`View::accepts_text`, set by `TextInput`) has focus, characters
without ctrl or alt are not remapped, so they can be typed. A key held with
meta matches no chord.

### 4. Where it is applied

Once, in the walk, so every widget gets it without knowing about overrides:

- Native: `render_view_to_native`, which every container already uses for its
  children. A hidden widget is sent as a region with `display: none`, no
  content or children, and semantics `hidden`; style is merged into the region's style and into the
  spans of rich-text content, so the override wins over colours the widget
  chose.
- ANSI: a new `render_view_ansi` that the backend uses for the root and every
  container for each child (they called `child.render_ansi` directly before).
  It skips a hidden widget and, after the widget draws, sets the overridden
  colours and bold over its area. `Column` and `Row` leave hidden children out
  of their layout, as Taffy does natively. Other containers (`Stack`,
  `DockLayout`, `Modal`) leave a blank where the widget was.
- Text: the walk scopes the widget's replacement text while it renders that
  widget (and its own `content`/`semantics`), and clears it for each child, so
  text set on a container does not leak into its children. The text-bearing
  leaves read it through one helper.
- The app's root view: only its style is overridden, the same way in both
  modes and beneath the toast overlay (the native root region's style; in
  ANSI, painted after the root draws and before the toasts). `hidden` and
  `text` on the root are ignored and logged once.
- Focus: `FocusManager::register_view_tree` skips hidden subtrees.
- Keys: `App::run_tick`, before the screen stack, the app and Tab navigation
  see the event.

Inline cards (`ulttui::inline`, ADR 0020) are not run through `App` and do
not get overrides in v1.

### 5. Reload

The runner checks the file's modification time and length before each frame
it draws and reparses it when either changed, so an edit shows on the next
frame (the next key, timer or message). An idle app does not wake to look;
`notify` would add a thread and a dependency to every app for little gain
over "press a key".

### 6. Errors

A bad override file never stops or crashes the app. A file that does not
parse, a key that is not a key, an unknown setting or a colour that is not a
colour is logged once with `tracing::warn!` (path and reason) and the whole
file is ignored. A missing file is not an error. Only a regular file is opened (a FIFO
would block the app, a device such as `/dev/zero` never ends), and one over
1 MiB is ignored the same way. On a reload that fails, the
last good overrides stay in force, as `ulttui-lua` does with a bad save.
Ignoring the whole file rather than the bad entry keeps the rule simple and
makes a typo visible in the log instead of half applying.

### 7. Sandboxing

TOML is data: there is nothing to run, read or execute. Luau override files
run in the sandbox `ulttui-lua` scripts run in (§8), with no way to start a
process.

### 8. Amendment: Luau override files

Some changes are computed rather than written out: colour a count red when
it is negative, hide every button whose label starts with "Delete", map `j`
to Down except in one list. A Luau file does these.

**File.** `<app>.luau` next to `<app>.toml` (`ULTTUI_OVERRIDES=<path>` reads
a Luau file when the path ends in `.luau`). Either, both or neither may
exist. When both do, both apply: TOML first, then Luau over it. For a
widget, the fields the Luau hook returns replace the same TOML fields and
the rest stay; for a key, the Luau hook answers first and `[keys]` answers
when it returns nil. Picking only one would make adding a small script mean
porting the whole TOML file.

**API.** The file returns a table with up to two functions:

```lua
return {
  widget = function(id, info)   -- info = { role, text, focused }
    if id == "count" and tonumber(info.text:match("-?%d+") or 0) < 0 then
      return { fg = "red", bold = true }  -- the [widget.<id>] fields, or nil
    end
  end,
  key = function(chord, focused)  -- chord as [keys] writes it: "ctrl+n", "j"
    if chord == "j" and focused ~= "search" then return "down" end
    if chord == "/" then return { focus = "search" } end
  end,
}
```

`info.role` is the widget's accessibility role (`button`, `input`,
`heading`, ..., or `auto` when the widget does not say), `info.text` its text (a text widget's text, a button's label, `nil`
for others) as the app wrote it, `info.focused` whether it has focus. Any
other field in the returned table, or a value of the wrong type, is an error.
The rules for TOML hold: no colour override while the widget has focus,
characters typed into an input are not passed to `key`, keys held with meta
match nothing, `hidden` and `text` do not apply to the root.

**Cost.** `widget` is called only if the file defines it, and only for
widgets with an id. Its results, and its failures, are cached by (id, role,
a 64-bit hash and the length of the text, focused), so a widget is asked
again only when one of those changes, an entry does not grow with the text,
and a hook that always fails costs one call per input rather than the frame
budget every frame. Two texts of one length with the same hash would share
an answer; that is accepted. The hook must be a function of its arguments
(it can keep state, but cached results will not see it). The cache holds
4096 entries; when full it is emptied, together with the last good results
by id, so ids an app generates and drops do not pile up. Within a frame
each id is resolved once however often the walks ask, which reads the
widget's content and role once more per id'd widget per frame.
With 200 buttons (`two_hundred_widgets_call_luau_once_each_then_not_at_all`,
release build, median of 101 frames of the native walk, 2026-11-11): no
overrides 65 µs, TOML 67 µs, a Luau file with only a `key` hook 64 µs (no
widget is read for it), a warm `widget` hook (no Luau calls, all cache
hits) 113 µs, the first frame (200 calls) 1.1 ms. Re-reading inputs costs
about 0.23 µs a widget, under 0.3% of a 60 Hz frame for 200 widgets, so it
is kept rather than cached by something cheaper but less exact.

**Bounds.** Each call gets 2 ms of Luau time and the frame 8 ms: a call that
runs over is stopped by the VM's interrupt (at loops and calls), and once a
frame has spent its 8 ms the remaining widgets keep their last results until
the next frame, so a loop that never ends costs a frame at most about 8 ms
plus one call. The interrupt cannot stop a single library call, which can
run past the limit by tens of milliseconds: measured with the earlier 16 MiB
cap, `string.rep('x', 15e6)` took 33 ms, sorting 200,000 numbers 4.9 ms, a
backtracking `string.find` 9 ms. The memory cap bounds the worst of these,
so override scripts get 4 MiB (`ulttui-lua` apps keep 256 MiB), and the
`string.rep` case now fails at the cap. Loading the file gets 200 ms. Values
a hook returns are read with raw accessors only, outside the time limit:
field names must be strings, and nothing is converted with `tostring`, so a
`__tostring` or other metamethod cannot run unbounded. A call that fails
(an error, the time limit, a bad return value) is logged once per loaded
version, and the widget keeps the last result that worked (nothing, if none
did); a failing `key` call falls through to `[keys]`.

**Sandbox.** The VM is `ulttui::lua_sandbox::Sandbox`, which is what
`ulttui-lua` used before and now shares: Luau's safe libraries only
(`string`, `table`, `math`, `utf8`, `bit32`, `buffer`, `coroutine`, and `os`
without `execute`), no `io`, `require`, `loadstring`, `getfenv`, `setfenv` or
`debug`, globals frozen, a memory cap and the time limit. `print` goes to
the log. There is no `run{}`: override scripts cannot start processes.

**Reload and errors.** As for TOML (§5, §6): checked before each frame, a
file that fails to load (syntax, a time-out while loading, a wrong return)
is logged and the last good version stays, a removed file stops applying.

**Feature.** Linking Luau means mlua and the Luau VM built from C++ in every
binary. That is behind a cargo feature, `lua-overrides`, off by default for
`ulttui`; `ulttui-lua` and the workspace examples turn it on. An app built
without it logs that it is ignoring a `.luau` file and still applies the
TOML one. The release build of `counter-app` (LTO,
`codegen-units = 1`) grows from 1,915,936 to 3,965,664 bytes (+2.05 MB,
about twice the size) with it on, measured 2026-11-11.
`porygon-tui`, which does not turn the feature on, does not link mlua.

## Consequences

- Widget ids become a user-facing interface. Renaming one in an app breaks
  users' overrides of it, as renaming a CSS class does. Apps should treat
  ids as stable; `ulttyctl regions` lists them in native mode.
- Widgets without ids cannot be targeted. In `ulttui-lua` every node takes
  an `id`; in Rust, plain `Text` and the layout containers have none (open
  question 4).
- The ANSI style override paints the widget's whole box, borders included.
  The native one sets the region's style, which the terminal also applies to
  the whole region. They agree.
- Each widget costs a thread-local read per frame when no overrides are
  loaded, and a hash lookup by id when some are.

## Alternatives considered

- **Luau overrides (v1).** Left out of v1 (§1), added behind a feature in
  §8.
- **Luau overrides through a hook that `ulttui-lua` fills in**, so `ulttui`
  never names mlua. Every app would need code (and a dependency on
  `ulttui-lua`) to get them; a feature gives them to an app with one line in
  its manifest.
- **Overrides as a wrapper `View` built per widget.** Containers own their
  children as concrete or boxed views and call them directly, so a wrapper
  cannot reach them without each container's help. Hooking the two walks
  reaches every widget.
- **Patching only the native protocol stream.** Would skip ANSI mode, which
  ADR 0009 commits to.
- **Applying the good entries of a bad file.** Harder to explain and to
  notice; see §6.

## Open questions

1. Should `ULTTUI_OVERRIDES` name an app rather than a file? This ADR takes
   a path as the more general of the two.
2. Overrides keyed by something other than id (widget type, role) would reach
   widgets without ids, at the cost of the precision ids give.
3. Should `ulttyctl` be able to push an override into a running native app?
4. A generic way for a Rust app to give any view an id (an `Id` wrapper, or
   `.id()` on `Column`, `Row` and `Text`), so more of an app is reachable.
