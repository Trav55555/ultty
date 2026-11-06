# ADR 0017: Runtime Socket And Control Plane Semantics

## Status

Accepted. Amended 2026-11-05: PTY output no longer goes through the wakeup
bridge. Each terminal's I/O thread parses its PTY and wakes the UI itself
(`ultty::event_loop`); the bridge watches only the native protocol and control
sockets.

## Context

Ultty hosts two Unix-socket surfaces:

- The native protocol socket accepts structured Ulttui applications.
- The control socket accepts local automation requests, such as session listing and wait conditions.

These sockets serve different product goals. The native protocol socket is per terminal process so multiple Ultty windows can run side by side without stealing each other's native app sessions. The control socket is stable so local tools can find the currently running terminal process. Both are Unix-first runtime features.

The UI loop must wake promptly when any runtime socket becomes readable, but it should not spin at frame cadence when there is no terminal, native app, or control-plane work.

## Decision

Ultty uses a per-process native protocol socket path and an active-instance control socket path.

The native protocol server publishes its listener fd and connected session fds as runtime readiness fds. The control server publishes its listener fd and connected client fds the same way. The app wakeup bridge watches PTY, native protocol, and control readiness together, then emits a single runtime-readable event into the UI loop.

The UI loop also keeps a low-frequency maintenance tick for timers and config reloads. This tick is not the primary source of PTY, native protocol, or control-plane responsiveness.

The control socket is local-only. If a control token is configured, every request except the bootstrap-safe path must present the token before side effects run.

## Consequences

- Multiple Ultty windows can host native apps independently.
- Local automation has one stable control target.
- Socket readability wakes the runtime without a 60 Hz polling loop.
- Tests should cover socket fd publication, control auth behavior, protocol frame compatibility, and wait semantics.
- Windows support needs an explicit named-pipe or ConPTY-era replacement for this Unix fd contract.
