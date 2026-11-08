# Changelog

All notable changes to Ultty are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor
versions may break the native protocol, the `ulttui` API, and config keys.

## [Unreleased]

### Security

- Bracketed pastes drop every ESC and C1 control character, so pasted text
  can no longer end the bracket early and run the rest as typed input. Plain
  pastes send newlines as carriage returns.
- OSC 52 clipboard reads (`OSC 52;c;?`) are refused. They used to erase the
  clipboard.
- `behavior.clipboard_write = "deny"` ignores every OSC 52 write. OSC 52 is
  never written to Ultty's own stdout, even when Ultty was started inside
  tmux.
- Ctrl+click opens only `http`, `https`, `mailto` and `ftp` links. Other
  schemes, `file:` included, are refused and logged.
- Keys typed in a pane reach a native socket session only while that
  session's process group is in the pane's foreground. A background job that
  inherited `ULTTY_SOCKET` no longer sees what is typed.
- A native session holds at most 64 MiB of text, and one region keeps at most
  4 MiB (the oldest text is dropped). Before, an in-band sender could grow
  terminal memory without bound.
- In-band sessions are not drawn, given input, or answered beyond the
  handshake until the app confirms them with a later read. A `cat`ed file
  that contains a `ClientHello` no longer blanks the tab.
- Each Ultty process has its own owner-only (0600) control socket and token,
  removed on exit. `ulttyctl` run inside a window controls that window.
- Shells no longer inherit file descriptors above stderr.

### Added

- `ulttyctl panes` (control request `list_panes`) lists every pane in every
  tab with a pane id, tab index, focus, title, foreground process, shell
  PID, working directory and size. Pane ids are unique within an Ultty
  process and never reused, so they stay valid across splits, focus changes
  and tab moves until the pane closes.
- Control requests that read or drive a pane (`text`, `selection`,
  `send-text`, `send-key`, `send-raw`, `run`, `search`, `wait-for`) take an
  optional pane `target`; `ulttyctl --pane <id> <command>` sets it. Without
  one they act on the focused pane, as before. An id naming no open pane is
  error 6.
- `ulttyctl subscribe [--pane <id>]` (control request `subscribe`) keeps
  the connection open and prints one JSON event per line: `pane_opened`,
  `pane_closed`, `command_started` and `command_finished` with the exit code
  (OSC 133, needs shell integration), `bell`, `notification`,
  `title_changed` and `process_exited`. An idle subscriber costs no
  wakeups. A subscriber more than 1 MiB behind is disconnected, and at most
  64 can be open.
