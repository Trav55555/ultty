# Release Artifact Smoke Tests

Use `scripts/release-artifact-smoke.sh` to verify release artifacts without launching the Ultty GUI.

The release workflow runs these checks automatically. The terminfo checks need
the workflow to copy `dist/terminfo` into the tarball and to run
`tic -x -o target/terminfo dist/terminfo/ultty.terminfo` before building the
`.deb` and `.rpm`; until it does, those checks fail.

- Tarball contents include `ultty`, `ulttyctl`, `dist/ultty.desktop`, hicolor icon assets, and `dist/terminfo/ultty.terminfo`.
- Linux tarball binaries support `ultty --version` and `ulttyctl --version`.
- `.deb` contents include both binaries, the desktop entry, hicolor icons, and the compiled `usr/share/terminfo/x/xterm-ultty`; extracted binaries report versions.
- `.rpm` contents include both binaries, the desktop entry, hicolor icons, and `/usr/share/terminfo/x/xterm-ultty`.
- The Homebrew formula compiles `dist/terminfo/ultty.terminfo` into its `share/terminfo`.
- The Homebrew formula test checks both `ultty --version` and `ulttyctl --version`.
- The rendered Homebrew formula has no leftover `VERSION` or `SHA256_` placeholders, declares the release version, and carries three 64-hex-character sha256 values.

Local examples:

```bash
scripts/release-artifact-smoke.sh --tarball path/to/ultty-v0.1.0-x86_64-unknown-linux-gnu.tar.gz --run-binaries
scripts/release-artifact-smoke.sh --deb path/to/ultty_0.1.0_amd64.deb
scripts/release-artifact-smoke.sh --rpm path/to/ultty-0.1.0-1.x86_64.rpm
scripts/release-artifact-smoke.sh --homebrew dist/homebrew/ultty.rb
scripts/release-artifact-smoke.sh --expected-version 0.1.0 --homebrew-rendered path/to/rendered/ultty.rb
```

`--homebrew` checks the template in `dist/homebrew/ultty.rb`, which still holds
placeholders. `--homebrew-rendered` checks a formula produced for a specific
release and rejects any placeholder that survived rendering.

## Checksums and the Homebrew Formula

The release workflow collects every tarball, `.deb`, and `.rpm` into one
directory, writes a `SHA256SUMS` file over all of them, and attaches it to the
GitHub release. Verify a download with:

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

`scripts/render-homebrew-formula.sh` then fills the formula template for the
tag and the workflow attaches the rendered `ultty.rb` as a release asset:

```bash
scripts/render-homebrew-formula.sh \
  --version v0.1.0 \
  --artifact-dir artifacts \
  --out artifacts/ultty.rb
```

## Gitea Release Workflow

`.gitea/workflows/release.yml` runs on Gitea for every pushed `v*` tag, on the
`ultty` runner pool. It tests, builds the Linux x86_64 tarball, `.deb` and
`.rpm` with the smokes above, writes `SHA256SUMS`, and publishes them as the
Gitea release with the tag's `CHANGELOG.md` section as notes. It then
publishes the same files to the GitHub release on `Trav55555/ultty`, which is
where `install.sh` downloads from. It does not build macOS or aarch64 targets,
render the Homebrew formula, or sign build provenance; the GitHub release
workflow still does those.

The GitHub step needs a Gitea Actions secret named `GH_RELEASE_TOKEN` on
`trav/ultty`: a fine-grained GitHub personal access token for the
`Trav55555/ultty` repository only, with Contents read and write. It waits for
the push mirror to deliver the tag and checks that the tag names the commit
that was built. If the GitHub release workflow published the release first,
the step leaves it alone. A draft left by a failed attempt stops the step;
delete the draft and re-run the job.

## Release Targets

v0.1.0 was published with Linux x86_64 builds only, and `install.sh` refuses
every other platform. The workflow still lists macOS and Linux aarch64 targets.
The Homebrew formula template names the macOS tarballs, so render and publish
it only for a release that has them.

## macOS Signing Status

macOS binaries are not signed or notarized. There are no Apple Developer
credentials wired into the release workflow. Gatekeeper quarantines the
downloaded binaries, so users must run
`xattr -d com.apple.quarantine ./ultty ./ulttyctl` or right-click the binary in
Finder and choose Open. Say this in the release notes for any candidate that
advertises macOS artifacts. Signing and notarization are tracked in
[Issue Backlog](ISSUE_BACKLOG_30_60_90.md).
