# Configuration

Ultty reads `ultty/config.toml` in the platform config directory
(`~/.config` on Linux, `~/Library/Application Support` on macOS) at start and
reloads it when the file changes. Every key is optional; missing
keys keep their defaults. The full set of keys is defined in
`crates/ultty/src/config.rs`.

This page covers what programs running in a pane are told and allowed to do,
and the settings that change how panes behave.

## Themes

Ultty bundles 94 colour themes: its original six (`everforest-dark`,
`tokyo-night`, `gruvbox-dark`, `catppuccin-mocha`, `dark`, `light`) and 88
more converted from the [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)
collection, among them Catppuccin, Dracula, Gruvbox, Nord, Solarized, Tokyo
Night, One, Rose Pine, Kanagawa, Everforest, GitHub and the Nightfox family.
List them with:

```sh
ultty --list-themes
```

On a terminal the list shows a swatch of each theme's background, text and 16
colours. Piped, it is one name per line.

```toml
[colors]
theme = "catppuccin-latte"
```

Names match without regard to case, and a space or underscore is the same as
a dash: `Rose Pine`, `rose_pine` and `rose-pine` are one theme. Colours you
set in `[colors]` (`foreground`, `background`, `cursor`, `selection`,
`selection_foreground`, and single keys of `[colors.palette]`) win over the theme's. The key combination
`ctrl+shift+n` (`keybindings.cycle_theme`) steps through the themes in name
order.

### Your own themes

Put a theme in `themes/NAME.toml` next to `config.toml`
(`~/.config/ultty/themes/` on Linux). It is found by its file name, and
replaces a bundled theme of the same name. Foreground, background, cursor and
all 16 palette colours are required; `name`, `selection` and
`selection_foreground` are optional. A
file that does not parse, or lacks a colour, is skipped with a warning. The
directory is read when the config is loaded, so a new theme file shows up
the next time `config.toml` changes or Ultty starts. `[colors.themes.NAME]`
tables in `config.toml` define themes the same way and win over files.

```toml
name = "My Theme"
foreground = "#c0caf5"
background = "#1a1b26"
cursor = "#c0caf5"
selection = "#33467cc0"   # optional; last two digits are opacity
selection_foreground = "#c0caf5"   # optional; text colour in a selection

[palette]
black = "#15161e"
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
magenta = "#bb9af7"
cyan = "#7dcfff"
white = "#a9b1d6"
bright_black = "#414868"
bright_red = "#f7768e"
bright_green = "#9ece6a"
bright_yellow = "#e0af68"
bright_blue = "#7aa2f7"
bright_magenta = "#bb9af7"
bright_cyan = "#7dcfff"
bright_white = "#c0caf5"
```

