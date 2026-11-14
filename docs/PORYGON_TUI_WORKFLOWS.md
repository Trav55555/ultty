# Porygon TUI Workflows

This note summarizes the workflow commands and runtime visibility added for milestone completion.

## Composer and navigation

The client opens directly into the conversation. The connection indicator and
composer hint explain whether sending is available; a failed or busy send keeps
the draft in the composer.

| Input | Action |
| --- | --- |
| Enter | Send the draft when connected and idle |
| Ctrl+X, then Enter | Insert a newline |
| Shift+Enter | Insert a newline when the terminal reports this chord distinctly |
| Up/Down, Home/End | Move within the draft |
| Ctrl+P | Open the searchable command palette without changing the draft |
| Up/Down, Page Up/Page Down | Select a command while a palette or slash suggestions are visible |
| Tab | Insert the selected command without executing it; otherwise move focus |
| Enter | Run the selected command deliberately, or send the draft when no suggestion is active |
| Ctrl+Space | Insert the selected slash suggestion |
| F1 | Show keys and commands |
| F2 | Find/open conversations; Ctrl+N starts a new conversation |
| F3 | Inspect work and verification results |
| F4 | Inspect approvals and waiting or blocked work |
| Ctrl+Y | Copy the latest nonempty reply as source text |
| Escape | Close the palette/help or dismiss suggestions; preserve the draft |
| Ctrl+C | Stop the active response; when idle, quit |
| Page Up / Page Down | Read earlier or later transcript content |
| Ctrl+Home / Ctrl+End | Read the oldest output / follow the latest output |

With an unsent draft, the first idle Ctrl+C asks for confirmation before exit.
Press Ctrl+C again to quit; editing cancels that confirmation. Drafts are saved
locally. Stopping a response retains the draft. Help and navigation pages keep
these stop and confirmation controls active.

Command search ranks case-insensitive matches in command names, aliases and
descriptions. Slash suggestions appear when the draft begins with a single
command token; arguments remain editable without intercepting Enter. The palette
accepts a bounded pasted query. Escape closes it without changing the draft;
Tab inserts at the cursor and Enter runs the selected command while preserving
the existing draft. File and `@` path completion are not provided by this palette.

The optional sidebar shows daemon-reported values only when available. Budget,
cost and model may say `not reported`. `LOAD/core 1m` is the local host's
one-minute load average divided by its online CPU count, not process CPU usage;
it can exceed 1. These host metrics are implemented for Linux; unavailable
samples currently fall back to zero rather than an unknown indicator. `LOCAL RAM`
is the local host memory-use estimate. Weather and
moon panels appear only with `PORYGON_WEATHER=1`; that opt-in also enables a
third-party weather/geolocation request.

Bracketed paste inserts text as one edit, including newlines, without executing
embedded keys or submitting a message. ANSI terminals must support bracketed
paste; unbracketed input is indistinguishable from typing. ANSI pastes above
1 MiB are discarded through their closing marker. Native text input uses the
same insertion and control-character normalization as the composer.

The composer grows up to eight rows and scrolls with the caret. Scrolling back
holds the transcript position while new output arrives. Ctrl+End follows new
output again. The client caches wrapped rows and renders only the visible rows,
so the transcript is not limited by the generic ANSI scroll canvas. Loaded
history and its cached rows still consume memory in proportion to their size.

## Conversations and recovery

F2 searches the conversation titles already loaded from the daemon. Up/Down
select and Enter opens a conversation without starting a turn. Ctrl+L loads
older pages; the picker displays its search scope. Ctrl+N opens a new local
conversation. `/sessions` opens the same picker, and `/resume <id>` uses the same
read-only history route and switching guards. Resolve active or unconfirmed work
before switching conversations.

History loads through the daemon's paginated message route, up to 50,000 messages
and 32 MiB of response data. The client checks conversation identity, page offsets
and counts before switching; errors preserve the previous conversation. An older
daemon's detail response is accepted only when it contains the complete history.
Offset pagination does not provide a snapshot during concurrent edits; reload
after other clients change the history.

Drafts and reading positions are separate per conversation. The active draft and
any unresolved run identity survive restart, with no automatic resubmission.
After restart, F2 opens the previous history; `/reconnect` inspects a known run.
History restoration is not live stream reattachment.

