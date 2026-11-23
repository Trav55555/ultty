# Track C5: Tern Surface Protocol (TSP) Receiver — Go / No-Go Evaluation

Status: **Decision: No-Go (defer)** — 2026-11-23. Plan item C5 of
[docs/agent-tui-plan.md](../agent-tui-plan.md).

## Question

Should Ultty implement a Tern Surface Protocol (TSP) receiver to become the second open
implementation of TSP, and what would it cost?

## Architectural Comparison (Current State)

Ultty's protocol and ecosystem now cover the core primitives that motivated comparing against TSP:

| Capability | TSP (Stencil Labs / Tern) | Ultty (as of 2026-11-23) |
|---|---|---|
| **Transport** | In-band APC (`ESC _ tsp;<verb>;<JSON> ESC \`), 64 KiB chunks, SHA-256 blobs | Binary socket fast path + in-band APC (`ESC _ U<b64> ESC \`, [ADR 0018](../adr/0018-in-band-native-transport.md)) |
| **Scrollback surfaces** | `inline` / `flow` surfaces anchored to a grid row | Inline surfaces anchored to scrollback rows ([ADR 0020](../adr/0020-inline-surfaces.md), Plan B3) |
| **Layout model** | Flex `row`/`col` (`grow`/`shrink`/`basis`, `ch`/`lines`/fractions), no grid | Full Taffy Flexbox + CSS Grid (`crates/ultty/src/scene/layout.rs`) |
| **UI vocabulary** | **74 semantic widget kinds** (`md`, `code`, `diff`, `table`, `chart`, `editor`, `calendar`, `tool`, `agent`, `trace`, `el` HTML element) + CSS stylesheets (`s` verb) | Structural regions + `Text`/`RichText`/core widgets; rich widgets live in `ulttui` ([ADR 0018](../adr/0018-in-band-native-transport.md)) |
| **Accessibility** | Thin (`aria` label prop, Reduce Motion) | Full semantic tree + AT-SPI / AccessKit bridge ([ADR 0021](../adr/0021-accessibility-semantics.md), Plan B4) |
| **Fallback** | Plain text (`tern-sdk` `plain` renderer) | Full ANSI TUI (`ulttui`, `ratatui-ultty`, `@ultty/client` Ink-style ANSI renderer) |
| **Adoption bridges** | Rust/Python/Go/TS SDKs, bundled with `omp` | Rust (`ulttui`, `ultty-client`, `ratatui-ultty` C1), TypeScript (`clients/typescript` C3), Lua overrides |

## Effort Estimate for a Credible TSP Receiver

Building a real TSP receiver inside Ultty is **not** a thin translation layer onto `SceneTree`:

1. **Wire & framing layer (~1–2 weeks):**
   - APC `tsp;` parser in `ultty-vt` / `ultty`, 64 KiB chunk reassembly with 1 s timeout,
     JSON op decoder (`add`/`set`/`text`/`splice` with UTF-16 offset conversion/`move`/`del`/
     `settle`/`focus`/`reveal`/`scroll`/`suspend`/`resume`), credit-based flow control
     (`credits`/`ack`), and SHA-256 content-addressed blob store (`b` verb + `$TERN_BLOB_DIR`).
2. **Surface lifecycle mismatch (~2–3 weeks):**
   - TSP supports three surface modes (`inline`, `screen`, `flow`) with `main` / `dock` / `layer`
     sub-regions, dynamic surface height growth, retention eviction (`gone` + subtree revival),
     and keeping `main` in scrollback on `x` (`keep:true`). Ultty's current inline surfaces
     ([ADR 0020](../adr/0020-inline-surfaces.md)) are fixed-height `rows` blocks; supporting TSP
     `flow` + sticky `dock` + `layer` overlays requires extending `Buffer` anchoring and renderer
     clipping.
3. **74 semantic widget kinds + CSS stylesheet engine (~8–16+ weeks):**
   - TSP pushes rendering responsibility for 74 high-level semantic kinds (`md`, `code`, `diff`,
     `table`, `tree`, `json`, `chart`, `calendar`, `picker`, `editor`, `tool`, `agent`, `trace`,
     `stacktrace`, `el` HTML, plus terminal-clocked `shimmer`/`elapsed`/`rate` animations) and
     program-supplied CSS stylesheets (`s` verb, `data-role`) into the terminal emulator.
   - Implementing even a minimal subset (`kinds` capability filtering lets a receiver advertise
     only the kinds it implements) still requires building a Markdown/code/diff/table/editor/
     stylesheet renderer inside `ultty` — reversing Ultty's explicit architectural decision to
     keep the terminal thin and push widget rendering to client libraries (`ulttui`, `ratatui-ultty`).
4. **Input & editing protocol (~2–3 weeks):**
   - TSP keeps keys on the PTY (kitty keyboard protocol), adds semantic `action`/`select`/`toggle`/
     `change` events, optional terminal-owned `edit`/`undo`/`send` with IME preedit, and UTF-16
     splicing.
5. **Ongoing spec drift (unbounded):**
   - Tern is closed beta and Stencil-controlled; `tern-sdk` had 147 open issues on a 29-commit repo
     in its first month. Chasing a moving, single-vendor 74-kind vocabulary creates continuous
     compatibility churn.

**Total estimated effort:** **13–24+ engineering weeks** for a partial receiver (narrow `kinds`
subset) and **6+ months** for broad `omp` parity, plus ongoing spec-tracking maintenance.

## Decision: **No-Go (Defer)**

Do **not** build a TSP receiver right now.

### Rationale

1. **Architecture conflict:** Ultty's strength is a lean, fast, open binary protocol with full
   Taffy Flexbox/Grid, real accessibility (AT-SPI/AccessKit), a control plane + MCP server for
   agents (`ulttyctl`), and rich ANSI fallbacks. Embedding a 74-widget-kind JSON+CSS browser-lite
   engine inside `ultty` directly undermines Ultty's speed, memory footprint, and simplicity goals.
2. **Low third-party adoption today:** Outside Stencil's own `omp` agent, third-party TSP usage
   is negligible (~1.6k total `tern-sdk` downloads at evaluation time), and Tern itself remains
   closed beta without an open governance process.
3. **Better leverage in Ultty's own Track C:** Shipping `ratatui-ultty` (C1) and the TypeScript
   client (C3) targets far larger existing ecosystems (`ratatui`, Ink/OpenTUI) with a fraction of
   the complexity and zero vendor lock-in.

### Revisit Triggers

Re-evaluate this decision only if **all three** hold:

1. TSP stabilizes under an open, versioned specification (v1.0+) with a public conformance suite
   and stable core widget subset (`<= 15` core kinds usable via `kinds` negotiation).
2. Multiple independent major coding agents (beyond `omp`) adopt TSP as their primary rich output
   protocol.
3. Implementing a narrow `kinds` subset (`col`, `row`, `text`, `md`, `code`, `scroll`, `button`,
   `input`) can map cleanly onto Ultty's existing `SceneTree` and inline surfaces without adding
   a CSS engine to the terminal binary.