Ultty does not switch between a light and a dark theme with the system
appearance (Ghostty's `theme = "light:X,dark:Y"`): it does not read the
system setting. Choose one theme, or cycle with the keybinding.

### Selected text

`selection` is the selection's background. Its opacity is raised to at least
55% so a faint selection still shows. `selection_foreground` (`[colors]` key
and theme key, `#rrggbb`) is the text colour inside it, like Ghostty's
`selection-foreground`; the bundled themes carry the one their authors chose.

Without it, each selected character keeps its own colour while that colour has
a contrast of at least 4.5:1 against the selection (WCAG relative luminance),
so syntax colours survive. Otherwise it is drawn in the theme's foreground or
background colour, whichever contrasts more (black or white if neither
reaches 3:1). Ghostty, Alacritty and kitty swap
the cell's foreground and background instead; that makes text vanish when the
selection is close to the text colour, as it was on Nord.

The importer keeps every bundled theme legible: a selection that is within
1.3:1 of the background is moved towards the foreground until it shows, and an
upstream selection foreground under 3:1 on the selection is dropped. A test
holds all bundled themes to those numbers.

### Where the bundled themes come from

The theme files are in `crates/ultty/themes/` and are compiled into the
binary. `crates/ultty/themes/THIRD_PARTY_THEMES.md` lists each theme's licence
and source, and every file names its own. Only themes licensed for
redistribution (MIT, Apache-2.0 and similar) are included; themes marked
unknown, non-free or copyleft upstream are left out.

To add themes or refresh them, check out iTerm2-Color-Schemes, add names to
`scripts/themes/selection.txt`, and run:

```sh
scripts/import-themes.py --upstream /path/to/iTerm2-Color-Schemes
```

The script reads each named scheme from `ghostty/`, looks up its licence in
the checkout's `licenses.yml`, and rewrites `crates/ultty/themes/` and the
notice. It stops with an error for a scheme whose licence is not on its
allow list. A scheme that `licenses.yml` lists as unknown can be admitted with
`Name | SPDX | source-url` in the selection file once you have checked the
licence at the source. Schemes with no upstream file can go in
`scripts/themes/sources/`, in Ghostty format with `# license:` and
`# source:` header lines (Solarized does). The script's tests run with
`python3 -S -m unittest discover -s scripts/themes -p 'test_*.py'`.

## Terminal type

```toml
[terminal]
term = "xterm-256color"   # or "xterm-ultty"
```

`term` is the `TERM` that shells and commands in new tabs start with. A
split gets the same `TERM` as the rest of its tab, and open panes keep the
value they started with, so after a config reload the new value applies to
tabs opened from then on. `COLORTERM` is always `truecolor`.

## Shell integration

```toml
[terminal]
shell_integration = "auto"   # or "off"
```

With `auto` (the default), the bash, zsh and fish shells Ultty starts load the
shell integration by themselves: prompt, command and exit-status marks
(OSC 133) and working-directory reports (OSC 7). You do not source anything
from your rc files. With `off`, shells start as they are. The setting applies
to tabs opened after a config reload.

Setting `ULTTY_SHELL_INTEGRATION=off` (or `0`, `false`, `no`) in the
environment Ultty starts in turns it off too. In a shell that was injected,
`ULTTY_SHELL_INTEGRATION` is `auto` and `ULTTY_SHELL_INTEGRATION_DIR` names
the directory of integration files, so an rc file can test for it.

How each shell is started (the shell runs with no other arguments, except
that `-l` or `--login` is kept):

- **bash** runs with `--rcfile` pointing at a wrapper that sources your
  `~/.bashrc`, then the integration. This keeps bash's normal startup: no
  `--posix` mode to undo, and `/etc/bash.bashrc` still runs first. A login
  bash (`-l`, `--login`, or a leading `-` in `argv[0]`) ignores `--rcfile`, so
  it runs as `bash [-l] --posix` with `ENV` naming a script that puts your
  `ENV` back, turns posix mode off, runs `/etc/profile` and the first of
  `~/.bash_profile`, `~/.bash_login` and `~/.profile` (your `~/.bashrc` runs
  only if one of them sources it, as usual), then loads the integration. The
  shell is still a login shell, and no longer in posix mode, when you get the
  prompt. Ultty itself starts your shell as a plain interactive shell, not a
  login shell, on Linux and macOS alike; a login bash comes from a launch
  profile or `ultty -e bash -l`.
- **zsh** runs with `ZDOTDIR` pointing at a directory whose `.zshenv` puts
  your original `ZDOTDIR` back (unset stays unset), runs your own `.zshenv`,
  and loads the integration from the first prompt, after your `.zshrc`. With
  `-l`, `.zprofile` and `.zlogin` are read from your original `ZDOTDIR`, since
  it is restored before they are looked for.
- **fish** finds the script in `vendor_conf.d`, because Ultty's directory is
  first on `XDG_DATA_DIRS`; the script removes that entry again.
- A tab that runs a bare shell as its command (`ultty -e bash`, a launch
  profile or `ulttyctl` new-tab with `command = "zsh"`, or the same with
  `-l`/`--login`) is injected like a default shell, and the off switches apply.
- Any other shell is not touched, and neither is a shell given other
  arguments (`bash -c ...`) or a tab that runs any other command.

If you already source `ultty.bash`, `ultty.zsh` or `ultty.fish` yourself, keep
doing so: the scripts do nothing the second time, so marks are sent once.
Over ssh the remote shell is not injected; source the scripts there.

The files are built into the binary. Ultty writes them to
`$XDG_RUNTIME_DIR/ultty-shell-integration-<version>-<hash>` (mode 0700, files
0600), or `<temp dir>/ultty-<uid>/` when that is unset.

- `"xterm-256color"` (the default) is installed on almost every system, so it
  works everywhere, including over ssh. It describes a few things Ultty does
  not do (left and right margins, a printer port, setting the cursor colour),
  and lacks the capabilities for curly and coloured underlines and
  synchronized output.
- `"xterm-ultty"` describes what Ultty implements: everything in
  `xterm-256color` that Ultty does, plus truecolor (`Tc`), extended
  underline styles and colour (`Su`, `Smulx`, `Setulc`), synchronized output
  (`Sync`), and OSC 52 clipboard writes (`Ms`). The entry is
  `dist/terminfo/ultty.terminfo`. `install.sh`, `make install`, and the
  `.deb`, `.rpm`, and Homebrew packages install it. To install it by hand:

  ```sh
  tic -x -o "$HOME/.terminfo" dist/terminfo/ultty.terminfo
  ```

  Every host a program runs on needs the entry too. Over ssh, a host without
  it gives errors such as `'xterm-ultty': unknown terminal type`. Copy the
  entry once per host:

  ```sh
  infocmp -x xterm-ultty | ssh host tic -x -
  ```

A value that is not a terminfo name (1 to 64 letters, digits, `.`, `_`, `+`
or `-`) is ignored with a warning, and `xterm-256color` is used. Programs that
ask the terminal for its name (XTGETTCAP `TN`) get the `TERM` their pane
started with.

## Scrollback

```toml
[behavior]
scrollback_lines = 10000
```

How many lines each pane keeps after they scroll off the top, up to
1,000,000 (larger values are clamped, with a warning). A config reload applies
the new value to open panes as well; lowering it drops their oldest lines. The
alternate screen, which full-screen programs use, has no scrollback.

## Images

```toml
[images]
storage_limit = 256
```

How much decoded image memory, in MiB, each screen of a pane may hold for
sixel and Kitty graphics images together. Each pane has two screens (main and
alternate), so a pane can hold up to twice this. Past the limit the oldest
images are evicted, those no longer on screen first. `0` turns images off:
both protocols are ignored, and Kitty graphics queries get no reply, so
programs fall back to text. A config reload applies the new value to open
panes as well.

## When a program exits

```toml
[behavior]
close_on_exit = true
```

With `true` (the default), a pane closes when its program exits, a tab closes
with its last pane, and the window closes with its last tab. With `false`, a
pane whose program exited stays open with its last output and a
`[process exited with status N]` line, until you close it (close pane, close
tab, or the window's close button). This is useful for launch-profile tabs
that run a command rather than a shell.

## Multiple windows

```toml
[behavior]
window_per_process = false
single_instance = false
```

By default (`window_per_process = false`), "New window" opens another window
inside the running Ultty process, sharing the GPU device and avoiding a second
driver startup. Set `window_per_process = true` to spawn a separate OS process
per window for process-level crash isolation (at the cost of a separate GPU
context per window). A config reload applies this to the next "New window"
action.

With `single_instance = true` (off by default), launching `ultty` (with or
without `-e COMMAND`) connects to the newest running Ultty instance's control
socket, opens a window there in the caller's working directory, and exits once
the window opens. If no instance answers within a short timeout, `ultty` starts
normally. Pass `ultty --new-process` to force a new process even when
`single_instance = true` (`--profile` and `--workspace` always start a new
process).

## Quick terminal

```toml
[quick_terminal]
position = "top"     # top, bottom, left, right, center
size = "40%"         # percentage (10%-100%) or pixels (e.g. "420px")
autohide = true      # hide when the quick terminal loses focus
# hotkey = "ctrl+grave"  # built-in global hotkey on X11 and Wayland GlobalShortcuts portal
```

Toggle the quick terminal from a desktop or compositor shortcut bound to
`ultty --quick` (or via `ulttyctl toggle-quick` / `show-quick` / `hide-quick`).
See [docs/quick-terminal.md](quick-terminal.md) for sway, Hyprland, niri, KDE,
GNOME, and X11 setup snippets.

## Restoring the last session

```toml
[behavior]
restore_session = false
```

With `true`, Ultty saves its tabs when the window closes (or when it gets a
termination signal) and reopens them at the next start: each tab's title,
its split layout and which pane had focus, and each pane's working
directory, taken from the directory its shell last reported with OSC 7. The
active tab is selected again. The file is `last.json` in the `sessions`
directory next to `config.toml`.

A restored pane is a fresh shell. Scrollback and screen contents are not
restored, and programs that were running are not started again, including the
commands of launch-profile tabs. A saved directory that no longer exists falls
back to the directory Ultty was started in. A pane whose shell never sent OSC 7
starts in its tab's saved directory (the first pane's), else in the directory
Ultty was started in.
If the file is missing or unreadable, Ultty starts as it would with the
option off. A saved session takes precedence over a default launch profile.
The default is `false`.

## Where new tabs and splits open

```toml
[behavior]
inherit_working_directory = true
```

With `true` (the default), a new tab or split starts in the directory of the
pane it was made from: the focused pane for a new tab, the pane being split for
a split. Ultty reads that directory from the shell's last OSC 7 report
(`file://host/path`), which the shell integration scripts in
`dist/shell-integration` (loaded for you, see above) send on every prompt. The report counts only when
its host is empty, `localhost` or this machine's hostname (so a pane that is
ssh'd elsewhere does not send the new shell to a path that exists only on the
remote host) and the directory still exists. The path is percent-decoded.

