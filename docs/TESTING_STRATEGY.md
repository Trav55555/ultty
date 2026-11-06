# Ultty Testing Strategy

Ground truth testing approach inspired by Alacritty and Ghostty.

## Test Pyramid

```
                    ┌──────────────┐
                    │  Screenshot  │  Visual regression (Ghostty-style)
                    │   Diffing    │  Pixel-perfect terminal output
                    ├──────────────┤
                 ┌──┤  Integration │  End-to-end scenarios
                 │  │    Tests     │  App → Protocol → Render
              ┌──┤  ├──────────────┤
              │  │  │   Snapshot   │  Insta golden files
              │  │  │    Tests     │  ANSI sequence verification
           ┌──┤  │  ├──────────────┤
           │  │  │  │  Reference   │  Data-driven parser tests
           │  │  │  │    Tests     │  input.bin → expected.json
        ┌──┤  │  │  ├──────────────┤
        │  │  │  │  │  Property    │  Proptest roundtrips
        │  │  │  │  │   Tests      │  Encode/decode correctness
     ┌──┤  │  │  │  ├──────────────┤
     │  │  │  │  │  │    Unit      │  Inline #[test] blocks
     │  │  │  │  │  │    Tests     │  Individual functions
     └──┴──┴──┴──┴──┴──────────────┘
```

## Layer 1: Unit Tests

**Location**: Inline in source files

**Pattern**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_escape() {
        // ...
    }
}
```

**Coverage targets**:
- Every public function
- Edge cases (empty input, max values, unicode)
- Error conditions

## Layer 2: Property-Based Tests (Protocol)

**Location**: `crates/ultty-protocol/tests/roundtrip.rs`

**Status**: Working (40 tests pass, 1 ignored for stack size)

**Pattern**:
```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn message_roundtrip(msg in arb_app_message()) {
        let encoded = msg.encode_to_bytes();
        let decoded = AppMessage::decode(&mut encoded.as_ref()).unwrap();
        prop_assert_eq!(msg, decoded);
    }
}
```

**Ground truth criteria**:
- Every message type round-trips perfectly
- Edge cases: empty content, max lengths, all unicode planes
- Invalid input rejects gracefully

## Layer 3: Reference Tests (Parser)

**Location**: `crates/ultty/tests/ref/`

**Pattern** (Alacritty-inspired):
```
tests/ref/
├── cursor_up/
│   ├── input.bin         # Raw escape bytes: 1b 5b 35 41
│   └── expected.json     # {"actions": [{"action": "CursorUp", "params": 5}]}
├── sgr_colors/
│   ├── input.bin
│   └── expected.json
└── ...
```

**Macro-driven test generation**:
```rust
macro_rules! ref_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_ref_test(stringify!($name));
            }
        )*
    };
}

ref_tests! {
    cursor_up, cursor_down, cursor_position,
    erase_display, erase_line,
    sgr_bold, sgr_colors, sgr_reset,
    scroll_up, scroll_down,
    // ... 100+ test cases
}
```

**Ground truth sources**:
- VT100 spec compliance
- Compare against xterm/Alacritty behavior
- vttest output capture

## Layer 4: Snapshot Tests (Widget Rendering)

**Location**: `crates/ulttui/tests/snapshots/`

**Framework**: Insta

**Pattern**:
```rust
use insta::assert_snapshot;

