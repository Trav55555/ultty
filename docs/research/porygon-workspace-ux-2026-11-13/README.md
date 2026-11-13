# Porygon workspace UX

In progress, 2026-11-13. Continues PR139's composer and transcript changes.
Baseline Ultty `fad7c6b`; daemon main `928514bd`. The original Porygon checkout
contains unrelated uncommitted workflow changes and is not an integration source.

## Delivery and acceptance

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

## Verification plan

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

Local receipts: `/home/travers/projects/porygon-ai/ux-workspace-verification`.
Evidence and final limitations will be recorded after integration.
