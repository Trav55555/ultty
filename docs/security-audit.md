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
| `RUSTSEC-2026-0097` | `rand` | Warning-only unsound advisory through `tokio-tungstenite` and dev-only `proptest`. Track through upstream dependency updates. |

When any ignored advisory becomes directly reachable in Ultty code, remove the ignore and update or replace the dependency instead.
