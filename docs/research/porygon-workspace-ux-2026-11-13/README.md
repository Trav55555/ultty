# Porygon workspace UX

Implemented and qualified on 2026-11-13. Continues PR139's composer and transcript changes.
Baseline Ultty `fad7c6b`; daemon main `928514bd`. The original Porygon checkout
contains unrelated uncommitted workflow changes and is not an integration source.

## Delivered behavior

1. Session picker, persistent drafts, saved reading position. Search and open
   conversations without typing IDs. Switching, restart and reconnect must
   preserve the draft and never automatically resubmit a turn.
2. Work and attention. Inspect runs, blockers and decisions through shipped
   daemon routes. Notifications keep keyboard focus; decisions remain explicit.
3. Result review. Read results and verification evidence without clipping,
   identify the run and any supplied revision, and distinguish finished,
   verified, accepted and integrated. Missing provenance remains unknown.
4. Terminal qualification. Exercise native and ANSI input, narrow/wide resize,
   clipboard, links, split panes, tmux/SSH fallback and long histories. Preserve
   unsupported and failed cases. No daily-driver/default promotion from synthetic
   tests alone; human task trials remain a separate observation.

Sol owns session navigation and draft recovery; Terra owns work/attention/review.
A separate Sol verifier owns real-terminal fixtures. Root owns integration and
transcript virtualization, and reviews delegated changes before acceptance.
All implementation agents use isolated branches and worktrees.

## Verification method

The incumbent is the frozen PR139 app, with binary hashes in the prior composer
receipt. Deterministic, provider-free fixtures exercise real daemon routes and
SQLite state. The cheapest falsifier is a session switch or restart losing a
marked multiline draft, or an approval notification changing what Enter does.
The final gate also checks exact submitted message count and decision identity.

Run one parent and one candidate pass per frozen fixture/configuration; preserve
failures, diagnose the cause and name any fixture change before rerunning. Do not
make live provider requests or change the configured client default for these
checks. Synthetic runtime tests establish behavior, not human usability gains.

Long-history target: 10,000 mixed-height items, access to oldest/newest entries,
manual scroll stability during new output, and bounded visible rows. Record
input latency and resident memory against fixed input; no speed claim until
measured. App-level row virtualization replaces the transcript's use of the
4,096-row ANSI scroll canvas without raising the generic allocation limit.

## Results

The frozen final client is Ultty source `897e26e5c10f5b6be48f9829c8f93205634ec7ba`,
binary SHA256 `2413455fc5bcfab75de8fe551084bb3b16c72b0a3c2230e93d27b154ff0d46fb`.
It ran against daemon `b9d524dc364c0df8abd97b3a9c9c40c187016480`.
Later changes to this directory contain evidence only.

| Check | Observed result |
| --- | --- |
| ANSI PTY, 80 and 160 columns | All six final gates passed at each width |
| Native Ultty, Xvfb and software Vulkan | History boundaries, attachment, full approval arguments, exact version-bound denial, multiline paste and source/evidence copying passed; screenshots inspected |
| Incumbent with final daemon and final ANSI fixture | Failed all six workflow gates; failure logs retained |
| Authenticated OpenSSH PTY | 80-to-160 and 160-to-80 resize, no implicit paste submission, exactly one explicit turn passed |
| Native shell capabilities | Real clipboard paste, split isolation, URL dispatch, grid selection and private tmux fallback passed |
| App and shared Rust checks | Final app tests, app/workspace clippy and formatting passed; workspace tests passed on identical shared framework sources |
| Daemon correctness | Targeted tests plus independent real-SQL approval interleavings passed; migration/backfill and conditional claim SQL passed on isolated PostgreSQL 16 |

The first candidate exposed the daemon's oldest-100-message truncation. Its
native screenshots also exposed repeated single-character punctuation in detail
rows. Complete pagination and literal rich-text rows fixed those failures. Review
found that run attachment could overwrite the target draft, and that approval
IDs could be reused or executed concurrently. Staged attachment preserves both
drafts; per-incarnation versions and a server-side atomic claim bind each durable
tool decision to the inspected action. These failures and subsequent receipts
are retained separately.

The independent Sol verifier did not author the implementation. Terra reviewed
the final approval/history integration and ran SSH qualification. Reviews are
targeted source and runtime checks, not a comprehensive security audit.

## Remaining limits

- Native transcript mouse selection failed the recorded probe. Explicit raw
  reply/evidence copying works; terminal-grid selection is a separate passing
  capability. Native scene link activation is not established by the grid URL
  test.
- No human task trial, production-provider run or default-client promotion was
  performed. Automated timings include the driver and are not production latency
  or comparative speed claims.
- SSH covered Linux loopback, not another host or operating system. PostgreSQL
  covered migration and claim SQL, not the full daemon integration suite.
- Loaded history and wrapped-row caches remain proportional to history size.
  The loader caps responses at 50,000 messages and 32 MiB; offset pagination does
  not provide a snapshot against concurrent edits.
- Deploy the daemon update before using durable TUI decisions. Legacy bodyless
  API clients share single-consumer claiming but do not bind a decision to an
  earlier inspection. The TUI requires that binding and keeps unversioned or
  heartbeat approvals read-only. An uncertain consumed action is never retried
  automatically.
- Acceptance, integration and revision provenance are shown only when supplied
  by the daemon; successful completion or passing verification does not imply
  either acceptance or merge.

See [workflow controls and recovery](../../PORYGON_TUI_WORKFLOWS.md). The full
local receipt archive is `/home/travers/projects/porygon-ai/ux-workspace-verification`;
the adjacent evidence directory preserves the portable verification subset.
