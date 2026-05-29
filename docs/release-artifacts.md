# Release Artifact Smoke Tests

Use `scripts/release-artifact-smoke.sh` to verify release artifacts without launching the Ultty GUI.

The release workflow runs these checks automatically:

- Tarball contents include `ultty`, `ulttyctl`, `dist/ultty.desktop`, and hicolor icon assets.
- Linux tarball binaries support `ultty --version` and `ulttyctl --version`.
- `.deb` contents include both binaries, the desktop entry, and hicolor icons; extracted binaries report versions.
- `.rpm` contents include both binaries, the desktop entry, and hicolor icons.
- The Homebrew formula test checks both `ultty --version` and `ulttyctl --version`.

Local examples:

```bash
scripts/release-artifact-smoke.sh --tarball path/to/ultty-v0.1.0-x86_64-unknown-linux-gnu.tar.gz --run-binaries
scripts/release-artifact-smoke.sh --deb path/to/ultty_0.1.0_amd64.deb
scripts/release-artifact-smoke.sh --rpm path/to/ultty-0.1.0-1.x86_64.rpm
scripts/release-artifact-smoke.sh --homebrew dist/homebrew/ultty.rb
```