If the pane sent no usable OSC 7, Ultty uses the working directory of the
pane's foreground process, read from `/proc` (Linux only). If that fails too,
or with `false`, the new pane starts in the directory Ultty was started in.
A config reload applies the setting to the next tab or split.

A pane that runs a native-protocol app is an ordinary pane: its shell started
the app, so a new tab or split opens where that shell last reported, not in
the app's own directory. Tabs and splits made from a launch profile or by
`ulttyctl` with an explicit directory use that directory instead.

## Command palette

```toml
[keybindings]
command_palette = "ctrl+shift+p"
```

The command palette lists every action you can bind to a key, with the key
bound to it at the right, and runs the one you pick. It is the same table the
keyboard shortcuts overlay (`show_shortcuts`) shows, so a key you rebind here
shows up in both.

- Type to filter. Matching is fuzzy and ignores case and spaces: `sd` finds
  "Split down", `nt` finds "New tab". Matched letters are colored and
  underlined, and the best match is selected.
- Up and Down, or Ctrl+P and Ctrl+N, move the selection; Enter runs it; Esc,
  a click or the palette's own key closes the palette.
- With an empty query, the actions you ran from the palette come first, most
  recent at the top. Ultty forgets them when it exits.
- While the palette is open it takes every key, so nothing you type reaches
  the program in the pane, whether that is a shell or a native-protocol app.
  Closing it leaves the same pane focused.
