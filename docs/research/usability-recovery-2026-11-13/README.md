# Porygon human-trial repairs

The November 13 human trial stopped at `run_argv` permission denial. Typing
`allow commands` retried the call without changing authorization; the native
client displayed `run completed`. At 102 columns the sidebar consumed 34 columns
and prose wrapped through ordinary words.

Work starts from Ultty `6c64033` and Porygon daemon `51375c16`, in isolated
`codex/usability-recovery` branches. No production permission or installation
changes are part of this implementation.

## Contracts under repair

- Missing capability requests must name the exact invocation and offer an
  explicit, version-bound decision. A decision must not create a permanent
  policy binding or resubmit the original prompt.
- Live approvals must return control to the waiting invocation through normal
  authorization. A missing, expired, cancelled, or stale waiter must fail closed.
- Transport completion must not claim that the user's objective succeeded.
- Narrow layouts must preserve useful transcript width; optional status must
  not crowd the composer or split prose unnecessarily.
- Word wrapping must preserve rendered selection bytes, link targets, Unicode
  boundaries, and code spacing.

## Regression evidence

The private Xvfb test `test_usability_native.py` extends the existing native
workspace acceptance fixture. Its changes are the compact `/status/tui` observer,
a larger private display, and independent layout checks/captures at 99, 100,
102, 120, and 160 columns. It checks a minimum transcript width, no hardcoded
`MODE: ACTIVE`, and no disabled weather panel. Existing session, copy, approval,
and draft checks remain in the same test.

Against the installed incumbent, the test failed at 100 columns with a
66-column transcript, below the required 84-column region including four cells
of message spacing. The preceding 99-column check passed. This proves the new
layout gate rejects the human-trial regression. Baseline evidence is under
`/tmp/porygon-usability-baseline` and the command log is
`/tmp/porygon-usability-native-v1/baseline.log`.

## Implemented behavior

The native client advertises `execution_context.approval_protocol =
"versioned-live-v1"`. The paired daemon commit
`c67b591ff6f9369b6c35ce39f0ee22ef6a47578c` creates an expiring, exact-call approval
only for eligible live `run_argv` calls missing a policy binding. Older clients
retain the prior refusal. Approve/deny uses HTTP `expected_version`; the original
executor resumes after the decision and checks authorization again. Approval
creates no permanent binding. Missing waiters, expired decisions, replays and
cancellation fail closed.

The modal puts the actual tool and arguments first. Pending execution remains
pending until its matching tool result arrives. A terminal stream says “turn
ended”; failed and cancelled outcomes remain distinct. Drafts and the original
WebSocket survive approval, with no prompt resend.

The sidebar now requires 118 columns, preserving at least 80 columns of prose.
Status reflects current state; weather and moon information are optional. Prose
wraps at word boundaries while code spacing, Unicode and selection byte offsets
remain preserved by the opt-in Markdown API.

Client implementation commits: `35a7ec7` and `eea306197082cdbeeac7c5c9b5f4272b6f837e13`.

## Final verification

- Native client: 123 tests passed; Clippy with warnings denied and formatting passed.
- Framework: 834 library tests passed. Full workspace: 3,532 passed, 22 ignored; all-targets Clippy passed with warnings denied.
- Daemon: 84 selected security, approval and persistence tests passed; type checking,
  Ruff and commit hooks passed. See the paired daemon research receipt.
- Private GPU layout/history workflow: passed in 44.56 seconds, at 99, 100, 102,
  120 and 160 columns, including draft, history, copy, cancellation and restart checks.
- Private GPU capability workflow: passed in 20.48 seconds. Real HTTP, SQLite,
  production executor/approval dispatch and UI; deterministic provider and tool
  registry. Deny executed nothing; approve invoked once; replay was refused;
  no persistent policy binding was created.

Layout and initial capability graphical candidate SHA-256:
`57761bf059938d2073fe2fd7f8e30d39031f30d15ac2bb543c3b5af92534bb12`.
Screenshots: [exact-command approval](evidence/approval-y.png),
[execution pending](evidence/execution-pending.png),
[102 columns](evidence/width-102.png), [160 columns](evidence/width-160.png).

Compact receipts, region captures and test logs are retained in [evidence](evidence/).
Full local run artifacts are in this worktree's `target/verification-layout-final`
and `target/verification-capability-final` directories.

## Reproduction and limits

Build the excluded client with `cargo build --manifest-path
examples/porygon-tui/Cargo.toml`. Run its tests and Clippy using that same manifest.
Run `cargo test -p ulttui --lib` for the framework. Set `RUSTFLAGS=-Dwarnings`
and use a short writable `TMPDIR` for Unix-socket tests.

For each Python fixture, set `PORYGON_SOURCE_ROOT` to the paired daemon package,
`PORYGON_NATIVE_TEST_BINARY` to the built client, `ULTTY_BINARY` and
`ULTTYCTL_BINARY` to the installed terminal pair, and `PORYGON_ARTIFACT_ROOT`
to a fresh output directory. Use the daemon development Python environment:

```sh
PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 python -m pytest \
  -p pytest_asyncio.plugin -p pytest_timeout -p xdist.plugin -n0 \
  -c "$PORYGON_SOURCE_ROOT/pytest.ini" -q -s \
  docs/research/usability-recovery-2026-11-13/test_capability_native.py
```

Use `test_usability_native.py` for the layout/history workflow. Both fixtures
launch a private Xvfb display and synthetic local daemon; neither touches the
user's live window or requires a model provider.

Initial runs exposed fixture problems: display too small for 160 columns,
checking terminal resize before the client frame, an outdated TaskStore call,
and long Unix-socket paths. These were corrected before final runs; original
failed logs remain under `target/verification-tmp`. A corrected incumbent run
still rejects its 100-column layout.

These checks do not qualify a real provider, real shell execution, production
PostgreSQL, installation or deployment. Live waiters are process-local and do
not survive daemon restart. Cancellation after the final state check is not
atomic with process creation. No production permissions were changed.


The final review added an approve-A/deny-B regression: denying another request
must not clear an earlier accepted invocation when attached history lacks its
local tool row. The test failed before the fix and passes afterward.

Final committed binary SHA-256:
`a0acde32b28eadc1bdbc95731d324ca060a528bcd59b34a24dfbe317e6be05a2`.
The capability GPU workflow passed again on this binary in 5.26 seconds;
see `evidence/final-capability-receipt.json` and `capability-race-final.log`.

Four additional mouse-selection GUI fixtures passed in 50.38 seconds on the
layout candidate (wrapping code unchanged by the final approval fix): soft-wrap
copy, code spaces/tabs, resize/scroll, wide/combining/ZWJ character alignment,
split-pane isolation, and plain/styled/wrapped link targets. Frozen fixtures
are in `selection-fixtures`; the corrected log is in `evidence`. The first run
used a misspelled terminal environment variable and was invalid; its log is
retained at `/home/travers/projects/tui-terminal/target/selection-final`.
The corrected run used the explicit installed terminal pair and four selected
native tests, excluding the separate grid/tmux test.

The private Xvfb captures use a different font environment from the user's
live desktop. These checks establish interaction behavior and layout geometry,
not font appearance or rendering performance parity.