- `ulttyctl wait-for --command-done` (exits with the command's status),
  `--bell` and `--quiet <ms>` wait for the next command to finish, the next
  bell, or a period without output, instead of for text. Control request:
  `wait_for` with `until`.
- `behavior.cursor_blink` (default `true`); `false` keeps the cursor steady.
- `dist/terminfo/ultty.terminfo`, the `xterm-ultty` entry, installed by
  `install.sh`, `make install`, the packages and the Homebrew formula.
- `terminal.term` sets `TERM` for new tabs (default `xterm-256color`).
- The kitty keyboard protocol (flags 1, 2, 4, 8 and 16) and modifyOtherKeys
  levels 1 and 2 (`CSI > 4 ; n m`).
- Input methods (fcitx, ibus): IME is enabled and the composition text is
  drawn at the cursor.
- Curly, dotted and dashed underlines (`SGR 4:3`, `4:4`, `4:5`) and underline
  colour (`SGR 58`/`59`), parsed and drawn.
- Colon sub-parameters in SGR (`38:2::r:g:b`, `4:3`).
- OSC 4 and OSC 104 palette set, query and reset. OSC 10/11/12 report the
  live theme.
- XTVERSION, XTGETTCAP (`TN`, `Co`, `RGB`), DECRQSS (SGR, DECSTBM,
  DECSCUSR), DSR 5, DECXCPR, XTWINOPS 14/16/18 and the title stack (22/23).
- DECRQM answers for every implemented mode, and the ANSI form for IRM, LNM,
  KAM and SRM.
- LNM (mode 20), DECSCNM reverse video, and mode 1048.
- A behavioural VT conformance suite (`crates/ultty-vt/tests/conformance.rs`).
- `behavior.bell_urgent` marks an unfocused window as wanting attention.

### Changed

- A pending `wait-for` watches the pane that was focused when it was sent
  (or its `--pane` target), not whichever pane has focus at each check, and
  fails with error 6 if that pane closes first.
- An idle window no longer wakes up. The cursor blinks only while the
  window has focus and the cursor is in a blinking style; nothing else
  polls unless a native app is connected. A pending `ulttyctl wait-for`
  wakes the window only at its timeout.
  Config edits are reloaded 100 ms after the file stops changing.
- The GPU is opened with the primary API (Vulkan on Linux) alone, and GL
  only if that finds no adapter. Probing GL as well cost startup time and
  memory.
- DA1 reports what is implemented (`CSI ? 62 ; 4 ; 22 c`: VT220 class, sixel,
  ANSI colour). DA2 reports VT220 and the version.
- The VT parser executes C0 controls inside a control sequence, ignores
  sequences it does not know instead of printing their tail, and prints
  U+FFFD for invalid UTF-8.
- DECCOLM (`CSI ? 3 h/l`) is ignored unless mode 40 allows it, so `reset` and
  `tput init` no longer shrink the window to 80 columns.
- Lines scrolled out of a region whose top is row 0 on the main screen go to
  scrollback.
- `CSI 3 J` clears only the scrollback. DECSTR leaves the cursor where it is.
  DECSTBM with top >= bottom is ignored. Setting DECOM homes the cursor.
- Key encoding: Shift+Tab sends `CSI Z`; Ctrl with space, punctuation and
  digits sends the xterm control characters; Ctrl+Backspace sends 0x08 and
  Alt+Backspace ESC DEL; Enter sends CR LF while LNM is set.
- A keystroke's echo is drawn at once instead of waiting for the 16 ms frame
  interval.
- Text rendering rebuilds only rows whose content changed.
- `behavior.scrollback_lines` sets the scrollback length; it used to be
  ignored. `behavior.close_on_exit = false` keeps an exited pane open.
  `font.line_height` is clamped to 0.5 to 3.0.
- `ulttui` returns its own `ulttui::Error` instead of `anyhow::Error`.
- The vim types moved from `ultty-protocol` (where nothing on the wire used
  them) to `ulttui::vim`. Ultty's copy mode has its own mode type.
- The control socket reads whole requests (up to 1 MiB), answers malformed
  ones with an error, and finishes replies larger than the socket buffer.
- The selection survives repaints of other rows.
- New icon.
- `install.sh` refuses platforms without a release build.

### Fixed

- Widening the window gives the new columns tab stops.
- A window larger than the GPU's texture limit no longer aborts the process.
- Sixel images are sized with the real cell size.

### Removed

- `behavior.bell_sound` (Ultty has no audible bell) and the `[tmux]` config
  table. Configs that still have them load.

## [0.1.0] - 2026-11-07

First public release, built for Linux x86_64 only.

### Added

- GPU-rendered (wgpu) terminal emulator with tabs, splits, scrollback, vim-style
  copy mode, sixel images, OSC 8 hyperlinks, OSC 52 clipboard writes,
  bracketed paste, focus reporting, SGR mouse reporting, synchronized output
  (mode 2026) and grapheme clustering (mode 2027).
- The native protocol: apps send layout constraints and content to a
  retained scene graph and the terminal lays out and draws them, over the
  tab's Unix socket or in-band in the PTY stream (ssh, `sudo`, `script`).
- `ulttui`, an Elm-architecture TUI framework whose apps render natively
  inside Ultty and through ratatui in other terminals.
- `ulttyctl` and an MCP server for automation.
- `install.sh`, release tarball, `.deb`, `.rpm`, and a Homebrew formula.

[Unreleased]: https://github.com/Trav55555/ultty/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Trav55555/ultty/releases/tag/v0.1.0