- Splits are offered as "Split right" (`split_horizontal`) and "Split down"
  (`split_vertical`). The palette does not list itself.
- Screen readers see the palette as a dialog with a search field and a list
  of commands; the selected command has focus and carries its key as the
  keyboard shortcut.

Set `command_palette = ""` to turn the key off.

## Jumping between prompts

```toml
[keybindings]
jump_prev_prompt = "ctrl+shift+pageup"
jump_next_prompt = "ctrl+shift+pagedown"
select_last_output = "ctrl+shift+o"
```

With shell integration (the scripts in `dist/shell-integration`, loaded automatically,
or any shell that sends OSC 133), these move through the shell prompts in
scrollback:

- `jump_prev_prompt` scrolls so the nearest prompt above the top of the view
  is the top row. Press it again for the one before.
- `jump_next_prompt` does the same going down. After the last prompt it
  returns to the bottom, the live screen.
- `select_last_output` selects, line by line, the output of the last command,
  from where it started to where it finished (or to the cursor, if it is still
  running), and scrolls to it if needed. Copy it with the copy key. A
  selection in Ultty is made of view rows, so this works only when the output
  fits on screen; for longer output nothing is selected, and
  `ulttyctl last-command` reads it whole.

A prompt is found by its OSC 133;B mark, so a jump lands on the row where you
type the command, which for a prompt that spans several lines is the last one.
Prompts that have scrolled out of the scrollback limit are gone, and a pane
without shell integration has none to jump to. The keys do nothing on the
alternate screen (full-screen programs such as editors), where they go to the
program.

