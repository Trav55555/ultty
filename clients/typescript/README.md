# `@ultty/client` (TypeScript)

Zero-dependency TypeScript client, binary v1 wire codec, and Ink-style declarative flex tree renderer for the [Ultty](../../README.md) native terminal protocol.

## Features

- **Binary protocol v1 codec (`src/codec.ts`)** — byte-for-byte compatible with `crates/ultty-protocol`, verified against every vector in [`crates/ultty-protocol/tests/conformance/v1.json`](../../crates/ultty-protocol/tests/conformance/v1.json).
- **Async Unix socket client (`src/client.ts`)** — connects via `ULTTY_SOCKET`, negotiates protocol version and capabilities, batches atomic frames (`beginFrame` / `endFrame`), answers keepalive `Ping`s automatically, and emits terminal events.
- **Ink-style declarative tree (`src/ink.ts`)** — build flex UIs with `Box` and `Text` that render natively inside Ultty and fall back to styled ANSI output in every other terminal (or inside `tmux`/`screen`).

## Usage

Requires Node.js >= 22.6 (native TypeScript execution) or Bun. No build step or `node_modules` install is required.

```bash
# Run v1 wire conformance, roundtrip, socket client, and ANSI fallback tests
npm test

# Run the Ink-style example once (native inside Ultty, ANSI fallback elsewhere)
npm run example
```

```ts
import { Box, Live, Role, Text, render } from "./src/index.ts";

const app = await render(
  Box(
    { flexDirection: "column", padding: 1, borderStyle: "round" },
    Text("Ultty TypeScript", { bold: true, color: "cyan" }),
    Box(
      { overflowY: "scroll", role: Role.Log, live: Live.Polite },
      Text("Native scrolling in Ultty; ANSI fallback everywhere else."),
    ),
  ),
);
```
