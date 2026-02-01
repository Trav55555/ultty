# Ultty Project Review Report

Date: 2026-01-31
Reviewer: Senior Rust Terminal Engineer

## Scope

This report reviews the Ultty project with a focus on correctness, reliability,
performance, and alignment with the stated mission: "Build the terminal
emulator + TUI framework that makes Claude Code's architecture obsolete."

## Executive Summary

The vision is strong and the architectural primitives are well chosen (terminal-
side layout, retained scene graph, and atomic frames). However, the current
implementation is still centered on ANSI/PTY execution and does not yet
demonstrate the native protocol path end-to-end. There are also critical
protocol decoding gaps that can lead to framing desynchronization and memory
exhaustion under malformed input. These issues block the protocol from being a
robust primary API.

## Critical Findings

1) Protocol decoding ignores declared payload lengths
   - `AppMessage::decode` and `TerminalMessage::decode` read `_payload_len` but
     never enforce it. This can desynchronize the stream and let extra bytes
     bleed into the next frame.
   - Files:
     - `crates/ultty-protocol/src/codec.rs:1292`
     - `crates/ultty-protocol/src/codec.rs:1452`

2) Unbounded allocation from attacker-controlled lengths
   - `decode_string` and layout region counts allow arbitrary sizes, which can
     cause OOM/DoS from malformed frames.
   - Files:
     - `crates/ultty-protocol/src/codec.rs:60`
     - `crates/ultty-protocol/src/codec.rs:1489`

## High/Medium Findings

3) PTY processing hot-path allocations
   - `process()` allocates and copies on every chunk (`to_vec()`), which is
     unnecessary and will hurt throughput under heavy output.
   - File: `crates/ultty/src/event_loop.rs:205`

4) Unsafe blocks missing `// SAFETY:` comments
   - Project standards require documenting invariants around unsafe blocks.
   - Files:
     - `crates/ultty/src/event_loop.rs:72`
     - `crates/ultty/src/pty/master.rs:106`
     - `crates/ultty/src/pty/master.rs:199`
     - `crates/ultty/src/pty/child.rs:48`
     - `crates/ultty/src/pty/child.rs:230`

5) Child process teardown can block UI path
   - `Drop` for `Child` performs blocking `waitpid` after SIGKILL; this can
     stall the UI on tab close or shutdown.
   - File: `crates/ultty/src/pty/child.rs:197`

## Testing Gaps

- Protocol tests are mostly round-trip. There are no malformed input or
  size-limit tests to verify decoder hardening.
  - File: `crates/ultty-protocol/tests/roundtrip.rs`

## Vision Alignment Review

### Alignment

- Architecture primitives are correct for a modern retained-mode TUI stack:
  terminal-side layout, retained scene graph, atomic frames, and a binary
  protocol.
- The crate split matches product boundaries and encourages clear ownership.
- App-compat testing exists, which is a good sign for terminal-grade reliability.

### Gaps vs Vision

- The native protocol is not yet hardened or integrated end-to-end with a real
  app loop. This prevents validation of performance targets or the atomic frame
  model under load.
- The terminal executable is still effectively an ANSI/PTY-first product; the
  native protocol path is present but not driving real usage.
- The framework (`ulttui`) still looks ANSI-centric, not a retained-mode client
  speaking the native protocol.
- The top-level README still frames the project as a ratatui/crossterm sandbox,
  which undermines the stated mission.

### Overall Assessment

The project is directionally aligned with the mission, but execution remains
early-stage. The core architectural loop (native protocol + terminal-side
layout + retained scene graph + atomic frames) is not yet demonstrated in a
vertical slice. Hardening the protocol and wiring an end-to-end demo are the
highest leverage steps to prove the vision.

## Suggested Next Steps (Prioritized)

1) Enforce payload length and add hard caps (strings, region counts).
2) Add negative tests/fuzz seeds for malformed frames and size limits.
3) Remove PTY hot-path allocations (`to_vec()`); feed slices directly.
4) Add `// SAFETY:` comments for all unsafe blocks and document invariants.
5) Build a minimal native-protocol demo app to validate layout + atomic frames.