Ghostty binds its `jump_to_prompt` to Ctrl+Shift+Up and Down. Ultty already
uses those to move between panes (`focus_pane_up`, `focus_pane_down`), so the
defaults here are Ctrl+Shift+PageUp and PageDown, beside Shift+PageUp and
PageDown for scrolling by a page. Set any of the three to `""` to turn it off.

## Searching scrollback

```toml
[search]
regex = false
smart_case = true

[keybindings]
find = "ctrl+shift+f"
```

`find` enters copy mode with a query prompt. You can also press `/` (search
forward) or `?` (search backward) in copy mode. Type the query and press Enter;
`n` and `N` move to the next and previous match, and the first jump goes to the
match nearest the cursor in the search direction. Esc leaves the prompt.

- Matching reads a soft-wrapped line as one line, so a match can run across
  the wrap. A hard line break (a newline from the program) still ends it.
  Trailing blanks at the end of a line are not part of it.
- `smart_case = true` ignores case unless the query has an uppercase letter.
  With `false`, matching is always case sensitive.
- `regex = true` reads the query as a regular expression in the Rust `regex`
  crate's syntax. Ctrl+R in the prompt toggles it for that search; the prompt
  shows `[regex]` while it is on. A pattern that does not compile shows its
  error in the status line; a pattern that
  can match nothing (`a*`) skips the empty matches.
- All matches are highlighted, the current one brighter, and the status line
  shows `[3/17]` or `[no matches]`.
- The scan runs in slices of about 2 ms per frame, so the terminal keeps up
  with its program while a large scrollback is searched; the status line shows
  `searching 40%`. Measured in a release build on 120-column lines: 10k lines
  in 8 ms, 100k in 140 ms, 1M in 0.6 to 1.4 s, split over many frames. Matches are a snapshot: output that arrives after the scan
  passes a row is not searched, and matches in lines the scrollback limit
  evicts later are dropped.
- A logical line over 1 MiB is searched in pieces, so a match cannot cross
  those cuts.

## Font size

```toml
[font]
size = 11.0
```

Font size in typographic points (1/72 inch), rendered at 96 DPI multiplied by
the display scale factor (so `1 pt = 4/3` logical pixels, matching Alacritty
and Ghostty). Values outside 4.0 to 128.0 are clamped.

## Line height

```toml
[font]
line_height = 1.0
```

