# Ultty - Project DNA

**Mission**: Build the terminal emulator + TUI framework that makes Claude Code's architecture obsolete.

## Quick Context

```
ultty/
├── crates/
│   ├── ultty/            # The terminal emulator (GPU-rendered)
│   ├── ulttui/           # The framework (apps link this)
│   └── ultty-protocol/   # Shared wire protocol
├── examples/             # Reference implementations
├── docs/                 # Specs and guides
└── tools/                # Dev utilities
```

## The Two Products

| Crate | What It Is | Who Uses It |
|-------|-----------|-------------|
| `ultty` | Terminal emulator binary | End users install this |
| `ulttui` | TUI framework library | Developers build apps with this |
| `ultty-protocol` | Wire protocol definitions | Both above depend on this |

## Architecture (30-Second Version)

```
┌─────────────────────────────────────────────────────────────┐
│                    User's Terminal                          │
│                       (ultty)                               │
├─────────────────────────────────────────────────────────────┤
│  Scene Graph ◄── Layout Engine (Taffy) ◄── Protocol Parser │
│       │                                          ▲          │
│       ▼                                          │          │
│  GPU Renderer ──────────────────────── Binary Protocol      │
│    (wgpu)                              (Unix socket)        │
└─────────────────────────────────────────────────────────────┘
                                               ▲
                                               │
┌──────────────────────────────────────────────┴─────────────┐
│                      TUI App                                │
│                  (built with ulttui)                        │
├─────────────────────────────────────────────────────────────┤
│  App State ──► Component Tree ──► Protocol Messages         │
│                                                             │
│  When in ultty:          Native binary protocol             │
│  When in other terminal: ANSI fallback (via ratatui)        │
└─────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

1. **Terminal-side layout**: Apps declare constraints, terminal computes positions
2. **Retained mode scene graph**: Terminal owns the tree, apps send diffs
3. **Atomic frames**: `BeginFrame`/`EndFrame` prevents tearing
4. **Dual-mode framework**: Native protocol when possible, ANSI fallback otherwise

## File Navigation

| Looking for... | Go to... |
|----------------|----------|
| Protocol wire format | `docs/spec/PROTOCOL_SPEC.md` |
| Architecture overview | `docs/architecture/OVERVIEW.md` |
| Terminal emulator code | `crates/ultty/` |
| Framework code | `crates/ulttui/` |
| Example apps | `examples/` |

## Development Commands

```bash
# Run terminal emulator
cargo run -p ultty

# Run examples
cargo run -p hello-native  # Raw protocol demo (run inside ultty)
cargo run -p counter-app   # Framework counter app
cargo run -p todo-app      # Framework todo list app

# Run tests
cargo test --workspace

# Check all targets
cargo check --workspace --all-targets

# Format
cargo fmt --all

# Lint
cargo clippy --workspace --all-targets
```

## Coding Standards

- **Error handling**: Use `thiserror` for library errors, `anyhow` in binaries/examples
- **Logging**: Use `tracing` crate, not `println!` or `log`
- **Unsafe**: Avoid; if required, document why and add `// SAFETY:` comment
- **Tests**: Unit tests in same file, integration tests in `tests/` directory
- **Docs**: All public items must have doc comments

## Current Status

- [x] Protocol spec drafted
- [x] Protocol implementation (encode/decode)
- [x] Terminal: VT100 parser
- [x] Terminal: GPU renderer
- [x] Terminal: Native protocol handler
  - [x] NativeSession with scene tree + layout engine
  - [x] ProtocolServer (Unix socket listener)
  - [x] Event forwarding (keyboard, resize)
  - [x] Content rendering (Text, RichText, Widgets)
- [x] Framework: NativeBackend
  - [x] Socket connection + handshake
  - [x] View reconciliation
  - [x] Event conversion
- [x] Framework: Component system
  - [x] View trait with native rendering support
  - [x] Text, RichText, TextInput widgets
  - [x] Button, List, ScrollView widgets
  - [x] Column, Row, Stack containers
  - [x] Checkbox, Toggle, Select, Table, Tabs, Modal, Image widgets
  - [x] ProgressBar, Spinner, Divider, Badge utility widgets
- [x] Framework: Elm architecture (Cmd/Sub side effects)
- [x] Framework: Theme system (dark/light presets, thread-local)
- [x] Framework: Focus management (ID-based, Tab/Shift+Tab navigation)
- [x] Framework: Semantic event dispatch (focused_id on KeyEvent)
- [x] Framework: Native protocol content for all widgets
- [x] Framework: ANSI fallback (via ratatui)
- [x] Framework: Snapshot tests (25 tests covering all widget types)
- [x] Framework: Integration tests (MockBackend, headless event loop)
- [x] Framework: E2E test with ulttyctl automation
- [x] Examples
  - [x] hello-native (raw protocol demo)
  - [x] counter-app (framework counter app with focused_id events)
  - [x] todo-app (framework todo list app)
  - [x] dashboard (multi-panel with live updates)
  - [x] form-app (form with validation)
  - [x] chat (chat interface)
  - [x] async-fetch (Cmd::spawn + Spinner + Badge demo)
- [x] Terminal Features
  - [x] Scrollback search (Ctrl+Shift+F)
  - [x] Split panes (Ctrl+Shift+D/E)
  - [x] Configurable keybindings (TOML config)
  - [x] App compatibility tests (htop, nvim, vim, nano, top, tmux, less, claude, opencode)
  - [x] Property-based tests for parser robustness
