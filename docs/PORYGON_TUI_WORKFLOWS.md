# Porygon TUI Workflows

This note summarizes the workflow commands and runtime visibility added for milestone completion.

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
