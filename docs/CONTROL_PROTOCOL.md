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

If a child starts but the subsequent host layout update fails, the successful
spawn response retains its pane identity and includes an optional `warning`.
Inspect the pane before taking further action; launching again can duplicate the
already running process. JSON and MCP retain the warning; text CLI output writes
it to stderr while keeping the pane ID on stdout.

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

## Native semantic actions

`regions` reports each session's `shown_generation`, regions and metadata.
`ulttui.widget_id` identifies the application widget; `ulttui.actions` lists
supported actions. This identity is separate from native reconciliation keys.
First-stage dispatch supports full-scene `focus` and explicitly registered
`activate`; inline surfaces and value editing are refused.

`invoke_action` requires explicit `target` pane, `session`, `expected_generation`,
`region`, `widget_id` and `action`. It does not infer a recipient from current
focus. The native server checks the committed display generation, unique widget
identity, declared action, visibility, disabled state and owning pane. It sends
the actual last committed application FrameId with its request. The application
checks that frame and its current registry again before handling the action.
The terminal display generation and the application FrameId are distinct fences.

The caller receives `action_applied {session, request_id}` only after the matching
application acknowledgement. Activation dispatch acceptance does not establish
completion of an asynchronous application task. Application refusal returns code
15. A full acknowledgement queue returns code 12. There are at most 128 unresolved
control/accessibility actions globally and 128 per native session; these are
independent of transport byte limits.

A disconnect, flush failure or five-second acknowledgement timeout may leave the
outcome unknown: the application may already have applied the action. The error
says to reinspect before retrying. Cancelling bookkeeping does not undo app work.
Request IDs never repeat within one native session; late or mismatched replies
cannot satisfy a newer request. AccessKit actions use the same dispatcher and
acknowledgement tracking, with outcomes logged rather than sent to a control
client. Keyboard and raw mouse events retain their application-owned behavior.

```sh
# Inspect IDs, shown_generation and declared actions first:
ulttyctl --json --pane 4 regions
ulttyctl invoke-action 4 12 37 8 inc activate
```

MCP exposes `ultty_invoke_action` with the same required identities. It never
substitutes the focused pane for an omitted action target.

## Reading text

`get_text {include_scrollback, scrollback_lines?, target?}` (`ulttyctl text`,
MCP `ultty_read_text`) returns a pane's text as it was written, not as a grid:
rows the terminal soft-wrapped are joined into one line, a wide character
appears once, and each line ends in a newline with its trailing blanks
dropped. A line can therefore span several screen rows, so the output does
not index by screen row. `get_selection` (`ulttyctl selection`) returns the
selected text by the same rules; a block selection keeps one line per row.

## Inline images

`images {target?}` lists the image placements on a pane's active screen,
scrollback included, oldest first: sixel images and Kitty graphics placements
(ADR 0026). It returns no pixel data. Each entry has `protocol` (`kitty` or
`sixel`), the Kitty client's `image` and `placement` ids (0 when there are
none), `line` (the top row's line number, which scrolling does not change),
`row` (its screen row, negative in scrollback), `col`, `cols` and `rows` in
cells, `z`, and the image's pixel `width` and `height`.

```sh
ulttyctl --json --pane 2 images
```

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

## Audit log

Every control socket, a window's and `ultty-session`'s, records who used it
under the tracing target `ultty::audit`, in the [log file](configuration.md#log-file)
(`~/.local/state/ultty/ultty.log`) and on stderr. One connection gives up to
three kinds of line, each with the connection id (`conn`), the socket
(`transport=local` or `transport=session-host`) and the peer's pid:

```text
pid=4242 2026-11-25T10:00:00.000000Z  INFO ultty::audit: control connection conn=101 transport=local peer_pid=5151 peer_uid=1000 peer_gid=1000 peer_exe=/usr/bin/ulttyctl
pid=4242 2026-11-25T10:00:00.000010Z  INFO ultty::audit: control request kind=send_text pane=3 bytes=10 with_token=true conn=101 transport=local peer_pid=5151
pid=4242 2026-11-25T10:00:00.000020Z  INFO ultty::audit: control authentication succeeded conn=101 transport=local peer_pid=5151
```

- **Connection**: the peer's pid, uid and gid from `SO_PEERCRED` and its
  executable from `/proc/<pid>/exe` (Linux; `-` where unknown), written when
  the connection sends its first request. A connection that closes without
  one, such as `ulttyctl` probing for a live instance, is DEBUG.
- **Request**: the operation (`kind`, as in `describe`), the pane, window or
  tab it names, and for requests that carry text (`send_text`, `send_raw`,
  `run_command`, `run_command_and_wait`, `new_tab`, `new_window`,
  `split_pane`, `push_override`, `search`, `wait_for`, `workspace_save`) the
  text's length in `bytes`. Each command of a `batch` gets its own line.
  Unreadable (malformed or oversized) requests are WARN with their size.
- **Authentication**: success, or failure at WARN with
  `reason=wrong_token` or `reason=token_unavailable`.
- **Refusal**: every error reply, with its `code`; 403 at WARN.

Status queries (`ping`, `version`, `describe`, `get_info`, `list_windows`,
`list_tabs`, `list_panes`, `workspace_list`, `program_status`, `images`,
`job_status`, `wait_job`) are DEBUG, and so are the connection and
authentication lines of a connection that asks only those, so a poller does
not fill the log. Everything that changes state or reads what is on screen
(`get_text`, `get_selection`, `search`, `last_command`, `regions`,
`wait_for`, `subscribe`, `screenshot`) is INFO. `RUST_LOG=ultty::audit=debug`
(or `[log] level`) records the rest.

Never recorded: tokens (presented or expected), the text a request types or
sends, commands and their arguments, search and wait patterns, override
sources, titles, working directories, screenshot paths, and the messages of
error replies (some echo what the request named). MCP tool calls arrive
through `ulttyctl mcp` and appear as that process's connections.
