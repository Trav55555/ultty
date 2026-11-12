<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo/ultty-animated-dark.svg">
    <img alt="ultty" src="docs/assets/logo/ultty-animated-light.svg" width="460">
  </picture>
</p>

# Ultty

A GPU-accelerated terminal that replaces ANSI escape sequences with a native binary protocol for TUI applications.

## Why

A TUI that draws with ANSI escape sequences turns its layout into characters
at fixed cells before the terminal sees anything. The terminal gets a grid of
cells, not the structure behind it. It cannot tell a sidebar from a log, so it
cannot scroll, resize, or select inside one part of the screen without the app
redrawing it, and every app carries its own layout engine (ratatui, ncurses,
Bubble Tea) to do that work.

Speed and tearing are not the problem. Parsing escape sequences is cheap:
Ultty parses and applies more than 100 MiB/s of terminal output
([measurements](docs/perf/2026-11-05-io-thread.md)). Synchronized output (mode
2026), which Ultty and most current terminals support, already lets an app
mark frame boundaries so a half-drawn frame is never shown.

Ultty tests whether moving layout into the terminal, with the structure on the
wire, makes TUIs simpler to write and lets the terminal do more for them. The
native protocol is experimental. ANSI support is the baseline, and every
ordinary terminal program runs as it would in xterm.

## The Solution

Ultty introduces a native binary protocol that lets applications declare *what* to render, while the terminal handles *where* and *how*:

```
App declares constraints ──→ Binary protocol ──→ Terminal computes layout ──→ GPU renders
                                                       ▲
                                                 Retained scene graph
                                                 (apps send diffs, not full frames)
```

**Terminal-side layout** — Apps send flex/grid constraints. The terminal runs Taffy layout and positions regions.

**Retained scene graph** — The terminal owns a tree of regions. Apps create, update, and delete regions. Only diffs cross the wire.

**Binary protocol** — A compact length-prefixed binary format carrying structured messages: `CreateRegion`, `SetContent`, `AppendContent`, `SetConstraints` instead of escape sequences. The decoder allocates; it is not zero-copy. The win is that the terminal never re-parses an escape-sequence stream to recover structure the app already knew.

**Atomic frames** — `BeginFrame`/`EndFrame` batching prevents tearing. The terminal renders nothing until the frame is complete.

## Quick Start

### Run the terminal

```bash
cargo run -p ultty --bin ultty
```

