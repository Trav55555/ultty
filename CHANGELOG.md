# Changelog

All notable changes to Ultty are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor
versions may break the native protocol, the `ulttui` API, and config keys.

## [Unreleased]

### Added

- `ulttui` accessibility (ADR 0021 slice 7): `View::semantics` says what a
  view is, and every built-in widget reports its role, name and state
  (checkbox and switch checked, progress value, tree items expanded and
  level, tabs and list rows position, modal dialogs, heading levels;
  dividers and spacers hidden). `Semantic` sets the role, label,
  description, liveness or hidden flag of any view without adding a region.
  New builders: `.label()` on `TextInput`, `Select`, `List`, `Tree`,
  `Image` (its alt text) and `MarkdownView`, `.description()` on
  `TextInput`, `Select` and `Button`, `.live()` on `MarkdownView`,
  `TextInput::password` and `Title::level`. In Ultty the native backend
  sends `SetSemantics` only where it adds to what the terminal derives from
  content, and only when it changes. `MarkdownView::streaming(true)` is a
  busy, polite live log whose streamed text a screen reader announces, once
  and without unresolved markdown markers; natively the block still arriving
  is the live region and no cursor is drawn. Semantics strings are cut to
  the codec's limits. ANSI mode is unchanged.
- `MarkdownView` natively is one region per block whether it streams or not,
  and a fenced code block no longer has its last row cut off.
- `ultty_protocol::derive_semantics`: the terminal's rule for a region's
  semantics from its content, for apps to compare with.
- The `chat` example streams its replies into a live transcript;
  `scripts/e2e/accessibility-chat.sh` checks what AT-SPI announces.
- `ulttui-lua` (prototype): write a ulttui app in Luau, run it with
  `ulttui-lua app.luau`, and every save reloads it, keeping its state. The
  script returns an Elm-style app table (`init`, `update`, `view`, named
  `every` timers) and builds views from `text`, `column`, `row`, `card`,
  `scroll`, `list`, `progress`, `spinner`, `button` and `input`; unknown
  props are errors with file and line. `run{}` starts processes only with
  `--allow-exec`; `json.decode` reads their output. Scripts run in Luau's
  sandbox. A broken save leaves the last good version running under an
  error banner. Native in Ultty, ANSI elsewhere. Examples in
  `examples/lua/`; see [Luau apps](docs/lua.md).
- `ulttui`: `Row::padding` and `List::selected` builders.

- `ulttui::inline::Console` (ADR 0020 slice 5): a command-line program
  prints a transcript with live cards. In Ultty each card is an inline
  surface drawn in the scrollback and left there as text when closed;
  elsewhere (tmux, other terminals, no tty) it is printed as the view's ANSI
  rendering and redrawn in place only while it is the last thing printed
  (never when stdout is not a terminal: a card is then written once, in its
  last state). A thread of its own keeps the session alive while the app
  blocks, and a lost session falls back to text instead of failing.
  In-band, keys typed while the console is native are kept for
  `take_input` instead of being swallowed. See [inline cards](docs/inline-cards.md).
- `ultty-client`: `open_surface` writes a surface's anchor and fallback
  text and sends `CreateSurface` with its contents in one frame;
  `close_surface`, `write_output`, `set_output` and `is_closed` go with it.
  The client now tracks `Resize`, so `viewport_width` stays current.
  In-band it keeps the keys it reads while looking for protocol data
  (`take_input`) instead of dropping them, can send its heartbeat without
  reading the tty (`heartbeat`), and connects on a given terminal
  (`connect_in_band_on`).
- `examples/agent-transcript`, a made-up agent session with a tool card and
  a diff card, and `scripts/e2e/agent-transcript.sh`, which checks it in a
  headless Ultty (screenshot and `text --scrollback`, with shell output
  under the live cards) and under tmux.

### Changed

- `ultty_protocol::Constraints::default()` has zero margins, as in CSS,
  instead of `Auto` ones, which took a container's free space: a column of
  fixed-height widgets taller than its content spread them apart. The wire
  is unchanged (margins are always sent); code that wants an `Auto` margin
  sets it. Full-scene apps lay out again: content that `Auto` margins used
  to spread out or centre now packs to the start of its container; centre
  it with `justify_center`/`align_center`, or spread it with the new
  `Column::justify_around` (porygon-tui's boot screen does). `ScrollView`, `List`, `Table`
  and `Modal` now pad their children one cell inside their border, where
  the ANSI renderer puts them; before, a native border took no room and
  children could sit on it.

### Fixed

- `DiffView`'s line texts are one cell high in native mode; they were laid
  out with no height.

## [0.2.0] - 2026-11-08

Highlights: a security review's findings fixed (paste injection, clipboard
queries, link schemes, per-process control sockets, key routing); much wider
terminal compatibility (kitty keyboard protocol, IME, undercurl and underline
colour, truthful DA/DECRQM/XTVERSION replies, a VT conformance suite); a
control plane for agents and TUI tests (`ulttyctl panes`, `subscribe`,
`last-command`, `wait-for --command-done`, MCP); and large speedups: non-ASCII
text is about 3x faster to process, ASCII about 1.3x, Wayland throughput no
longer collapses, and an idle window no longer wakes up.

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