#[test]
fn button_renders_correctly() {
    let button = Button::new("Submit");
    let output = render_to_string(&button, 20, 3);
    assert_snapshot!(output);
}
```

**Generated snapshots** (`*.snap` files):
```
---
source: crates/ulttui/tests/widget_snapshots.rs
expression: output
---
┌──────────────────┐
│     [Submit]     │
└──────────────────┘
```

**Ground truth criteria**:
- Exact ANSI sequence matching
- Layout pixel-perfect at given dimensions
- Style attributes preserved

## Layer 5: Integration Tests

**Location**: each crate's own `tests/` directory, for example
`crates/ultty/tests/native_protocol.rs`, `crates/ultty/tests/app_compat.rs`,
and `crates/ulttui/tests/e2e_ultty.rs`.

**Pattern**: Full app lifecycle
```rust
#[tokio::test]
async fn app_sends_frame_to_terminal() {
    let (app_tx, term_rx) = channel();

    // Simulate app sending layout
    app_tx.send(AppMessage::BeginFrame { frame_id: 1 }).await;
    app_tx.send(AppMessage::CreateRegion { ... }).await;
    app_tx.send(AppMessage::EndFrame).await;

    // Verify terminal receives and processes
    let response = term_rx.recv().await;
    assert!(matches!(response, TerminalMessage::FrameAck { .. }));
}
```

## Layer 6: Screenshot Tests (Visual Regression)

**Location**: `scripts/*-smoke.sh`, driven against a running Ultty instance.

There is no golden-image comparison suite. The screenshot gates capture images
from a live window through `ulttyctl` and assert structural properties with the
`scripts/analyze-*.py` helpers, rather than diffing against committed PNGs.

```bash
cargo run -p ultty --bin ultty
scripts/live-smoke-gate.sh out/live-smoke-gate      # runs the gates below in order
scripts/screenshot-smoke.sh out/screenshot
scripts/terminal-surface-smoke.sh out/terminal-surface
scripts/native-richtext-smoke.sh out/native-richtext
scripts/append-content-smoke.sh out/append-content
```

These need a GUI session. [Visual Regression](visual-regression.md) documents
the canonical commands and what each artifact means.
[Live Smoke Automation](live-smoke-automation.md) covers running them on the
self-hosted runner and the rules for citing their output as release evidence.

**Not implemented**: pixel-exact golden-image comparison. Adopting it needs a
deterministic headless renderer path first, otherwise font and compositor
differences make every run a false failure.

## Layer 7: Performance Benchmarks

**Location**: `benches/`

**Framework**: Criterion

**Benchmarks**:
```rust
// benches/parser.rs
fn parser_throughput(c: &mut Criterion) {
    let input = include_bytes!("../testdata/vttest_output.bin");

    c.bench_function("parse_1mb", |b| {
        b.iter(|| {
            let mut parser = Parser::new();
            parser.advance(black_box(input));
        })
    });
}

// benches/layout.rs
fn layout_recompute(c: &mut Criterion) {
    let tree = build_complex_tree(1000);  // 1000 regions

    c.bench_function("layout_1000_regions", |b| {
        b.iter(|| tree.compute_layout())
    });
}

criterion_group!(benches, parser_throughput, layout_recompute);
criterion_main!(benches);
```

**Performance targets**:
| Benchmark | Target | Regression Threshold |
|-----------|--------|---------------------|
| parse_1mb | < 10ms | 20% slower |
| layout_1000 | < 1ms | 20% slower |
| frame_render | < 5ms | 20% slower |

## Implementation Priority

### Phase 1: Fix Foundations
1. **Fix proptest roundtrip.rs** - Resolve trait bound compilation errors
2. **Wire up ANSI parser** - Connect actual parser to reference tests
3. **Expand reference test cases** - Add 50+ vttest-derived cases

### Phase 2: Add Snapshots
4. **Add insta to ulttui** - Snapshot all widget outputs
5. **Capture ANSI sequences** - Verify exact escape code output

### Phase 3: Integration
6. **Protocol integration tests** - App ↔ Terminal message flow
7. **Backend switching tests** - Native vs ANSI fallback

### Phase 4: Visual & Performance
8. **Screenshot infrastructure** - Headless rendering + ImageMagick
9. **Criterion benchmarks** - Parser, layout, render throughput
10. **CI regression gates** - Fail on performance regression

## CI Pipeline

`.gitea/workflows/ci.yml` runs on Gitea Actions (Linux) for every push and
pull request to `main`:

| Job | What it runs |
|-----|--------------|
| `fmt` | `cargo fmt --all -- --check` |
| `security-audit` | `scripts/security-audit.sh` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo test --workspace`, the ignored protocol roundtrip tests, and the snapshot tests |
| `msrv` | `cargo check --workspace --all-targets` on the MSRV toolchain |
| `vt-integrity` | The deep property run of `cargo test -p ultty-vt --test integrity` |
| `fuzz` | Short cargo-fuzz runs of the VT parser, protocol decoders, and in-band splitter |
| `gui-smoke` | `scripts/ci-gui-smoke.sh`: Ultty under Xvfb with software Vulkan, driven by the live smokes |

There is no macOS runner; macOS is only built by the GitHub release workflow.
Benchmarks are not in this pipeline. The fuller screenshot gates also run
through `.github/workflows/live-smoke.yml` on manual dispatch. Benchmarks are run by hand and recorded under
`docs/perf/`.

## Test Data Management

### Reference test data sources:
1. **vttest** - Standard VT100 compliance suite
2. **xterm** - De facto terminal behavior
3. **Alacritty ref tests** - Borrowed test cases
4. **Manual capture** - Edge cases discovered in development

### Updating golden files:
```bash
# Snapshot tests
cargo insta review

# Reference tests (manual)
# Edit expected.json after verifying behavior
```

## Directory Structure

```
ultty/
├── crates/
│   ├── ultty/
│   │   ├── tests/
│   │   │   ├── ref/              # Reference parser tests
│   │   │   │   ├── cursor_up/
│   │   │   │   ├── sgr_colors/
│   │   │   │   └── ...
│   │   │   └── ref.rs            # Test runner
│   │   └── benches/
│   │       └── parser.rs
│   ├── ulttui/
│   │   ├── tests/
│   │   │   ├── widget_rendering.rs
│   │   │   └── snapshots/        # Insta snapshots
│   │   └── benches/
│   │       └── layout.rs
│   └── ultty-protocol/
│       └── tests/
│           └── roundtrip.rs      # Property tests
└── scripts/                      # Live GUI smoke gates and analyzers
```

There is no workspace-level `tests/` directory. Every integration test lives in
the `tests/` directory of the crate it exercises.
