# Session deployment and recovery verification

The tested implementation is `d7fafef735a231414d7d19b62d47995967033b1d`,
based on merged `153cacc`. The subsequent documentation/evidence commit changes
no implementation. Fresh branch CI is tracked on the pull request.

| Gate | Evidence and result |
| --- | --- |
| Workspace tests | [Suite results and recovery regressions](workspace-results.txt) passed. [Verification metadata](verification.json) records the full log hash and command. |
| Clippy | [All workspace targets, warnings denied](clippy-01.log), passed. |
| Rust 1.90 | [All workspace targets](msrv-01.log), passed. |
| Frozen GUI and host binaries | [Build](binaries-build-01.log) and [hash manifest](gui-build-manifest.json). |
| Actual headless release bundle | The deployment CLI built a clean source revision into a fresh target directory. [Build log](bundle-build-01.log), [manifest](headless-release-manifest.json). |
| GUI-host recovery | [Result and cleanup](gui-host-reconnect/result.json), [inputs](gui-host-reconnect/inputs.json), [token-free relay events](gui-host-reconnect/relay.jsonl.json). Passed both uncertain-mutation cases, offline-input discard, controller arbitration and replacement-owner refusal. |
| Packaged-host recovery | The same GPU gate passed with the actual headless release host and matching-source viewer. [Result and cleanup](release-host-reconnect/result.json), [inputs](release-host-reconnect/inputs.json), [relay events](release-host-reconnect/relay.jsonl.json). |
| User-service lifecycle | A transient systemd user unit used the installed bundle with explicit runtime, ExecStop, Restart=no and KillMode=control-group. Start, snapshot and stop passed; owner, child and private runtime were removed. [Receipt](service-gate-01.json), [exact local driver](service-gate.py). This exercised equivalent unit properties without installing or enabling a permanent service. |
| Bundle tests | [Final tests](deploy-tests-final.log) passed using the built release host, including install, activation, rollback, tampering and lifecycle. |
| Installer | [Current and older release fixtures](install-test-final.log) passed. |
| Release artifact gate | A package-shaped archive of the frozen debug executables passed [contents and version checks](release-artifact-01.log). An archive omitting the host was [refused as expected](release-artifact-missing-host-01.log). This is not a published release or a new Debian/RPM build. |

Both GPU gates used a disposable local SSH relay around the actual
`ultty-session attach-stdio` helper. They do not establish a new real-network
or Tailscale result. Network qualification from PR 135 remains tied to its
previous source. Screenshots show that the native counter survived recovery;
pixel equality before/after disconnection verifies retention. All owned
viewers, hosts, helpers, counter processes, Xvfb servers and private credentials
were removed.

The full release profile's command construction is covered by packaging tests;
the executed fresh release build here uses the headless profile. GUI binaries
use the repository's development profile. No macOS runtime, permanent devbox
installation or host-crash recovery is claimed.

`SHA256SUMS` covers every file in this directory except itself.
