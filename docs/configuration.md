# Configuration

Ultty reads `ultty/config.toml` in the platform config directory
(`~/.config` on Linux, `~/Library/Application Support` on macOS) at start and
reloads it when the file changes. Every key is optional; missing
keys keep their defaults. The full set of keys is defined in
`crates/ultty/src/config.rs`.

This page covers the settings that decide what programs running in a pane are
allowed to do.

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
