# Session deployment and reconnect

This stage starts from merged `153cacc`. It makes the persistent host easier to
install and the viewer usable after a transport interruption.

## Acceptance

- Release archives and package installers include `ultty-session` alongside the
  viewer and control client. The host reports its package version without
  starting a session.
- Versioned deployment bundles identify their source, platform, build role and
  executable hashes. Installation and rollback select complete releases without
  overwriting a running executable or silently restarting an owner.
- A systemd user unit gives the host and SSH helper a consistent runtime
  directory. Starting, stopping and upgrading remain explicit operations.
- A disconnected viewer retains its last complete screen and reports its
  connection state. Retry authenticates the same owner and grant before claiming
  control or sending input.
- Offline input and queued work from the previous connection are discarded.
  An uncertain mutation is never silently replayed or assigned a reused ID.
- Replaced owners, revoked grants and invalid snapshots fail closed. A competing
  controller keeps its lease; observers cannot gain input rights through retry.
- A real viewer/helper gate interrupts transport and verifies recovery, process
  identity, input behavior and cleanup with recorded binary hashes.

## Scope

Terra owns bundle tooling, user-service packaging and installation tests. Sol
owns viewer recovery and its transport regressions. A second Sol lane owns the
GPU reconnect gate. The parent integrates existing release packaging, user docs
and final validation.

This stage does not install a permanent devbox service, enable lingering,
publish a release, migrate processes, or restore applications after host death.
Existing window-owned sessions remain available. Version rollback changes future
launches; changing the running host terminates its applications and requires an
explicit stop/start.

## Verification record

Implementation `d7fafef` passed workspace tests, Clippy with warnings denied,
Rust 1.90 all-target checks, formatting, and package/installer tests. The
headless deployment CLI built a fresh release bundle from the clean committed
source. A transient systemd user unit started that installed host, exposed its
session, then stopped it and reaped its child.

The real GPU recovery gate passed with both the development host and the
packaged headless release host, in 13.36 and 13.76 seconds respectively. It
verified exact retained pixels, dropped offline input, both uncertain-mutation
cases, competing-controller behavior and refusal of a replacement owner before
any claim or snapshot. All owned processes and credentials were removed.
The fixture uses a local SSH relay around the production stdio helper; it does
not constitute another real-network/Tailscale test.

[Receipts, commands, hashes and screenshots](../releases/evidence/2026-11-12/session-daily-use/README.md)
identify each tested artifact. Fresh branch CI is tracked on the pull request.
Results from PR 135 remain evidence for that earlier source only.
