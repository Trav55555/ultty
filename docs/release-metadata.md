# Release Metadata

Ultty uses a dual license expression in workspace metadata:

```toml
license = "MIT OR Apache-2.0"
```

The matching top-level files are:

- [LICENSE](../LICENSE)
- [LICENSE-MIT](../LICENSE-MIT)
- [LICENSE-APACHE](../LICENSE-APACHE)

## Cargo Packages

The workspace-level package metadata owns the shared version, edition, license,
repository, and MSRV. Published crates should inherit those fields:

```toml
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
```

Example applications are workspace members for testing and smoke coverage, but
they are not release packages. Keep `publish = false` on examples so package
metadata scanners and accidental publish commands do not treat demos as public
crates.

## Binaries

The `ultty` package publishes two binaries:

- `ultty`
- `ulttyctl`

Release artifacts must include both binaries, and both must support
`--version`. The release artifact smoke script checks this for tarballs,
Debian packages, RPM packages, and the Homebrew formula:

```bash
scripts/release-artifact-smoke.sh --tarball path/to/ultty.tar.gz --run-binaries
scripts/release-artifact-smoke.sh --deb path/to/ultty.deb
scripts/release-artifact-smoke.sh --rpm path/to/ultty.rpm
scripts/release-artifact-smoke.sh --homebrew dist/homebrew/ultty.rb
```

See [Release Artifact Smokes](release-artifacts.md) for the current artifact
contract.

## Desktop Metadata

Linux desktop packages install:

- `dist/ultty.desktop`
- `dist/icons/hicolor/scalable/apps/ultty.svg`
- `dist/icons/hicolor/*/apps/ultty.png`

When changing icon names, desktop entry IDs, or package destinations, update all
of these together:

- `crates/ultty/Cargo.toml` package metadata for `.deb` and `.rpm`
- `.github/workflows/release.yml`
- `dist/homebrew/ultty.rb`
- `scripts/release-artifact-smoke.sh`
- [Release Artifact Smokes](release-artifacts.md)

## Readiness Impact

License files and release metadata are public-alpha gates. A release candidate
should not be described as public alpha until the metadata can be inspected from
the repository root, Cargo manifests, package contents, and release smoke docs.
