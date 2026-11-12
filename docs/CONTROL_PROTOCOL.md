# Versioned local control

The control socket accepts one authenticated request per connection. JSON clients
send `ControlEnvelope {output_format, request}` followed by a newline; ordinary
requests may instead half-close their write side. Subscribers keep that side open.
Requests are bounded to 1 MiB. Socket permissions, token authentication and owner
identity are separate checks.

## Discovery and owner identity

Authenticated `describe` returns `schema_version: 2`, `owner: {pid, epoch}`, and
`supported_operations`. The epoch is a random 256-bit value created once per
control-server lifetime. It changes even if the OS reuses the PID. The older
`version` operation retains its build/native-protocol response.

Wrap a workflow request as:

```json
{"output_format":"json","request":{"type":"with_token","token":"TOKEN","request":{"type":"with_owner","owner":{"pid":123,"epoch":"EPOCH"},"request":{"type":"list_panes"}}}}
```

Owner mismatch returns error 409 before target resolution, input enqueue, process
creation, or deferred wait registration. Nested batch operations validate the same
wrapper. An explicit `ULTTY_PID` must be positive numeric and available; it never
falls back to another window. Without it, discovery may select the newest window.
`ulttyctl mcp` performs authenticated discovery at startup and pins endpoint, token
and owner epoch for every subsequent call. It fails against older control schemas;
restart the MCP process explicitly to choose another owner. Separate invocations
of the CLI are separate discoveries unless a custom client pins `with_owner`.

## Input and completion

`send_text`, `send_key`, `send_raw` and legacy `run_command` acknowledge queue
acceptance. Missing/closed input returns code 11, full queues code 12, other input
failure code 13, and unsupported input code 4. Native fan-out failure may indicate
partial delivery: retries may duplicate input in earlier recipients.

`run_command` is shell text. Its `args` are joined with spaces without escaping;
it assumes the foreground program is a shell. It does not preserve argv.
`new_tab` and `split_pane` accept an explicit command and argument array. Arguments
or a working directory without a command fail with code 4. Their successful
startup receipt means no setup/exec failure was reported; application readiness
and completion remain separate.

The returned pane ID is also a direct-child job ID, scoped to the owner epoch.
`job_status {job}` returns `running`, `exited {status}` or `closed`.
`wait_job {job, timeout_ms}` waits by identity, including already completed jobs.
The owner retains up to 256 finished/closed job receipts after pane removal;
unknown/evicted jobs return 404. Explicit closure makes no exit-code claim. Output
and argument lists are not retained by this job receipt API.

`run_command_and_wait {command, timeout_ms, target}` is a single-line shell
operation. It atomically reserves a monotonic submission sequence and enqueues
text at an observed empty OSC 133 prompt. It refuses pending input/output, an
active command/submission, control characters and alternate-screen input. Only the
reserved OSC 133 C-to-D boundary yields `shell_command_finished {pane, sequence,
exit_code}`. A later prompt/reset/process exit cancels the reservation (code 14).
Previously queued completion events cannot satisfy the new wait. The timeout is
1..=86400000 ms and does not terminate the command.

Legacy `wait_for` keeps its future-event/text/quiet semantics. It is not a command
receipt. Neither a quiet period nor a stale last-command query proves that a newly
typed command completed. Deferred waits and subscriptions cannot appear in a batch.

## V2 subscriptions

`subscribe` remains the original bare-event NDJSON stream. It does not add the
`program_status_changed` family. `subscribe_v2 {cursor?, target?}` uses JSON for both
the initial response and subsequent records, regardless of envelope output format.

Without a cursor, the initial `subscribed_v2` response includes:

- `cursor: {owner, sequence}`: the owner's event boundary.
- `snapshot`: current pane metadata and owner-processed program-status records.
- `replayed: 0`.

Snapshot construction and subscriber registration occur in one owner dispatch
iteration, with no intervening app mutation. A program-status snapshot uses the
generation last processed by the owner. This contract excludes raw PTY grids and
does not freeze every parser thread. Direct `program_status` queries may return a
newer parser snapshot. Pane metadata is read during control dispatch.

Subsequent lines are `ControlEventRecord {cursor, event}`. The sequence is global
to the owner; pane filters therefore create normal sequence gaps. Status events
carry a generation/reason notification, not a lossless record delta; query the
complete current status. History covers notifications the owner published, not
every intermediate terminal mutation or status report.

With a cursor, the server validates its owner and position, then returns
`subscribed_v2` with `snapshot: null`, `replayed: N`, and the current **replay upper
boundary** cursor. It sends N matching historical records in order before live
records. The client must keep its supplied applied cursor until those N records
have been consumed. The response's cursor is not already-applied state and must
not cause older replay records to be discarded. After the replay is applied, the
client may checkpoint the upper boundary, which includes skipped filtered events;
then advance on live records. A zero-record replay may checkpoint immediately.
Changing a filter requires a fresh snapshot when the new view needs current state.
A resumed pane filter may reference a closed pane so its closing event is replayable.

History retains at most 512 records and 1 MiB of serialized bytes, including
periods with no connected subscribers. Cursor validation returns
`resync_required {cursor, reason}` and closes for `owner_changed`, `history_expired`
or `future_cursor`. The client must explicitly request a fresh snapshot. No replay
or event history survives owner exit.

At most 64 subscribers are open. Each has at most 1 MiB of unsent output; initial
snapshots/replays that exceed the bound fail with 429. A subscriber exceeding its
live backlog is disconnected. Reconnect with its last applied cursor; if history
has expired, resynchronize. Streaming EOF alone does not prove all events were
received. A slow client never blocks the UI owner on socket writes.

## Layout recipes

`workspace_list` lists named saved layouts; `workspace_save {name}` atomically saves
the current recipe. `ultty --workspace NAME` opens it with fresh shells. These are
layout/directory recipes, not retained processes or reconnectable runtime sessions.
`focus_pane`, `zoom_pane` and `swap_panes` use stable explicit pane IDs; swaps are
limited to one tab. These operations remain authenticated and owner-scoped.