- [x] CLI Automation (ulttyctl)
  - [x] Unix socket control server
  - [x] Input injection (send-text, send-key, send-raw)
  - [x] Tab management (new-tab, switch-tab, close-tab, tabs)
  - [x] State queries (info, text, selection)
  - [x] Search with regex support
  - [x] Wait-for pattern with regex and timeout
  - [x] Screenshot capture (file or bytes)
  - [x] Batch command execution
  - [x] JSON output mode
  - [x] Token-based authentication

## Dependencies Policy

- Minimize dependencies
- Prefer `no_std` compatible crates where possible
- Audit new dependencies for security
- Pin versions in workspace

## Performance Targets

| Metric | Target | Measured | Margin |
|--------|--------|----------|--------|
| Layout recompute | < 1ms | 27µs | 37x |
| Frame render | < 5ms | 290µs | 17x |
| Input latency | < 10ms | ~0.3ms | 33x |
| Memory (terminal) | < 50MB | ~18MB | 2.8x |
| Memory (framework) | < 10MB | TBD | - |

## CLI Automation (ulttyctl)

Control ultty from the command line via Unix socket:

```bash
# Test connection
ulttyctl ping

# Send input
ulttyctl send-text "ls -la"
ulttyctl send-key enter
ulttyctl send-key c ctrl  # Ctrl+C

# Tab management
ulttyctl new-tab htop
ulttyctl tabs
ulttyctl switch-tab 2
ulttyctl close-tab

# Query state
ulttyctl info              # Terminal size, cursor, tab count
ulttyctl text              # Visible text
ulttyctl text --scrollback # Include scrollback

# Search (supports regex)
ulttyctl search "error"
ulttyctl search --regex "error|warning"

# Wait for pattern (useful for automation)
ulttyctl wait-for "$ " --timeout 5000
ulttyctl wait-for --regex "\\$\\s*$" --timeout 5000

# Screenshot
ulttyctl screenshot /tmp/screen.png
ulttyctl screenshot  # Returns bytes to stdout

# Batch commands (atomic execution)
ulttyctl --json batch '[{"type":"send_text","text":"ls"},{"type":"send_key","key":"Enter"}]'

# JSON output for scripting
ulttyctl --json info
ulttyctl --json tabs
```

**Socket location**: `$XDG_RUNTIME_DIR/ultty-ctl.sock`

## AI Agent Testing with ulttyctl

When testing ultty from an AI agent (like Claude Code), use `ulttyctl` instead of tmux for reliable automation.

### Setup

1. Start ultty in a tmux session (for persistence):
```bash
tmux new-session -d -s ultty-test
tmux send-keys -t ultty-test "ULTTY_PTY_LOG=/tmp/pty.bin ./target/release/ultty" Enter
```

2. Verify connection:
```bash
./target/release/ulttyctl ping  # Should return "PONG"
```

### Common Workflow

```bash
# Check terminal state
./target/release/ulttyctl info
# Output: 88x35 tab:1/1 cursor:(0,1) focused:true

# Send a command
./target/release/ulttyctl send-text "claude"
./target/release/ulttyctl send-key enter

# Wait for app to load
sleep 5

# Get visible text (for verification)
./target/release/ulttyctl text

# Take screenshot (for visual debugging)
./target/release/ulttyctl screenshot /tmp/screenshot.png

# Send keyboard shortcuts
./target/release/ulttyctl send-key q          # Press 'q'
./target/release/ulttyctl send-key c ctrl     # Ctrl+C
./target/release/ulttyctl send-key escape     # Escape
```

### PTY Capture for Debugging

Enable PTY logging to capture raw terminal data:
```bash
ULTTY_PTY_LOG=/tmp/pty.bin ./target/release/ultty
```

Analyze the capture:
```bash
xxd /tmp/pty.bin | head -50           # View hex dump
strings /tmp/pty.bin | grep "error"   # Search for text
```

### Key Differences from tmux

| tmux | ulttyctl |
|------|----------|
| `tmux send-keys` may not work with GPU terminal | `ulttyctl send-text/send-key` works reliably |
| `tmux capture-pane` captures tmux's view | `ulttyctl text` captures ultty's actual buffer |
| No screenshot support | `ulttyctl screenshot` captures GPU-rendered output |

## Testing Infrastructure

### Test Layers

| Layer | Type | Framework |
|-------|------|-----------|
| Unit | Inline `#[cfg(test)]` | std |
| Property | Parser robustness | proptest |
| Reference | Data-driven ANSI tests | Custom |
| Snapshot | Widget rendering | insta |
| Integration | App compatibility | TestTerminal |
| Widget | Ratatui rendering | TestBackend |

### TestTerminal Helper

```rust
use ultty::testing::TestTerminal;

let mut term = TestTerminal::with_size(80, 24)?;
term.send("echo hello\r")?;
term.wait_until_stable(Duration::from_millis(200), Duration::from_secs(5))?;

// Fluent assertions
term.verify()
    .assert_contains("hello")
    .assert_main_screen()
    .assert_cursor_at(0, 1);

// Snapshot comparison
let snapshot = term.snapshot();
assert!(snapshot.has_colors());
```

### Running Tests

```bash
cargo test --workspace           # All tests
cargo test -p ultty              # Terminal tests only
cargo test -p ulttui             # Framework tests only
cargo insta review               # Review snapshot changes
```
