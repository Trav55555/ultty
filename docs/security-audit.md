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
| `RUSTSEC-2024-0436` | `paste` | Warning-only unmaintained advisory from transitive dependencies (`ratatui`, `wgpu`/`metal`). Track through upstream dependency updates. |
| `RUSTSEC-2026-0002` | `lru` | Warning-only unsound advisory through `ratatui`; Ultty does not directly use `lru::IterMut`. Track through upstream `ratatui` updates. |
| `RUSTSEC-2026-0253` | `lru` | Warning-only unsound advisory (panic safety in `LruCache::pop()`) through `ratatui` 0.29; the fixed `lru` 0.18 is outside ratatui's range. Same root as `RUSTSEC-2026-0002`. |
| `RUSTSEC-2026-0192` | `ttf-parser` | Unmaintained advisory. Reached through `cosmic-text` 0.11 (`ttf-parser` 0.20) and `winit` 0.29 via `sctk-adwaita` (`ttf-parser` 0.25). Cleared by the renderer dependency upgrade. |
| `RUSTSEC-2026-0206` | `rustybuzz` | Unmaintained advisory through `cosmic-text` 0.11. Cleared by the renderer dependency upgrade. |
| `RUSTSEC-2026-0370` | `zvariant` | Unmaintained advisory (2026-11-05): merged into `zbus` 6 as its `wire` module, no patched release. Reached through `notify-rust` → `zbus` 5 for desktop notifications. Remove when `notify-rust` moves to `zbus` 6; no release does yet, and the latest needs Rust 1.89. |
| `RUSTSEC-2026-0371` | `zbus_names` | Same as `RUSTSEC-2026-0370`: merged into `zbus` 6, reached through `notify-rust` → `zbus` 5. |
| `RUSTSEC-2026-0372` | `zvariant_derive` | Same as `RUSTSEC-2026-0370`: merged into `zbus` 6, reached through `notify-rust` → `zbus` 5. |

All of these advisories are held open by dependencies: the renderer and
ratatui stack, two or more years behind current releases, and `notify-rust`. Upgrading them is tracked in
[Issue Backlog](ISSUE_BACKLOG_30_60_90.md).

Advisories that had semver-compatible fixes were resolved with `cargo update`
on 2026-09-22 rather than ignored: `crossbeam-epoch`, `anyhow`,
`event-listener`, `memmap2`, `rand` (both 0.8 and 0.9 lines, which retired the
former `RUSTSEC-2026-0097` ignore), and `quick-xml` (via `wayland-scanner` and
`notify-rust`, which closed `RUSTSEC-2026-0194` and `RUSTSEC-2026-0195`).
Prefer that path whenever a patched release is inside the parent's range.

On 2026-11-05 three more were resolved by targeted updates that stay within
the 1.85 MSRV:
- `tokio` 1.51.6 closes `RUSTSEC-2026-0386`, an unsound `AsyncFd::new`.
- `futures-util` 0.3.34 drops `pin-utils`, closing `RUSTSEC-2026-0441`.
- `clipboard-win` moves from a yanked release to 5.4.3.

A full `cargo update` was not used: it pulls `notify-rust` 4.18.2, which
needs Rust 1.89.

`examples/porygon-tui` is excluded from the workspace, so `cargo audit` covers
the workspace lockfile only. The `tokio-tungstenite` path above comes from
`ulttui` itself, not from porygon-tui, so the exclusion does not change this
table. Audit the standalone package separately with
`cargo audit --file examples/porygon-tui/Cargo.lock` when it is released.

When any ignored advisory becomes directly reachable in Ultty code, remove the ignore and update or replace the dependency instead.
