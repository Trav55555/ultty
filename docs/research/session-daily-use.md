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

Implementation and final-source results will be recorded here after the gates
complete. Results from PR 135 remain evidence for that earlier source only.
