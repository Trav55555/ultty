# Historical Rex session smoke evidence

These are selected receipts from the feature checkpoint `6881bc4`, before
integration with `origin/main` at `ba43d2e`. They do **not** verify the merged
source. The [integration ledger](INTEGRATION.md) records completed post-merge
checks, the retained workspace-test failure and successful rerun, completed
integration runtime gates, and remaining gates separately. It does not
upgrade these historical runtime claims.
The date is the local run date, America/Toronto; some logs use November 13 UTC.

The GPU viewer used SHA-256
`a0c363aafc1996c0425d7ad736d171cac7d1d2d5cc566efd25b5705f906e7ae9`.
Individual receipts record the other binary and harness hashes. The remote
host was rebuilt separately, so its result and source-archive hashes take
precedence over assuming every artifact was built from identical bytes.
Its [provenance receipt](devbox-ssh-02/provenance.json) records those inputs.
[files.json](files.json) hashes the copied evidence files.

| Gate | Evidence | What it establishes |
|---|---|---|
| Local persistent viewer | [receipt](viewer-05/receipt.json), [driver](viewer-05/driver.log), [initial image](viewer-05/first.png), [reattached image](viewer-05/reattached.png) | A key plus a 12-key burst reaches count 13; closing or killing the viewer preserves owner, child and native session; observer input is refused; reattachment restores the model. |
| Real devbox SSH | [result](devbox-ssh-02/result.json), [image](devbox-ssh-02/remote-counter.png), [viewer log](devbox-ssh-02/gui.log), [cleanup](devbox-ssh-02/cleanup.log) | Real SSH transport, framed helper replay, retained hidden native state, rendered remote counter, and cleanup of the owned test host. This run used the devbox DNS/public SSH path. Tailscale was absent. |
| Existing App native splits | [log](app-smokes/native-split.log), [image](app-smokes/split.png) | Pane-sized native layout, keyboard input, resize, rendered ink and independent in-band panes. |
| Existing App inline surfaces | [log](app-smokes/inline-surface.log), [placed](app-smokes/inline-placed.png), [scrolled](app-smokes/inline-shifted.png), [siblings](app-smokes/inline-split.png) | Placement, offscreen hiding/restoration, linewise scrolling, copy text and sibling surface survival. |
| Existing App control | [result](app-smokes/control-result.json), [semantic result](app-smokes/semantic-result.json), [log](app-smokes/control.log) | Status, fenced shell commands, retained jobs, subscription replay, layout, bounded input, Rust/Luau action acknowledgements, ANSI parity and an unknown timeout that later applies without retry. |

[The App smoke summary](app-smokes/result.json) and
[binary manifest](app-smokes/binary-manifest.json) identify the frozen binaries.
Absolute paths in receipts identify original local artifacts and are not
required to inspect this package. Large terminal snapshots, raw RPC bodies,
full configurations, credentials and token files are deliberately omitted.

The local viewer used private Xvfb and Mesa software Vulkan. Its graceful
close requests completed in 82 ms and 27 ms. The inline smoke's existing
cleanup kills the GUI and Xvfb concurrently, which produced an X11 teardown
panic after its assertions passed. That smoke establishes its functional
assertions, not graceful shutdown. The separate viewer lifecycle gate covers
graceful close.

RSS and one-second CPU samples in the local viewer receipt characterize one
local run. They do not establish a performance improvement, production sizing,
cross-platform support or network resilience. This package contains no
Tailscale-specific runtime evidence.
