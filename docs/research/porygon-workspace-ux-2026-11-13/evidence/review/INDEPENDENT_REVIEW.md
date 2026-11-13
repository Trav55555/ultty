# Independent review verdicts

## Ultty PR 143

PASS. Independently qualified frozen client commit `897e26e5c10f5b6be48f9829c8f93205634ec7ba`, binary SHA256 `2413455fc5bcfab75de8fe551084bb3b16c72b0a3c2230e93d27b154ff0d46fb`, against daemon `b9d524dc364c0df8abd97b3a9c9c40c187016480`. All six real-PTY gates pass at 80 and 160 columns: draft isolation/restart, durable result/raw evidence, older-session pagination, work attach restoring its bound conversation/draft, inspection without a decision followed by one exact version-bound denial, and 10,000-message oldest/latest navigation plus editing. The same final six-gate ANSI80 check rejects the frozen incumbent, 6/6 failures.

Native Xvfb/lavapipe qualification also passes actual multiline system clipboard paste without implicit submission, exact daemon request text, Ctrl+Y raw reply copy, review JSON copy through a fresh shell, work attach, exact approval ID/expected_version, and 10,000-message navigation. I inspected final native pixels: JSON braces/brackets are no longer repeated, evidence paths and approval arguments are readable, and the latest history boundary/draft are visible. Frozen Ultty and ulttyctl SHA256 are `2e6c63cf9b7700af22e0e4c4361aae3e89c7b7760c137a504b34df4134d98fb2` and `c12e003170ee864c33bdd8eee8b29c7be9403c735f9ee5941b87a43451a82f2d`.

Limits: automated provider-free qualification, not a human task trial or deployment. Timing samples include the test driver/polling and do not establish frame latency. Ordinary native-scene mouse selection remains unsupported in the recorded probe; explicit raw copy passes. Separate ancillary checks cover terminal-grid copy/spacing, URL dispatch, split output and tmux fallback. SSH evidence is a separate team receipt.

## Daemon PR 699

PASS WITH NOTES. Independently reviewed `928514bd..b9d524dc364c0df8abd97b3a9c9c40c187016480` for authenticated bounded history paging and version-bound single-consumer approval claims. No blocking introduced defect found. The read route retains existing read authorization, limits pages to 200, orders timestamp ties by ID, and counts durable rows. Both approval decisions consume the exact stored version with one SQL DELETE before action; principal/hash checks precede expired-approval consumption. All production pending-row insertion/upsert paths rotate the version.

Independent real-SQLite probes cover replacement between validation and claim, deny winning against paused approval, claimed approval rejecting competing denial, and version-aware requests refusing non-tool scope fallback. Final ANSI/native runtime verifies the exact expected_version body. The recorded PostgreSQL16 check exercises migration32 and claim SQL; it is not full PostgreSQL daemon integration.

Notes: expected_version remains optional for legacy bodyless clients. Their claims are single-consumer but are not bound to a previously viewed version. The claim provides at-most-once consumption, not exactly-once effects across a process crash. OFFSET pages are ordered live reads, not an immutable snapshot; the 10,000-message runtime proof uses a stable fixture. No provider, host deployment or production migration was exercised.