Draft files are private and scoped by daemon URL and working directory beneath
`$XDG_STATE_HOME/porygon-tui`, or `~/.local/state/porygon-tui`. Edits are cached and
flushed every 250 ms; switching, submission and clean exit force a flush. Abrupt
process loss can lose edits since the last flush. A second client in the same
scope uses in-memory drafts and shows a persistence warning. Unreadable files
are preserved rather than overwritten. `/save`, `/fork` and `/export` remain
explicitly labeled legacy-session commands, separate from conversations.

## Work, attention and review

F3 lists recent runs. Enter opens the selected run's result and verification
reports. Up/Down, Page Up/Down and Home/End navigate long evidence. `r` refreshes
its authoritative state; `a` attaches for inspection without typing an ID. `c`
copies a JSON record containing the run identity, result and returned reports.
Attachment first fetches the run and its complete conversation, then restores
that conversation's draft. A failed fetch or a change in navigation leaves the
previous conversation intact. Attachment never submits the draft.
Reported revision identifiers are shown when present. Completion, passing checks,
acceptance and integration are separate facts; absent evidence remains unknown.
The client does not infer a diff or acceptance decision from a successful run.

F4 shows pending approvals and waiting or blocked runs. Background checks update
the attention indicator without moving focus. Select an approval and press Enter
to inspect its full arguments. Up/Down, Page Up/Down and Home/End scroll; `y`
approves once and `n` denies. Enter does not approve. `r` reconciles an unconfirmed
decision. Escape returns without making a decision. Stream and durable approvals
retain their original identity and use their corresponding daemon routes.

Durable tool decisions include the server-issued version of the inspected
request. Background refresh never substitutes changed arguments into an open
decision: close, refresh Attention and explicitly inspect the replacement.
An uncertain decision stays locked against retries; refresh to reconcile it,
then inspect the run for the outcome. Older daemons without versioned approvals
and non-tool heartbeat approvals are read-only here. Use their originating
workflow or upgrade the daemon before deciding.

## Copy and links

Ctrl+Y copies the most recent nonempty assistant reply, including original
Markdown and whitespace, rather than the rendered wrapping. Review's `c` copies
result/evidence JSON. Native mode sends the existing clipboard protocol message;
ANSI mode emits OSC 52 through the terminal writer. The terminal must permit
clipboard writes. The UI reports that it sent a copy request, not that another
application has read the clipboard. Text over 48,000 UTF-8 bytes is rejected
without truncation.

Transcript links show their destinations. Native scene drag-selection and link
activation are separate terminal capabilities; explicit copy is available for
source text even where grid selection cannot reach native content.

## Memory Workflows

- `/remember <text>`: store a memory
- `/recall <query>`: search memories by query
- `/memory`: browse memory entries (with score/timestamp metadata)

## Project Tree Workflows

- `/tree [path]`: fetch project tree (optionally scoped to a path)
- `/scan [path]`: alias for scoped tree fetch
- `/expand <path>`: fetch subtree for lazy expansion

## Runtime Health and Reconnect

- Sidebar now shows MCP healthy/total server count.
- Sidebar exposes `MCP ERR` and `MODEL ERR` when present.
- MCP status table includes `Last Error` column.
- `/reconnect`: triggers runtime refresh (connection poll, MCP status, model list).

## Performance and Streaming Metrics

- `/perf`: prints stream performance report (TTFT, append latency, cancel-to-visual-stop).
- `/bench-scroll`: runs virtual-scroll benchmark for 1k/5k/10k item scenarios.
- `/bench-diff`: runs 1000-line diff parse/classification benchmark.

Status bar includes stream summary and `cancel_p95` once cancel samples exist.

## Streaming Smoke

Run the fake-daemon streaming smoke from the repository root:

```bash
scripts/porygon-stream-smoke.sh
```

The smoke is a focused `porygon-tui` test path. It starts local fake
WebSocket/HTTP daemons inside the test process, drives the real stream client,
feeds token/completion messages through the app update handlers, and verifies
abort behavior through a fake `/agent/abort` endpoint. It does not require a
real Porygon daemon or external network service.
