# Agent console events

`agent-console` reads agent events as JSON Lines: one JSON object per line,
from a file or stdin. Every event has:

- `type`: one of the nine tags below;
- `t`: milliseconds since the session started, which replay uses for pacing.

The reader skips blank lines. It skips an event whose `type` it does not know
with a warning, so producers can add types. A line that is not valid JSON, or
that lacks a field its type requires, is skipped and reported with its line
number. Unknown fields are ignored.

The source of truth is `src/event.rs`.

## `prompt`

The user's prompt; it opens the next turn.

```json
{"t":0,"type":"prompt","text":"Fix expand_tabs for wide characters."}
```

## `text_delta`

Markdown appended to the assistant's prose for `turn` (numbered from 1 by
prompt). Deltas concatenate into the turn's text.

```json
{"t":250,"type":"text_delta","turn":1,"text":"I'll start by reading `src/parse.rs`."}
```

## `turn_end`

The assistant finished `turn`.

```json
{"t":9600,"type":"turn_end","turn":1}
```

## `tool_start`

A tool call started. `call` identifies it in later `tool_output` and
`tool_end` events; `title` is a one-line summary of its arguments.

```json
{"t":3000,"type":"tool_start","call":"call-1","tool":"bash","title":"cargo test -p pager"}
```

## `tool_output`

A chunk of a running call's output. `stream` is `stdout` or `stderr`. Chunks
concatenate into the call's output; a chunk may hold several lines.

```json
{"t":3100,"type":"tool_output","call":"call-1","stream":"stdout","text":"running 4 tests\n"}
```

## `tool_end`

A call finished. `status` is `ok` or `error`; `exit_code` is optional and
omitted for tools that run no process.

```json
{"t":4200,"type":"tool_end","call":"call-1","status":"error","exit_code":101,"duration_ms":1100}
```

## `diff`

A change to `path`, as a unified diff.

```json
{"t":5000,"type":"diff","path":"src/parse.rs","diff":"--- a/src/parse.rs\n+++ b/src/parse.rs\n@@ -2 +2 @@\n-    let stop = 4;\n+    let stop = 8;\n"}
```

## `approval`

The agent asks the user to approve an action. `request` identifies the
request; `choices` are the answers in display order.

```json
{"t":5300,"type":"approval","request":"approval-1","summary":"cargo test -p pager","choices":["Allow once","Deny"]}
```

## `status`

A status line update: model, cumulative token counts, what the agent is doing,
and elapsed time as the agent reports it.

```json
{"t":200,"type":"status","model":"example-model-1","input_tokens":921,"output_tokens":0,"state":"thinking","elapsed_ms":200}
```

## Fixtures

`fixtures/` holds three deterministic sessions, written by
`cargo run -p agent-console --bin gen-fixtures`; a test fails if they drift
from the generator (`src/generate.rs`):

- `short.jsonl`: one turn with one of everything;
- `long-tool-output.jsonl`: one call streaming 20,000 lines of log;
- `two-hours.jsonl`: 200 turns over about two hours.

No text in them has trailing whitespace, and they include lines over 80
columns, CJK, emoji and indented code.

Replay one with `cargo run -p agent-console -- replay fixtures/short.jsonl`
(`--fast` to skip the waits, `--speed 10` for ten times real time, `-` for
stdin, `--auto-approve <choice>` to answer approvals without a key). It
renders the console: inline cards inside Ultty, ANSI text anywhere else.
`events` instead of `replay` prints one summary line per event.
