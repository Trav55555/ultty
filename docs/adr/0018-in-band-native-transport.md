# ADR 0018: In-Band Native Transport and a Primitive-Only Wire Vocabulary

## Status

- Widgets (proposal 2): Accepted 2026-11-05 for `Markdown` and `Code`.
  The terminal's markdown and syntax renderers are removed; the tags still
  decode and draw as verbatim source. Moving `Input`, `Button`, `Progress`
  and `Spinner` into `ulttui` is accepted in direction, one slice at a time.
- Transport (proposal 1): Accepted 2026-11-05 and implemented in Ultty,
  `ulttui`, and (2026-11-06) `ultty-client`, which now owns the shared
  detection and in-band tty code. The benchmark
  ([docs/perf/2026-11-05-in-band-transport.md](../perf/2026-11-05-in-band-transport.md))
  measured +35% bytes and +6 to +14 µs round-trip latency at p50 on real
  app traffic. Every frame crossed a real ssh hop to EC2 intact. The design
  below differs from the proposal in places; see "As built".

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

Multiplexers (measured with tmux 3.6):
- tmux drops bare APC whether passthrough is on or off. It forwards
  DCS-wrapped APC only with `allow-passthrough on`, and only up to 1 MiB.
- It does forward APC written to its input to the active pane.

Ultty cannot know where a tmux pane sits in its tab, so native mode must
stay off under multiplexers. The probe enforces this: apps send it bare
and never DCS-wrap it, so tmux drops it and the app falls back to ANSI.
Frames are chunked into APCs well under 1 MiB (64 KiB), which also bounds
what the parser buffers. This proposal does not make native mode work
under multiplexers; it makes it work over ssh, under sudo, in containers,
and in recordings.

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

## Decisions

1. **Transport.** In-band is adopted. The socket stays and `ulttui` prefers
   it when it runs on the tab that owns it.
2. **Widgets.** `Markdown` and `Code` are deprecated and no longer rendered
   by the terminal ([ADR 0002](0002-native-protocol-semantic-ui.md) is
   amended). The other widgets move to `ulttui` one slice at a time.

## As built

The wire format is as proposed, with each APC string carrying a chunk of
at most 64 KiB of the frame stream ([spec §1.3](../PROTOCOL_SPEC.md)).
Building and running it changed five things.

- **Probe.** There is no separate query frame. The app sends `ClientHello`
  then DA1, and support means a frame arrives before the DA1 reply, so
  non-Ultty terminals are detected without waiting out a timeout. This
  requires the terminal to keep replies in request order. Ultty does that by
  routing every PTY write through one queue on the I/O thread, and by
  holding query replies until the UI has answered the in-band data before
  them.
- **Multiplexers.** tmux takes an APC string as a pane title and screen as
  its hardstatus, so the probe would rename the pane. `ulttui` does not
  probe when `TMUX` or `STY` is set or `TERM` is `tmux*` or `screen*`.
  Ultty no longer passes `TMUX`, `TMUX_PANE`, `STY`, or `WINDOW` to its
  shells: its tabs are not inside that multiplexer.
- **Liveness.** The terminal never pings in-band, because a ping to a dead
  app reaches the shell as typed input. Instead an idle app sends a `Pong`
  every 2 s, and a session ends after 10 s of silence. It also ends at once
  when the app is seen to be gone:
  - `Disconnect` is sent from `Drop`, the panic hook, and an `atexit` hook,
    so `std::process::exit` ends the session too.
  - Ultty checks the PTY's foreground process group, and the processes it
    held at hello. The process check matters without job control (a
    script, `bash -c`, `script`), where the group does not change when the
    app dies.
  - Neither check sees a remote app over ssh. There, a crash without
    `Disconnect` leaves its scene up for up to 10 s.
- **Scope.** A tab hosts one in-band session, only while it has a single
  pane, and only after a well-formed `ClientHello`.
- **Trust.** Anything that writes to the tab can open a session, including
  a remote host or a `cat`ed file. In-band sessions cannot read the
  clipboard.

Known costs:
- A remote app that dies without `Disconnect` leaves its scene up for up to
  10 s. Keys typed meanwhile are sent as in-band strings into the remote
  shell's input, where they show up as junk on the command line.
- A `cat`ed file holding a valid `ClientHello` opens a session that blanks
  the tab until the idle timeout. A remote host could already clear the
  screen with ANSI, so this adds no new capability.

## Follow-ups

- Done 2026-11-06: `ultty-client` gains the in-band transport, and
  `UlttyClient::connect` applies the same `ULTTY_TTY` check as `ulttui`.
- Image-heavy apps keep using the socket when it is available. In-band
  moves a 1 MiB image in about 3 ms.

## Validation

- In a live Ultty tab with no `ULTTY_SOCKET`:
  - `counter-app` ran natively over in-band, and keys reached it.
  - Under `script` it ran natively as well.
  - Inside tmux it fell back to ANSI, with the pane title untouched.
  - After `q` (`process::exit`) and after `kill -9`, the session ended at
    once and the shell came back.
  - A normal tab still used the socket.
- `cargo test --workspace` passes, including:
  - parser, splitter, and event-loop ordering tests;
  - server lifecycle and clipboard tests;
  - probe tests against a fake terminal on a real PTY.
- Fuzzing ran clean: `in_band_split` 7.9M runs, `vt_terminal` 540K runs.
- Over ssh, 2026-11-05. A local Ultty tab ran `ssh -t` to a throwaway
  Ubuntu 24.04 EC2 host (us-east-1, torn down afterwards), with no socket on
  the remote side:
  - The remote `counter-app` ran natively.
  - It stayed up through 15 s of idle on heartbeats alone.
  - Keys reached it.
  - `q` ended the session through `Disconnect`.
  - After a remote `kill -9`, the session ended at the 10 s silence timeout
    (9.9 s measured) and the shell came back.
