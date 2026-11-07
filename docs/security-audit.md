# Security Audit

Run the dependency security audit separately from the normal local build/test loop:

```bash
cargo install cargo-audit --version 0.22.1 --locked
scripts/security-audit.sh
```

The script runs `cargo audit --deny warnings` and ignores only the warning-class advisories listed below. Security vulnerabilities are not ignored.

## Accepted Warning Advisories

| Advisory | Crate | Reason |
|----------|-------|--------|
| `RUSTSEC-2026-0192` | `ttf-parser` | Unmaintained advisory. Reached through `cosmic-text` 0.19 (via `fontdb`) and `winit` 0.30 (via `sctk-adwaita` and `ab_glyph`), both `ttf-parser` 0.25.1. Remove when either dependency moves off `ttf-parser`. |
| `RUSTSEC-2026-0370` | `zvariant` | Unmaintained advisory (2026-11-05): merged into `zbus` 6 as its `wire` module, no patched release. Reached through `notify-rust` 4.18.2 (the latest release) → `zbus` 5. Remove when `notify-rust` moves to `zbus` 6; no release does yet. |
| `RUSTSEC-2026-0371` | `zbus_names` | Same as `RUSTSEC-2026-0370`: merged into `zbus` 6, reached through `notify-rust` → `zbus` 5. |
| `RUSTSEC-2026-0372` | `zvariant_derive` | Same as `RUSTSEC-2026-0370`: merged into `zbus` 6, reached through `notify-rust` → `zbus` 5. |

All of these advisories are held open by upstream dependencies that have no
fixed release yet: `cosmic-text` and `winit` (`ttf-parser`) and `notify-rust`
(`zbus` 5). The renderer and TUI stack is current (wgpu 30, winit 0.30,
cosmic-text 0.19, taffy 0.15, ratatui 0.30), and the `paste`, `lru` and
`rustybuzz` advisories cleared with that upgrade.

Advisories that had semver-compatible fixes were resolved with `cargo update`
on 2026-09-22 rather than ignored: `crossbeam-epoch`, `anyhow`,
`event-listener`, `memmap2`, `rand` (both 0.8 and 0.9 lines, which retired the
former `RUSTSEC-2026-0097` ignore), and `quick-xml` (via `wayland-scanner` and
`notify-rust`, which closed `RUSTSEC-2026-0194` and `RUSTSEC-2026-0195`).
Prefer that path whenever a patched release is inside the parent's range.

On 2026-11-05 three more were resolved by targeted updates that stayed within
the MSRV of the time, 1.85 (it is now 1.89, see below):
- `tokio` 1.51.6 closes `RUSTSEC-2026-0386`, an unsound `AsyncFd::new`.
- `futures-util` 0.3.34 drops `pin-utils`, closing `RUSTSEC-2026-0441`.
- `clipboard-win` moves from a yanked release to 5.4.3.

On 2026-11-06 a full `cargo update` followed the renderer and TUI upgrade (the
MSRV is now 1.89, which `notify-rust` 4.18.2 needs). It cleared
`RUSTSEC-2024-0436` (`paste`), `RUSTSEC-2026-0002` and `RUSTSEC-2026-0253`
(`lru`), and `RUSTSEC-2026-0206` (`rustybuzz`), so those ignores are gone.

`examples/porygon-tui` is excluded from the workspace, so `cargo audit` covers
the workspace lockfile only. The `tokio-tungstenite` path above comes from
`ulttui` itself, not from porygon-tui, so the exclusion does not change this
table. Audit the standalone package separately with
`cargo audit --file examples/porygon-tui/Cargo.lock` when it is released.

## Runtime hardening

The dependency audit above is one part. These are the defences in Ultty's own
code against hostile terminal output, other local processes, and oversized
input, as of the changes listed under Unreleased in
[CHANGELOG.md](../CHANGELOG.md):

| Threat | Defence |
|--------|---------|
| Pasted text closing bracketed paste early (`ESC[201~` in the clipboard) and running as typed input | Bracketed pastes drop every ESC and C1 control (U+0080 to U+009F). |
| Output setting or reading the clipboard (OSC 52) | Reads are refused. Writes can be turned off with `behavior.clipboard_write = "deny"`. OSC 52 is never written to Ultty's own stdout. |
| A link whose text and target differ (OSC 8) starting a local program | Ctrl+click opens only `http`, `https`, `mailto` and `ftp`; `file:` and other schemes are refused and logged. |
| A background process reading keys through an inherited `ULTTY_SOCKET` | Keys reach a socket session only while its process group (from `SO_PEERCRED`, or `LOCAL_PEERPID` on macOS) is the foreground group of the focused pane's PTY, checked on every delivery. |
| Another local user, or a second Ultty, using the control socket | Each process has its own `ultty-ctl-<pid>.sock` (mode 0600) and token in the private `ultty-<uid>` directory, removed on exit. Authentication fails closed. |
| Oversized or split control requests and replies | Requests end at a newline or half-close, up to 1 MiB (413 beyond); malformed ones get a 400; long replies are finished on writability. |
| Unbounded native content (an in-band sender growing memory) | A region keeps at most 4 MiB of text; a session at most 64 MiB, beyond which `SetContent` and `AppendContent` are refused. |
| A `cat`ed file or remote host opening an in-band session | Sessions are not drawn, given input, or answered beyond the handshake until the app confirms them in a later read, and end after 2 s unconfirmed. See ADR 0018. |
| Shells inheriting the terminal's file descriptors (GPU buffers) | The child closes every fd above stderr before `exec`. |
| A window larger than the GPU texture limit aborting the process | The surface is clamped to `max_texture_dimension_2d`. |

These rules are in the code, not in configuration, except `clipboard_write`.

When any ignored advisory becomes directly reachable in Ultty code, remove the ignore and update or replace the dependency instead.