- Ultty exposes its window to screen readers through AccessKit (AT-SPI on
  Linux), on by default (`[behavior] accessibility = false` turns it off).
  It joins the accessibility bus only while assistive technology runs, and
  an idle window still does not wake up: `blocking` is pinned to 1.6.2,
  because 1.7.0 keeps a pool thread waking twice a second forever.

- Shift+click and Shift+drag select text even while a program tracks the
  mouse, as in xterm and other terminals; the program sees none of it.

- Shell integration for bash 4.4+ and zsh in `dist/shell-integration/`
  (OSC 133 prompt, command and exit status marks), installed by
  `install.sh`, `make install`, the packages and the Homebrew formula under
  `share/ultty/shell-integration/`. `last-command`, `wait-for
  --command-done` and command events need it. Guides: `docs/agents.md` and
  `docs/testing-tuis.md`, with `scripts/e2e/counter-app.sh` and
  `scripts/e2e/run-headless.sh`.
- `ulttyctl new-tab` prints the new tab's pane id (control reply
  `tab_opened`), so a script can write `pane=$(ulttyctl new-tab ...)`.
- `make install` installs `ulttyctl` too.
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
- `ulttyctl [--pane <id>] regions` (control request `regions`, MCP tool
  `ultty_regions`) shows the native apps in a pane's tab: each app's
  region tree in drawing order with every region's position and size in
  cells, scroll, content kind and text, metadata, and the focused region.
  `--json` has the full text, so a test can assert on a native program
  ([testing TUIs](docs/testing-tuis.md)). Read-only.
- Native protocol: `SetSemantics` (`0x0026`, capability
  `CAP_SEMANTICS = 0x0200`) gives a region an accessibility role, label,
  liveness and state ([ADR 0021](docs/adr/0021-accessibility-semantics.md),
  spec §10.5). Regions without it get a role derived from their content;
  text a region received by `AppendContent` makes it a `Log`. `regions`
  reports each region's effective `role`, `label` and `live`. Nothing is
  exposed to screen readers yet.
- Native protocol: inline surfaces, part one (capability
  `CAP_INLINE_SURFACES = 0x0100`, [ADR 0020](docs/adr/0020-inline-surfaces.md),
  spec §10.6). `CreateSurface` (`0x0015`) opens a region tree laid out on its
  own at the pane's width by 1 to 128 rows, `CloseSurface` (`0x0016`) or
  `DeleteRegion` closes it, and the terminal answers `SurfaceEvent`
  (`0x0026`) `Closed`. A session holds at most 16, and surface regions
  cannot subscribe to keys. `regions` lists each surface with its rows,
  state and region tree.