A multiplier on the font's natural line height (`ascent - descent + line gap`
at the configured point size), rounded up to whole pixels. Values outside 0.5
to 3.0 are clamped. At 1.0, block and box-drawing characters in adjacent rows
touch; larger values add space between lines. Font settings apply when the
config is reloaded; see
[Changing fonts without a restart](#changing-fonts-without-a-restart).

### Restore the previous cell dimensions

Add `metrics = "legacy"` to the existing `[font]` section to use the
pixel-based sizing from before PR #208.

```toml
[font]
metrics = "legacy"
size = 14.0
line_height = 1.0
```

Keep your saved `size` and `line_height` values. Ultty scales the size with
the display and computes the cell height as `size * line_height` after
scaling, rounded up to whole pixels. Legacy mode defaults to 14 logical
pixels when `size` is omitted. Use `metrics = "points"` for typographic
points and natural line height. Points is the default.

## Bold and italic families

```toml
[font]
family = "Fira Code"
bold_family = "Fira Code SemiBold"
italic_family = "Victor Mono"
bold_italic_family = "Victor Mono Bold"
```

`bold_family`, `italic_family` and `bold_italic_family` name the font for
bold, italic and bold italic text. Each is empty by default. An empty entry
uses `family`: its own bold or italic face if it has one, and otherwise the
regular face with a slant for italic and, for bold, a color brighter on a
dark background or darker on a light one.
`bold_italic_family` does not fall back to `bold_family` or `italic_family`.
A style family is asked for the bold weight and italic slant too, so a family
with several weights gives its bold one. Cells are sized from `family`, so a
style family with a different advance width breaks the grid; use families
meant to go together.

A family that is not installed (the default, DroidSansMono Nerd Font, on a
fresh machine) is replaced by the system monospace font, and Ultty logs one
warning naming it. The grid is never drawn in a proportional face. The same
goes for the style families and `codepoint_map` families.

## OpenType features

```toml
[font]
features = ["calt", "-liga", "ss01", "zero", "cv02=2"]
```

`features` is a list of OpenType features to apply when shaping. `"calt"` or
`"+calt"` turns a feature on, `"-liga"` turns it off, and `"cv02=2"` picks the
second alternate of a feature that has several. Entries apply in order and a
later entry for the same feature replaces an earlier one. An entry that is not
four characters of tag, with an optional sign or value, is ignored with a
warning in the log. The list applies to regular, bold and italic text alike.

`ligatures` is a separate switch over whether neighbouring cells are shaped
together at all. It defaults to `true`. With `ligatures = false` no cells are
shaped together, so `calt` and `liga` cannot join them whatever `features`
says; the other features, such as `zero` or `ss01`, still apply to each cell.
With `ligatures = true`, a font's default features apply first and `features`
adjusts them. Which feature forms a given ligature depends on the font: many
programming fonts, Fira Code among them, build theirs with `calt`, so
`["-calt"]` is what turns those off, while `["-liga"]` turns off standard
ligatures such as `fi`.

## Changing fonts without a restart

`family`, `bold_family`, `italic_family`, `bold_italic_family`, `features`,
`ligatures`, `size`, `line_height` and `emoji` all apply when the config is
reloaded (Ctrl+Shift+R, or when the config file changes). Ultty measures the
cell again, empties its glyph cache and resizes every pane to the new cell
size. A `size` you did not change in the file keeps any zoom you set with the
font size keys; changing it resets the zoom to the new value.

## Codepoint map

```toml
[[font.codepoint_map]]
range = "U+E0A0-U+E0A3"
family = "Symbols Nerd Font Mono"

[[font.codepoint_map]]
range = "U+4E00-U+9FFF"
family = "Noto Sans CJK JP"
```

Each `[[font.codepoint_map]]` table draws a range of characters from a family
of its own, whichever fonts would otherwise supply them. `range` is `U+XXXX`
or `U+XXXX-U+YYYY` in hex, inclusive. Where ranges overlap, the later table
wins. A mapped family is asked for bold and italic as usual, and gets
`features`. A mapping wins over the emoji font, and a mapped ASCII character
is shaped alone, so it does not join a ligature. An entry with a range that
does not parse or an empty family is ignored with a warning in the log. Block
and box-drawing characters and the Powerline triangles are drawn by Ultty
itself and stay that way when mapped.

## Emoji and accents

```toml
[font]
emoji = "Noto Color Emoji"
```

Characters the configured font lacks come from other installed fonts. Color
emoji need a color emoji font such as Noto Color Emoji
(`fonts-noto-color-emoji` on Debian and Ubuntu). Characters that display as
emoji by default (`😀`, `🎉`, flags, skin tones), and any character followed
by the emoji variation selector U+FE0F (`❤️`, `☺️`, `1️⃣`), are drawn from
that font, in color. Characters that display as text by default (`❤`, `☺`,
`™`), and emoji followed by the text variation selector U+FE0E (`⌚︎`), keep
the text font, or whichever installed font has them. Bold and italic do not
change an emoji. An emoji still takes two cells.

`emoji` names the color emoji font. When it is empty (the default) or not
installed, Ultty uses the first installed of Noto Color Emoji, Apple Color
Emoji, Segoe UI Emoji, Twemoji, Twitter Color Emoji, JoyPixels and OpenMoji
Color. With none of them installed, emoji come from whatever installed font
has them, often in black and white.

An emoji sequence (a ZWJ family, a flag, a skin tone) is drawn as one glyph
in whatever design the emoji font gives it. Noto Color Emoji 2.051, for example, draws the family
sequences as grey silhouettes on a light tile. That is the font's design,
not a missing-glyph box.

When the font has no positioning rule for a second accent on an accented
letter (as in `é̃`), Ultty stacks the accent above the first one itself.

## Clipboard

```toml
[behavior]
clipboard_write = "allow"   # or "deny"
```

Programs set the clipboard with OSC 52. Any output that reaches the focused
pane can do this, including a `cat`ed file or a remote host over ssh. With
`"deny"`, Ultty ignores every OSC 52 write. OSC 52 clipboard reads are always
refused, whatever this setting says.

## Paste

When the program has turned on bracketed paste (most shells and editors do),
Ultty removes every ESC and C1 control character (U+0080 to U+009F) from the
pasted text before wrapping it in the bracket markers. Pasted text can never
end the bracket early and have the rest run as typed input.

Without bracketed paste, newlines are sent as carriage returns, the byte the
Enter key sends. Ultty does not ask before a multi-line paste, so in a program
without bracketed paste each pasted line runs as it arrives.

## Links

Ctrl+click opens a link only when its scheme is `http`, `https`, `mailto`, or
`ftp`. Anything else is refused and logged, including `file:` links to paths
on this machine. Link targets come from terminal output, and OSC 8 lets a
program show one text and link to another, so a `file:` link to a `.desktop`
file or a link to another registered scheme handler would let a `cat`ed file
or a remote host start local programs. This list is not configurable.

## Bell

```toml
[behavior]
bell_visual = true   # flash the border when the focused pane rings
bell_urgent = true   # mark an unfocused window as wanting attention
```

Ultty has no audible bell. The old `bell_sound` key is ignored.

## Cursor

```toml
[behavior]
cursor_blink = true   # false keeps the cursor steady
```

The cursor blinks only while the window has focus and the program has not
asked for a steady cursor (`DECSCUSR` 2, 4 or 6). With `cursor_blink = false`
it never blinks, and an idle window does not wake up at all.

## Accessibility

```toml
[behavior]
accessibility = true   # false keeps Ultty off the accessibility bus
```

With `true` (the default), Ultty exposes itself to screen readers through
AccessKit: on Linux it watches the session D-Bus and joins the AT-SPI bus only
while assistive technology is running, and an idle window does not wake up.

What a screen reader sees:

- The window, with a tab list (one tab per tab, the active one selected) and
  a panel for the active tab.
- The focused pane as a `Terminal` node holding one text run per screen row,
  with the caret at the cursor. This is the live screen only; scrollback is
  not exposed yet.
- The other panes of the tab as groups with just their titles.
- For a pane running a native-protocol app, the app's regions in place of the
  grid: each region as a node with its role, name and state, and focus on the
  app's focused region. A region that announces changes (a live region) is
  read out when it changes.
- An inline surface in a pane's rows, as a node described by its anchor row.

With `false`, Ultty never connects to the accessibility bus and creates its
window visible at once. Read at start; restart Ultty after changing it.
See [accessibility-test.md](accessibility-test.md) for the manual test.

## Vulkan driver narrowing (Linux)

```toml
[behavior]
narrow_vulkan_icd = true   # false lets the Vulkan loader enumerate every installed ICD
```

On Linux, when `mesa-vulkan-drivers` is installed alongside a hardware GPU, the
Vulkan loader normally opens every ICD manifest during adapter enumeration and
keeps Mesa's software rasterizer (`libLLVM.so`, ~40+ MiB) mapped even when the
hardware GPU is selected.

With `true` (the default), Ultty inspects accessible render nodes under
`/sys/class/drm/renderD*` (and `/dev/nvidiactl`) at startup—before creating any
threads—and sets `VK_DRIVER_FILES` to the matching hardware ICD manifests.
The setting is a no-op if a Vulkan loader environment variable
(`VK_DRIVER_FILES`, `VK_ICD_FILENAMES`, `VK_ADD_DRIVER_FILES`,
`VK_LOADER_DRIVERS_SELECT`, `VK_LOADER_DRIVERS_DISABLE`) is already set, and
falls back to all drivers when no accessible hardware GPU, recognized vendor,
or matching ICD manifest is present. Ultty strips its internal `VK_DRIVER_FILES`
override when spawning child shells. Read at start; restart Ultty after
changing it.

## CPU first frames

```toml
[behavior]
cpu_first_frames = true    # default on Linux (false on macOS/Windows)
```

With `true`, Ultty renders the focused pane's grid and the tab bar/window
chrome on the CPU (via `softbuffer`) from the moment the window opens while
the GPU renderer starts on a background thread, then hands over to the GPU
once it is ready. This brings the first shell prompt onto the screen much
sooner on drivers with slow GPU initialization.

The default is `true` on Linux (X11 and Wayland) and `false` on other
platforms until hardware handover is verified there. Read at start; restart
Ultty after changing it. For a single launch, `ULTTY_CPU_FIRST_FRAMES=1`
forces it on and `ULTTY_CPU_FIRST_FRAMES=0` forces it off regardless of
`config.toml`.

## Log file

```toml
[log]
file = true                  # default; false logs to stderr only
# path = "/var/tmp/ultty.log"  # absolute path instead of the state directory
# level = "ultty=debug"        # RUST_LOG syntax; default "ultty=info,wgpu=warn"
```

Ultty writes its log to stderr and to `$XDG_STATE_HOME/ultty/ultty.log`
(`~/.local/state/ultty/ultty.log` when `XDG_STATE_HOME` is unset or not
absolute). It logs the file's location at startup. The log may hold paths
and window titles, so Ultty creates the directory with mode 0700 and the
file with mode 0600. `ultty-session` writes to the same file.

The file is rotated at 10 MiB, keeping three older files (`ultty.log.1`,
the newest, to `ultty.log.3`). Several Ultty processes can share the file:
each line starts with `pid=<pid>`, lines are appended whole, and when two
processes reach the cap together only one rotates it. Lines are written on a
background thread so a slow disk never stalls a window; if that thread falls
behind, new lines are dropped and a `dropped N log lines` line records how
many.

`level` applies to stderr and the file alike. Environment variables win over
`[log]`:

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | the filter, replacing `level` |
| `ULTTY_LOG_FILE=0` | no log file (tests, benchmarks) |
| `ULTTY_LOG_FILE=1` | the default file even when `file = false` |
| `ULTTY_LOG_FILE=<path>` | log to `<path>` instead |

The control plane's audit trail (target `ultty::audit`) goes to the same
file; see [CONTROL_PROTOCOL.md](CONTROL_PROTOCOL.md#audit-log). `[log]` is
read at start; restart Ultty after changing it.
