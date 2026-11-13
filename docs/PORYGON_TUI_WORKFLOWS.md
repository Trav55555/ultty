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
| Ctrl+Space | Complete the first matching slash command |
| Tab / Shift+Tab | Move focus |
| F1 | Show keys and commands |
| Escape | Close help or dismiss command suggestions |
| Ctrl+C | Stop the active response; when idle, quit |
| Page Up / Page Down | Read earlier or later transcript content |
| Ctrl+End | Follow the latest output again |

With an unsent draft, the first idle Ctrl+C warns before discarding it. Press
Ctrl+C again to confirm; editing the draft cancels that confirmation. Stopping a
response retains the draft. Help keeps these stop and confirmation controls active.

Bracketed paste inserts text as one edit, including newlines, without executing
embedded keys or submitting a message. ANSI terminals must support bracketed
paste; unbracketed input is indistinguishable from typing. ANSI pastes above
1 MiB are discarded through their closing marker. Native text input uses the
same insertion and control-character normalization as the composer.

The composer grows up to eight rows and scrolls with the caret. Scrolling back
holds the transcript position while new output arrives; the status bar offers
Ctrl+End to resume following. This is a readable conversation layout, not a
large-history virtualization implementation: the ANSI scroll canvas retains
its existing 4,096-row limit.

## Session Workflows

- `/sessions`: list sessions
- `/resume <id>`: resume a session
- `/save [name]`: save current session
- `/fork [id]`: fork a session (current session if `id` omitted)
- `/export <id>`: export a session payload

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
