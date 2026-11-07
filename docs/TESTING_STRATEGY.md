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

## Layer 3: Reference and Conformance Tests (Parser and Screen)

Two suites check VT behaviour against expected results written by hand from
xterm's documented behaviour (ctlseqs) and the DEC manuals.

**Parser reference tests**: `crates/ultty/tests/ref/` and `ref.rs`. Each case
is an `input.bin` of raw bytes and an `expected.json` of the parser actions
it must produce. There are 13 cases (cursor movement, erases, scrolling, SGR,
save and restore cursor).

**Conformance suite**: `crates/ultty-vt/tests/conformance.rs`. Each test
feeds bytes to an `ultty_vt::Terminal` and checks the resulting screen,
cursor, modes, or the replies the terminal sends back. About 55 tests cover
tab stops across resize, DECCOLM and mode 40, ED and EL, DECSTBM with IL and
DL at the margins, DECOM, scrollback from scroll regions, DECSTR, DECSC and
DECRC, the alternate screens, wide characters at the margin, LNM, identity
and status reports (DA1, DA2, DSR, XTVERSION), DECRQM, DECRQSS, XTGETTCAP,
mouse mode 1005, sixel placement, colour queries and the palette, SGR
underline styles and colour, private-marker dispatch, and UTF-8 decoding.
A rule that no byte of a control sequence ever reaches the screen is also a
property test in `crates/ultty-vt/tests/integrity.rs`.

```bash
cargo test -p ultty-vt --test conformance
cargo test -p ultty --test ref
```

**Not done yet**: vttest has not been run against this version, and no vttest
output is captured in the tree. Running vttest by hand and turning each
difference into a conformance case is the next step for this layer.

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

**Location**: `crates/ultty/benches/`

**Framework**: Criterion (the code below is a sketch of the shape)

**Benchmarks**:
```rust
// benches/parser.rs
fn parser_throughput(c: &mut Criterion) {
    let input = generate_terminal_output(1024 * 1024); // synthetic SGR text

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
3. **Run vttest** - Run it by hand and add a conformance case for each
   difference (the conformance suite exists; vttest has not been run)

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
| `porygon-tui` | `cargo check --locked` of `examples/porygon-tui`, which is outside the workspace |
| `vt-integrity` | The deep property run of `cargo test -p ultty-vt --test integrity` |
| `fuzz` | Short cargo-fuzz runs of the VT parser, protocol decoders, and in-band splitter |
| `gui-smoke` | `scripts/ci-gui-smoke.sh`: Ultty under Xvfb with software Vulkan, driven by the live smokes |

There is no macOS runner; macOS is only built by the GitHub release workflow.
Benchmarks are not in this pipeline. The fuller screenshot gates also run
through `.github/workflows/live-smoke.yml` on manual dispatch. Benchmarks are run by hand and recorded under
`docs/perf/`.

## Test Data Management

### Reference test data sources:
1. **xterm** - De facto terminal behavior (ctlseqs), the reference for the
   conformance suite
2. **vttest** - Standard VT100 compliance suite; not yet used
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
