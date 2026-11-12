# Contributing to Ultty

The toolchain, the checks a pull request has to pass, and the documents that
decide arguments.

## Toolchain

The repository pins a toolchain in `rust-toolchain.toml`. Install `rustup` and
the pin installs itself the first time you build.

- Pinned toolchain: **1.98.1**, with `rustfmt` and `clippy`.
- MSRV: **1.90**, declared as `rust-version` in the root `Cargo.toml`. CI has an
  `msrv` job that runs `cargo check --workspace --all-targets` on 1.90. If you
  need a newer language feature, raise the MSRV in `Cargo.toml` and in CI in the
  same pull request, and say why.
- Rust edition: 2021.

Linux builds need Wayland and xkbcommon development headers:

```bash
sudo apt-get install libwayland-dev libxkbcommon-dev
```

## Build, run, test

```bash
cargo build --workspace
cargo run -p ultty                   # the terminal emulator
cargo run -p counter-app             # an example app, run it inside Ultty

cargo test --workspace               # all tests
cargo test -p ultty --lib            # terminal emulator tests only
cargo test -p ulttui --lib           # framework tests only
cargo test -p ulttui --test snapshots
cargo insta review                   # review snapshot diffs
cargo bench -p ultty                 # benchmarks
```

`examples/porygon-tui` is not a workspace member. Build it from its own
manifest:

```bash
cargo build --manifest-path examples/porygon-tui/Cargo.toml
```

## Before you open a pull request

Run all three. CI runs the same commands, and warnings are errors.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`make lint` runs the clippy line. Do not silence a lint to get green; fix the
cause, or explain in the pull request why the lint is wrong here.

Renderer changes also need a GUI smoke. See
[docs/visual-regression.md](docs/visual-regression.md) for the commands and
[docs/live-smoke-automation.md](docs/live-smoke-automation.md) for how to record
the result. Evidence has to be a CI run URL or a committed path under
`docs/releases/evidence/<date>/`. A `/tmp` path is not evidence.

## Commit and pull request style

- Short imperative subject, under about 70 characters, scoped to one area when
  it helps: `ulttui: add Tree widget`. Not `fixed some stuff`.
- Body explains why, not what the diff already shows.
- Small, atomic commits. One logical change each.
- No AI attribution footers, trailers, or generated-by lines in commits, pull
  request bodies, or release notes.

## Authoritative documents

When two documents disagree, this is the order:

| Topic | Authority |
|-------|-----------|
| Coding conventions, build commands | [CLAUDE.md](CLAUDE.md) |
| System design and components | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Wire protocol | [docs/PROTOCOL_SPEC.md](docs/PROTOCOL_SPEC.md), with `crates/ultty-protocol/src/codec.rs` normative where the two disagree |
| Layout behavior | [docs/LAYOUT_MODEL.md](docs/LAYOUT_MODEL.md) |
| Testing | [docs/TESTING_STRATEGY.md](docs/TESTING_STRATEGY.md) |

Anything under [docs/archive/](docs/archive/) is historical and not maintained.

## The two sockets

Ultty listens on two kinds of Unix domain socket. Both are local only; neither
is a network service.

**Native protocol sockets.** Apps built on `ulttui` or `ultty-client` connect
here to send the binary protocol: region creation, content, layout constraints,
and frame boundaries. Every tab listens on its own socket,
`$XDG_RUNTIME_DIR/ultty-<pid>-tab<id>.sock` (falling back to a mode-0700
`ultty-<uid>` directory under `/tmp`), created when the tab opens and unlinked
when it closes. Ultty advertises that tab's path in the `ULTTY_SOCKET`
environment variable of the shell it spawns for the tab, and the tab's PTY in
`ULTTY_TTY`. `ulttui` uses native mode only when its own terminal is
`ULTTY_TTY`: both variables are inherited by tmux, screen, `script` and nested
terminals, which run the app on a different terminal, and drawing through the
socket from there would paint over the tab. `ULTTY_FORCE_NATIVE=1` skips that
check. Because a session is bound to the tab that accepted it, keyboard input,
mouse events and clipboard reads only reach sessions of the visible tab. Apps
that do not qualify fall back to ANSI rendering through ratatui.

**Control socket.** `ulttyctl` connects here to drive a running instance:
`ping`, `info`, sending input, capturing screenshots, waiting for output. It
Every Ultty process (each window) listens on its own socket,
`$XDG_RUNTIME_DIR/ultty-ctl-<pid>.sock` with mode 0600. Without
`XDG_RUNTIME_DIR` it uses a private `ultty-<uid>` directory (mode 0700, owned
by the user, checked at start) under `$TMPDIR` or `/tmp`. `ulttyctl` talks to
the instance named by `--pid`, else by `ULTTY_PID` (set for every shell Ultty
starts), else the most recently started one that is still running. A named
instance that runs but is not listening yet is an error, never a reason to
use another; only a `ULTTY_PID` whose Ultty has exited falls back to the
newest, with a notice on stderr. This is the socket the smoke gates use.

**Cleanup.** Ultty removes its sockets and token when it exits, including on
SIGTERM, SIGHUP and SIGINT, which it handles as a window close (a signal it
inherited as ignored, as under `nohup`, stays ignored). An instance killed with
SIGKILL or one that crashes cannot clean up, so each instance, before it binds,
removes `ultty-<pid>-tab<N>.sock`, `ultty-ctl-<pid>.sock` and
`ultty-auth-<pid>.token` files that are owned by the user, whose pid is not a
running Ultty, and whose socket (for a token, the control socket of that pid)
accepts no connection. `scripts/e2e/runtime-file-cleanup.sh` checks both.

Both sockets accept connections only from peers running as the same uid, and the
control socket additionally requires a token. Ultty writes the token to
`ultty-auth-<pid>.token` next to its socket with owner-only permissions; a client sends
it with `ulttyctl auth <token>` before any other command. Treat that as the
intended behavior when you touch either socket path: the same-uid check and the
token requirement are being landed now, so read the code in
`crates/ultty/src/control/` and `crates/ultty/src/protocol/server.rs` before
assuming what is already enforced.

## Filing issues

Use the GitHub issue tracker. It is the canonical queue.
[docs/ISSUE_BACKLOG_30_60_90.md](docs/ISSUE_BACKLOG_30_60_90.md) holds the
readiness items that are known but not yet filed; check it before writing a new
issue so you do not duplicate one.