- Native protocol: inline surfaces, part two (ADR 0020 slice 3). The
  terminal advertises `CAP_INLINE_SURFACES`, pairs each surface with the
  anchor its app printed (`ESC _ u <session>;<surface>;<rows> ESC \`) in
  either order, and draws it over those rows of a single-pane tab, moving
  with them through output and scrollback. `SurfaceEvent` reports `Placed`,
  `Hidden` and `Shown` as it leaves and returns to the screen, and
  `Evicted` when its rows leave scrollback or are erased. A surface or
  anchor without its other half for 2 s, or whose rows disagree, is
  dropped with `Error` 400 and the fallback text stays. `regions` shows
  `placed`, `unplaced` or `hidden` and the anchor line. A frozen surface
  keeps the app's fallback text; drawing its last frame into the rows comes
  with rasterisation. A session with no full-scene regions no longer hides
  the grid, and gets keys only once it has some. `scripts/e2e/inline-surface.sh` checks it headless.
- `ulttui` names each region it creates for a widget with an id
  (`SetRegionMetadata`, key `id`) and tells the terminal which region has
  focus, so `regions` can find widgets by id.
- `ulttyctl [--pane <id>] last-command [--tail <lines>]` (control request
  `last_command`, MCP tool `ultty_last_command`) prints the last command the
  pane's shell ran, its output and how it ended, or what it has printed so
  far while it runs, and exits with the command's status. It needs OSC 133
  shell integration. Output survives scrolling into the scrollback and
  window resizes; when its lines are gone (cleared, reset, or out of the
  scrollback) it says so instead of showing another command's output. OSC
  133 marks now live on rows (ADR 0019) at no memory cost.
- `ulttyctl [--pane <id>] status <state> [label] [--progress <0-1|N%>]
  [--attention]` (control request `set_pane_status`, MCP tool
  `ultty_set_status`) shows what a pane is doing: a colored marker before
  its tab title (blue working, yellow waiting, green done, red error), a
  progress line along the tab, and the focused pane's marker and label after
  the location in the title band. A tab shows its most urgent pane.
  `--attention` marks an unfocused window urgent. `status clear` removes
  it; `ulttyctl panes` lists it. It replaces `set_agent_status`, which was
  accepted but never shown.
- `ulttyctl mcp` serves Ultty's control plane to AI agents over MCP
  (stdio): list panes, read text, type text, send keys, run a command, wait
  (for text, a finished command with its exit code, the bell, or quiet),
  search, open a tab (returns its pane id), screenshot and info. Every pane
  tool takes a pane id. For Claude Code: `claude mcp add ultty -- ulttyctl
  mcp`. It replaces `mcp/ultty-mcp-server.py`, which framed messages with
  `Content-Length` headers that MCP's stdio transport does not use.
- `ulttyctl wait-for --command-done` (exits with the command's status),
  `--bell` and `--quiet <ms>` wait for the next command to finish, the next
  bell, or a period without output, instead of for text. Control request:
  `wait_for` with `until`.
- `behavior.accessibility` (default `false`) exposes the window to screen
  readers through AccessKit (AT-SPI on Linux): for now a single window node
  named after the title. It is off by default because its D-Bus client wakes
  an idle window twice a second.
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
- Mode 2031 and `CSI ? 996 n`: programs can ask whether the theme is dark
  or light, and hear when it switches, as in contour, kitty and Ghostty.
- `docs/compat-scorecard.md`: which modern terminal features Ultty supports,
  generated from tests by `scripts/compat-scorecard.sh`.

### Changed

- Throughput: the VT core takes 25-40% fewer instructions per byte on
  non-ASCII text and about a third fewer on ASCII (a combined width and
  cluster-class table, an in-place cell writer, scrollback that copies plain
  rows instead of re-encoding them, single-pass UTF-8 decoding), and the
  parser keeps escape sequences on a fast path. On a Tesla T4, live
  unicode-mixed output went from 16 to 24 MiB/s and ASCII from 71 to 93 MiB/s.
- Frames are drawn when the window system asks for them: under Wayland a
  hidden or throttled window no longer blocks the main thread in present, and
  frames are presented after the terminal locks are released, so output keeps
  being parsed while a frame waits.
- The minimum supported Rust version is 1.90, raised from 1.89: `wgpu-hal`
  30 depends on `ordered-float` 5.5, which needs it. The CI MSRV job did not
  actually build with the MSRV before, so this went unnoticed.

- The glyph atlas starts at 1024x1024 and grows to 2048x2048 only when it
  fills, halving GPU memory for a typical session (22.9 to 11.4 MB under
  lavapipe).
- Output in a background tab no longer redraws the window. A hidden tab
  streaming 100 lines a second cost 272 frames and 0.24 s of CPU per 5 s;
  now none beyond the cursor blink, and 0.05 s.
- A command given by name runs the first executable file of that name on
  `PATH`. A file without execute permission, such as the `~/.local/bin/env`
  that uv installs to be sourced, no longer hides `/usr/bin/env` (it made
  `ulttyctl new-tab env ...` fail).
- `ulttyctl new-tab` with a working directory (the control request's
  `working_dir`, the MCP tool's `cwd`) starts the command there; it was
  ignored.
- Naming the program in front of a pane (tab titles, `ulttyctl panes`, the
  native session foreground check) reads the process group's leader and
  walks only its relatives, not every process on the machine. With the
  shell in front, five commands' worth of title refreshes went from 7,805
  `/proc` opens to 150.
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

- Under Wayland, output throughput fell to about 1 MiB/s because a frame held
  the terminal lock while waiting to present.
- A shell's last output could be cut off on a busy machine: the exit is now
  reported only after the PTY's unread output has been read (2 s at most).
- A program that tracks the mouse with motion (modes 1002/1003, as Claude
  Code's full mouse mode sets) now sees drags: they were reported as
  motion with no button (code 35) and the release as button 3, so Claude
  Code's text selection never extended. Drags report the held button and
  an SGR release names it. Mode 1002 alone no longer reports motion with
  no button held.

- Resizing the window no longer leaves the process up to 100 MB larger.
  Reflow decoded the whole scrollback and built every row at full size
  before compacting it again, peaking at 149 MB of heap for 10,000 lines,
  which the allocator kept; it now rewraps line by line and peaks at
  4.7 MB. It is also about three times faster.
- A progress report (`OSC 9;4`) is shown as progress on the tab, not as a
  desktop notification reading "4;1;50". Control-socket statuses take
  precedence; the report is cleared by `OSC 9;4;0` or, with shell
  integration, when the command finishes.
- An `ulttui` app run over ssh from a tmux pane no longer renames that pane.
  The in-band probe now asks the terminal's name first and sends its hello
  only to Ultty, at the cost of one more round trip at start.
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

[Unreleased]: https://github.com/Trav55555/ultty/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Trav55555/ultty/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Trav55555/ultty/releases/tag/v0.1.0
