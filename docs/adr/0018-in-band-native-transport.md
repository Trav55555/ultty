# ADR 0018: In-Band Native Transport and a Primitive-Only Wire Vocabulary

## Status

Proposed (2026-11-05). Needs a maintainer decision on the two questions under
"Open decisions" before any implementation starts.

## Context

The native protocol's only transport is a per-tab Unix socket advertised in
`ULTTY_SOCKET` ([ADR 0017](0017-runtime-socket-and-control-plane-semantics.md),
[spec §1.2](../PROTOCOL_SPEC.md)). A socket only reaches programs on the same
machine that inherited the variable, so native mode is unavailable:

- over ssh, mosh, and in containers or VMs started from the tab;
- under `sudo` with its default `env_reset`;
- to anything that records or relays the PTY (`script`, asciinema, `screen`,
  tmux, zellij), which never sees the native output at all.

Until 2026-11-05 an inherited `ULTTY_SOCKET` also made `ulttui` apps inside a
tmux pane draw straight onto the outer Ultty tab. That is fixed: apps now
require their terminal to be the tab's PTY (`ULTTY_TTY`). The fix makes them
fall back to ANSI correctly, but it does not make native mode available
anywhere new.

Every terminal extension that spread (OSC 8 hyperlinks, OSC 52 clipboard,
OSC 133 marks, the kitty keyboard and graphics protocols) travels in-band in
the PTY byte stream. That is what lets them cross ssh and be recorded, and it
leaves multiplexers free to pass them through.

[ADR 0002](0002-native-protocol-semantic-ui.md) made the protocol carry
semantic UI, and the wire format now includes terminal-rendered widgets:
`Markdown`, `Code` (with syntax highlighting), `Input`, `Button`, `Progress`,
`Spinner`, and `Image` (spec §8.4). Each one is behavior every
implementation of the protocol must reproduce, frozen into the wire format.

## Proposal

### 1. Add an in-band transport

Carry the existing frames (`55 54` magic, type, length, payload; spec §3.1)
inside APC strings on the PTY:

    ESC _ U <base64url(frame)> ESC \

- App to terminal: written to the app's stdout like any other output.
  Terminals that do not know APC `U` ignore it, as they do other APC strings.
- Terminal to app: the same APC form written to the PTY input, which the app
  reads on stdin. `ulttui`'s input reader must split these out of keyboard
  input; crossterm does not do this today.
- Negotiation: the app writes a query frame and treats a reply within a short
  timeout as support, the way kitty-graphics clients probe.
- base64 costs about 33% of payload size, which matters only for images. The
  socket stays as a local fast path, used when `ULTTY_TTY` matches.

Multiplexers: tmux forwards app-to-terminal APC only with `allow-passthrough`
and DCS wrapping, and does not forward terminal replies to panes at all. So
native mode stays off under multiplexers, as kitty graphics do. This
proposal does not solve that; it solves ssh, sudo, containers, and
recording.

### 2. Keep the wire vocabulary primitive

Today's widget tags fall into two groups:

- `Markdown` and `Code` are sent by no `ulttui` code. `MarkdownView` already
  renders framework-side and streams through `AppendContent`
  ([ADR 0006](0006-framework-owned-markdown-streaming.md)). Only the
  raw-protocol examples `hello-native` and `split-panes` use them, so the
  terminal's markdown and syntax-highlighting renderers
  (`renderer/native_render/`) serve two demos.
- `Input`, `Button`, `Progress`, `Spinner`, and `Image` are sent by `ulttui`
  widgets.

Deprecate `Markdown` and `Code` in protocol v2 and port the two examples to
`RichText`. Move `Input`, `Button`, `Progress`, and `Spinner` rendering into
`ulttui` as `RichText` plus focus and event metadata, one widget per slice.
Keep `Image` as a primitive. Remove the deprecated tags after one release
that decodes both, as [ADR 0011](0011-pre-1-0-protocol-evolution.md) allows
before 1.0.

## Open decisions

1. **Transport.** Adopt the in-band transport (socket kept as local fast
   path), or keep the socket as the only transport and document native mode
   as local-only.
2. **Widgets.** Supersede the widget part of ADR 0002 (option above), or keep
   terminal-rendered widgets as a deliberate product feature and accept that
   the wire format freezes their behavior. Deprecating only `Markdown` and
   `Code` is a cheap middle step either way.

## Consequences if adopted

- `ultty`: an APC `U` handler in `ultty-vt::ansi` that hands decoded frames to
  the tab's protocol session; sessions gain a PTY-backed writer.
- `ulttui` / `ultty-client`: an in-band writer, a stdin demultiplexer, and the
  probe; the socket path becomes one of two transports.
- Fuzzing: the `vt_terminal` target starts covering protocol frames, since
  they now arrive through the VT parser.
- Docs: spec §1.2 gains the in-band framing; README's native-mode
  limitations shrink to multiplexers.

## Validation

- A `counter-app` session driven over `ssh localhost` renders natively.
- The same app under `script` records frames that replay correctly.
- `cargo test --workspace`, the protocol roundtrip tests, and both fuzz
  targets pass; the widget deprecation has a test that the v1 tags still
  decode during the overlap release.
