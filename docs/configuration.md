# Configuration

Ultty reads `ultty/config.toml` in the platform config directory
(`~/.config` on Linux, `~/Library/Application Support` on macOS) at start and
reloads it when the file changes. Every key is optional; missing
keys keep their defaults. The full set of keys is defined in
`crates/ultty/src/config.rs`.

This page covers what programs running in a pane are told and allowed to do,
and the settings that change how panes behave.

## Terminal type

```toml
[terminal]
term = "xterm-256color"   # or "xterm-ultty"
```

`term` is the `TERM` that shells and commands in new tabs start with. A
split gets the same `TERM` as the rest of its tab, and open panes keep the
value they started with, so after a config reload the new value applies to
tabs opened from then on. `COLORTERM` is always `truecolor`.

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

## Line height

```toml
[font]
line_height = 1.0
```

The cell height as a multiple of the font size, rounded up to whole pixels.
Values outside 0.5 to 3.0 are clamped. At 1.0, block and box-drawing
characters in adjacent rows touch; larger values add space between lines.
Font settings are read at start; restart Ultty after changing them.

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