This opens Ultty as a GPU-accelerated terminal emulator. It runs existing terminal programs (vim, htop, tmux, and so on) through its xterm-compatible emulation; see [ANSI Compatibility](#ansi-compatibility) for what is and is not implemented.

To open the window running a program instead of your shell, put it after `-e`. Everything after `-e` goes to the program, so its own flags need no quoting. `--` works the same way, and `ultty --help` lists the options.

```bash
ultty -e htop
ultty -e nvim -u NONE notes.txt
```

The tab closes when the program exits, like any other tab. A window opened with `-e` does not restore or save the session, so your saved tabs stay as they were. Launchers that follow the xdg-terminal-exec convention (`dist/ultty.desktop` sets `X-TerminalArgExec=-e`) open programs this way.

Platform status: Ultty is Unix-first. Release builds exist for Linux x86_64 only. Building from source on macOS is expected to work, but there are no macOS release builds and CI does not test macOS. Windows support is future work that requires a native transport and PTY platform boundary.

Linux desktop packages install `dist/ultty.desktop` and the Ultty app icon from the hicolor icon set. Brand asset usage is documented in [docs/brand.md](docs/brand.md).

### Installing a release build

Release builds exist for Linux x86_64 only. On other platforms, build from
source as above.

```bash
curl -fsSL https://raw.githubusercontent.com/Trav55555/ultty/main/install.sh | sh
```

[`install.sh`](install.sh) downloads the latest release, checks it against
the release's `SHA256SUMS`, and installs `ultty` and `ulttyctl` into
`~/.local/bin`, the application-menu entry and icons into `~/.local/share`, and
the `xterm-ultty` terminfo entry into `~/.terminfo` when `tic` is available.
It needs no root, and on any platform other than Linux x86_64 it stops with an
error. Set `ULTTY_VERSION=v0.1.0` for a specific release or
`ULTTY_INSTALL_DIR` for another location.

`SHA256SUMS` comes from the same release as the tarball, so it catches a
corrupted download, not a tampered release.

The release also has a Linux x86_64 tarball, `.deb` and `.rpm` packages, and
`SHA256SUMS`. Changes between releases are in [CHANGELOG.md](CHANGELOG.md).

Useful runtime overlays:

- `Ctrl+Shift+?` opens the keyboard shortcuts overlay.
- `Ctrl+Shift+I` opens the native protocol status overlay with socket, session, dirty-state, layout, and recent-error details.

### Run native apps inside Ultty

From within an Ultty terminal:

```bash
cargo run -p hello-native   # Raw protocol demo, no framework
cargo run -p counter-app    # Elm-architecture counter
cargo run -p todo-app       # Todo list with vim navigation
cargo run -p dashboard      # Multi-panel dashboard with live updates
cargo run -p chat           # Streaming chat layout
cargo run -p form-app       # Form widgets and focus traversal
cargo run -p split-panes    # Nested split layout
cargo run -p sysmon         # System monitor
cargo run -p async-fetch    # Async command and subscription demo
```

`examples/porygon-tui` is not a workspace member. Build and run it from its own
manifest:

```bash
cargo run --manifest-path examples/porygon-tui/Cargo.toml
```

Native apps auto-detect whether they're talking to Ultty. On an Ultty tab's own terminal they use the tab's socket. Anywhere else they probe the terminal, and if it is Ultty they carry the native protocol in-band in the terminal byte stream. That covers ssh, `sudo`, containers, and `script` recordings. In both cases the terminal does layout and rendering. Everywhere else they use ANSI mode via ratatui on supported Unix terminals. That includes tmux, screen, and zellij, where Ultty cannot know where a pane sits, so the probe is skipped or dropped. Set `ULTTY_IN_BAND=0` to skip the probe. See [spec §1.3](docs/PROTOCOL_SPEC.md) and [ADR 0018](docs/adr/0018-in-band-native-transport.md).

## Privacy

No account, no telemetry, no network calls. Ultty does not phone home,
check for updates, or report crashes; it contains no networking code. The
network is reached only by programs you run in it, and by a link you
Ctrl+click, which Ultty hands to your browser. Its sockets (the control
socket that `ulttyctl` uses and each tab's native socket) are local Unix
sockets that only your user can open.

## ANSI Compatibility

Ultty emulates an xterm-style terminal; ordinary terminal programs are the
baseline, not an afterthought. `TERM` is `xterm-256color` by default, or
`xterm-ultty` with `terminal.term` (see [Configuration](docs/configuration.md#terminal-type)).
The [compatibility scorecard](docs/compat-scorecard.md) checks, from tests,
the features that agent CLIs and other modern programs rely on.

Implemented:

- VT100/VT220 control sequences: cursor movement, scroll regions (DECSTBM),
  origin mode, insert and delete, erase, tab stops, character sets, DECSC and
  DECRC, DECSTR, RIS, insert mode, LNM, and DECCOLM behind mode 40
- 256 colours and 24-bit colour, OSC 4/104 palette changes, OSC 10/11/12
  colour queries that report the live theme
- Underline styles (single, double, curly, dotted, dashed; `SGR 4:x`) and
  underline colour (`SGR 58`/`59`), with colon sub-parameters
- Keyboard: xterm encoding, modifyOtherKeys (levels 1 and 2), the kitty
  keyboard protocol (flags 1, 2, 4, 8 and 16), input methods (IME)
- Mouse reporting: X10, normal, button and any-event tracking, SGR (1006),
  UTF-8 (1005), urxvt (1015), alternate scroll (1007)
- Bracketed paste (pasted text has ESC and C1 controls removed), focus
  reporting
- Alternate screen (47, 1047, 1049), synchronized output (2026), grapheme
  clustering (2027), light/dark theme reports (2031 and `CSI ? 996 n`),
  DECSCNM reverse video
- Sixel graphics, OSC 8 hyperlinks, OSC 52 clipboard writes (reads are
  refused), OSC 7 working directory, OSC 133 prompt marks, OSC 9 and 777
  notifications, OSC 9;4 progress on the tab
- Queries: DA1, DA2, DSR, DECXCPR, DECRQM, DECRQSS, XTVERSION, XTGETTCAP,
  XTWINOPS 14/16/18, the title stack (XTWINOPS 22/23)
- Tabs, splits, scrollback, vim-style copy mode

Not implemented:

- Left and right margins (DECSLRM, mode 69). `xterm-256color` advertises them
  (`smglr`); `xterm-ultty` does not.
- Setting the cursor colour (OSC 12 with a colour, OSC 112). Queries work.
- Printer port (media copy), HP memory lock, meta mode (1034).
- Protected cells (DECSCA). DECSED and DECSEL erase like ED and EL.
- Rectangular area operations (DECFRA, DECERA, DECCRA and the rest) and VT52
  mode.

The conformance suite in
[`crates/ultty-vt/tests/conformance.rs`](crates/ultty-vt/tests/conformance.rs)
checks expected screens and replies for these areas. vttest has not been run
against this version.

## Framework: ulttui

`ulttui` is an Elm-architecture framework for building native TUI apps:

```rust
use ulttui::prelude::*;

struct App { count: i32 }

enum Msg { Inc, Dec }

impl Application for App {
    type Message = Msg;

    fn update(&mut self, msg: Msg) -> Cmd<Msg> {
        match msg {
            Msg::Inc => self.count += 1,
            Msg::Dec => self.count -= 1,
        }
        Cmd::none()
    }

    fn on_event(&mut self, event: &Event) -> Option<Msg> {
        match event {
            Event::Key(k) => match k.key {
                Key::Char('+') => Some(Msg::Inc),
                Key::Char('-') => Some(Msg::Dec),
                _ => None,
            },
            _ => None,
        }
    }

    fn view(&self) -> impl View {
        Column::new()
            .child(Text::new(format!("Count: {}", self.count)))
            .child(Row::new()
                .child(Button::new("-"))
                .child(Button::new("+")))
    }
}

fn main() -> ulttui::Result<()> {
    App { count: 0 }.run()
}
```

Widgets: `Text`, `RichText`, `MarkdownView`, `Button`, `List`, `TextInput`, `ScrollView`, `Column`, `Row`, `Stack`

All widgets target both Ultty-native mode and ANSI mode.

Markdown streaming is available through `MarkdownView::streaming(true)`. In
native mode, append-shaped text growth can use `AppendContent` rather than
replacing the full region; structural markdown changes still fall back to
normal content updates.

## Performance

Ultty performance is defined first by idle efficiency and interaction responsiveness. Microbenchmarks are supporting evidence; live PTY, parser/grid, render, and screenshot gates are the release guardrails.

Current dated reports:

- [Baseline benchmarks](docs/perf/2026-05-14-baseline.md)
- [ASCII span and grid write path](docs/perf/2026-05-14-ascii-span.md)
- [Live pipeline trace and PTY readiness wakeups](docs/perf/2026-05-14-live-pipeline-trace.md)
- [PTY throughput after the I/O thread](docs/perf/2026-11-05-io-thread.md)
- [In-band transport prototype benchmark](docs/perf/2026-11-05-in-band-transport.md)
- [Where VT throughput goes: scrollback, not the render lock](docs/perf/2026-11-06-scrollback-cost.md)
- [Visual regression smoke](docs/visual-regression.md)

| Metric | Target |
|--------|--------|
| Idle CPU/GPU work | near-zero when terminal and native app state are unchanged |
| Layout recompute | < 1ms |
| Frame render | < 5ms |
| Input latency | < 10ms (3.6 ms keypress to pixel on a Tesla T4 under X11, idle) |
| Memory (terminal) | < 50MB |

A keystroke's echo is drawn without waiting for the 16 ms frame interval. Key
press to pixels on screen was measured on a Tesla T4 under X11 (XTest
injection, then polling the window's pixels): 3.6 ms median idle, level with
Alacritty, 5.5 ms under 10 MiB/s of output and 22 ms under maximum output
([T4 comparison](docs/perf/2026-11-08-t4-comparison.md)). Wayland and the
development workstation are not measured yet. The `input_latency` bench
measures only parsing and grid updates, in nanoseconds to microseconds.

Run the published benchmark set:

```bash
timeout 900s cargo bench -p ultty --bench layout -- --noplot
timeout 900s cargo bench -p ultty --bench vt_small_writes -- --noplot
timeout 900s cargo bench -p ultty --bench render -- --noplot
cargo test -p ultty --bench memory    # heap-byte budgets; fails when over
```

Headless VT throughput over deterministic CRLF corpora, the A/B and A/A
statistics, the instruction-count gate and the comparative live suite are
described in [docs/perf/README.md](docs/perf/README.md#benchmark-suite).

Run the dependency security audit separately from the normal local build/test
loop:

```bash
cargo install cargo-audit --version 0.22.1 --locked
scripts/security-audit.sh
```

The current accepted warning-only advisory exceptions are documented in
[docs/security-audit.md](docs/security-audit.md).

Run live gates when a GUI environment is available:

```bash
scripts/live-pipeline-trace.sh /tmp/ultty-live-pipeline-trace
scripts/softwrap-pty-probe.sh /tmp/ultty-softwrap-pty-probe
scripts/terminal-workflow-smoke.sh /tmp/ultty-terminal-workflow-smoke
```

Run the renderer screenshot smoke against a running Ultty instance:

```bash
cargo run -p ultty --bin ultty
scripts/live-smoke-gate.sh /tmp/ultty-live-smoke-gate
scripts/screenshot-smoke.sh /tmp/ultty-screenshot-smoke
scripts/native-richtext-smoke.sh /tmp/ultty-native-richtext-smoke
scripts/append-content-smoke.sh /tmp/ultty-append-content-smoke
```

See [Visual Regression](docs/visual-regression.md) for the canonical screenshot
smoke commands and artifact meanings.

## Architecture

```
crates/
├── ultty/            Terminal emulator — GPU renderer (wgpu), PTY, native
│                     protocol server, scene graph, tabs/splits, ulttyctl
├── ultty-vt/         VT parser and screen model (grid, scrollback, reflow);
│                     no I/O or GPU dependencies
├── ulttui/           TUI framework — Application trait, widgets, Elm architecture,
│                     dual-mode rendering (Ultty-native + ANSI mode)
├── ultty-client/     Low-level client: terminal detection, in-band transport
└── ultty-protocol/   Wire protocol — Message definitions, layout types,
                      content types, shared between terminal and apps
examples/           Nine workspace example apps: hello-native, counter-app,
                    todo-app, dashboard, chat, form-app, split-panes, sysmon,
                    async-fetch. The porygon-tui example is excluded from the
                    workspace and built from its own manifest.
```

## Status & Roadmap

**Working:**
- Native binary protocol with retained scene graph
- Append-oriented native protocol updates for streaming text
- GPU-accelerated rendering via wgpu
- xterm-compatible emulation (see [ANSI Compatibility](#ansi-compatibility))
- Framework with Elm architecture and widget system
- Markdown streaming affordance for partial assistant responses
- Dual-mode apps (Ultty-native mode inside Ultty, ANSI mode on supported Unix terminals)
- Flexbox and grid layout (via Taffy)
- Protocol hardening (bounds checks, size limits)
- Tabs, splits, vim-style copy mode

**Next:**
- Mouse event forwarding to native apps
- Animation primitives
- Image widget (native protocol)
- Scrollback search in native regions
- Release builds for macOS and Linux aarch64

## Documentation

- [Contributing](CONTRIBUTING.md) — Toolchain, checks, and pull request expectations
- [Getting Started](docs/GETTING_STARTED.md) — Installation and first app
- [Configuration](docs/configuration.md) — `TERM`, scrollback, exit, clipboard, paste, and link settings
- [Drive Ultty from an agent](docs/agents.md) — MCP, shell integration, panes, waits, last command, events
- [Test your TUI with ulttyctl](docs/testing-tuis.md) — End-to-end tests that press keys and read the screen
- [Inline cards](docs/inline-cards.md) — Native cards in a command-line program's scrollback, ANSI text elsewhere
- [Changelog](CHANGELOG.md) — Changes in each release
- [Architecture](docs/ARCHITECTURE.md) — System design and components
- [Architecture Vocabulary](CONTEXT.md) — Stable terms for core seams
- [Protocol Specification](docs/PROTOCOL_SPEC.md) — Native protocol details
- [Layout Model](docs/LAYOUT_MODEL.md) — How terminal-side layout resolves constraints
- [Widget Reference](docs/WIDGET_REFERENCE.md) — Every `ulttui` widget and its options
- [North Star](docs/NORTH_STAR.md) — Project direction and decision filters
- [Public Alpha Readiness](docs/alpha-readiness.md) — Dogfood, alpha, and general-use release gates
- [Performance Reports](docs/perf/README.md) — Dated benchmark and live-gate evidence
- [Visual Regression](docs/visual-regression.md) — Renderer screenshot smoke path
- [Live Smoke Automation](docs/live-smoke-automation.md) — Dedicated GUI runner workflow and artifacts
- [Widget Readiness](docs/widget-readiness.md) — Dual-mode widget evidence matrix
- [Architecture Deepening Backlog](docs/architecture-deepening-backlog.md) — Runtime and protocol implementation-depth slices
- [Release Artifact Smokes](docs/release-artifacts.md) — Headless package and tarball verification
- [Release Metadata](docs/release-metadata.md) — License, package, and desktop metadata conventions
- [Release Candidate Template](docs/releases/RC_TEMPLATE.md) — Per-candidate evidence checklist
- [Testing Strategy](docs/TESTING_STRATEGY.md) — How to test
- [30/60/90 Execution Plan](docs/ROADMAP_30_60_90.md) — Milestones, ticket map, and acceptance criteria
- [30/60/90 Issue Backlog](docs/ISSUE_BACKLOG_30_60_90.md) — Copy-paste issue templates with dependencies
- [Archive](docs/archive/) — Historical plans and reports, no longer maintained

## Development

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
