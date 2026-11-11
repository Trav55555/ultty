# ADR 0023: Per-App User Overrides for ulttui Apps

## Status

Proposed 2026-11-10.

The number may need changing at merge: another ADR (Luau in Ultty) was being
written at the same time and may also have taken 0023.

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

TOML is data: there is nothing to run, read or execute. If Luau overrides
come later they run under `ulttui-lua`'s sandbox with exec off.

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

- **Luau overrides (v1).** Rejected for now; see §1.
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
