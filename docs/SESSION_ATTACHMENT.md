# Persistent terminal sessions

`ultty-session` owns terminal processes and native application connections independently of a window. `ultty --attach NAME` opens a GPU viewer. Closing the viewer leaves applications running; restarting the host starts a new owner and does not recover its processes.

This is optional. `ultty` and `ultty -e PROGRAM` continue to own their processes in the window process.

## Local use

Build the host without graphics dependencies on a machine that only runs applications:

```sh
cargo build -p ultty --no-default-features --bin ultty-session
ultty-session host work -e /bin/bash
```

Keep the host running in the foreground or under your existing process supervisor. In another terminal, with the GUI build installed:

```sh
ultty --attach work
ultty --attach work --observe
```

The controller owns application input and terminal dimensions. Its measured cell pixel dimensions also drive PTY pixel-size queries and image reservations. Before attachment the host uses an explicit 8 by 16 pixel cell default. Observers can inspect, select, copy and scroll locally. `Ctrl+Shift+T` requests a vacant controller lease; it never steals one. `Alt+1` through `Alt+9` select tabs for a controller. `Ctrl+Shift+C/V` copy and paste through that viewer's clipboard. Shift-drag selects VT text even when an application tracks the mouse. A paste larger than 64 KiB is refused as a whole.

Use `ultty-session stop work` to terminate the host explicitly. Closing its last pane leaves an empty host available for new processes. `ultty-session --help` describes scoped grants, inspection, revocation, diagnostics and the persistent command client.

### Grant, inspect and revoke

On the host machine, create a grant with explicit capabilities and lifetime:

~~~sh
ultty-session grant work --caps read,input,layout --ttl 3600 --out ./work-viewer.token
ultty-session grant work --caps read --ttl 600 --out ./work-observer.token
ultty-session snapshot work --token-file ./work-observer.token
~~~

The grant command prints its grant ID, capabilities, lifetime and file path.
Credential bytes go only to the atomically created mode-0600 file. The output
path must be fresh; existing files and symlinks are refused. Transfer that file
through your existing secure channel. Process spawning is a separate capability
and is omitted from these viewer grants.

~~~sh
ultty-session revoke work 1  # replace 1 with the printed grant_id
ultty-session stop work
~~~

Grant, revoke and stop use the named host's private local administrator
credential. They cannot be delegated through the SSH helper.

### Persistent command client

The command client keeps one connection, owner/grant identity and mutation
watermark. It reads newline-delimited JSON Commands and prints JSON Responses,
starting with connection metadata. A controller lease lasts only while that
connection remains open. To claim and release a vacant lease explicitly:

~~~sh
printf '%s\n' \
  '{"type":"claim_control"}' \
  '{"type":"snapshot","after_sequence":null}' \
  '{"type":"release_control"}' |
  ultty-session client work --token-file ./work-viewer.token
~~~

The --controller option requests the lease during the initial handshake. A
read-only grant cannot claim it. Neither path steals an existing lease. For a
one-shot input operation, with the controller currently disconnected:

~~~sh
ultty-session input work --token-file ./work-viewer.token --pane 0 --text 'hello'
~~~

Input acknowledges queue acceptance. The numeric pane ID comes from a snapshot
and is scoped to that owner. The client does not automatically retry uncertain
mutations. The library exposes the exact pending request for explicit receipt
reconciliation; an evicted receipt remains unknown.

## SSH and Tailscale

The host supervisor and noninteractive SSH helper must use the same
XDG_RUNTIME_DIR on the remote machine. Configure that value consistently in the
supervisor and the remote SSH/user-session environment, for example the existing
per-user runtime directory. The helper does not receive a local runtime path from
the viewer and does not search alternative owners when the selected directory
has no matching host. Real SSH environment/discovery remains a deployment gate.

The remote machine must already have `ultty-session` installed and the named host running. Give the viewer an explicitly created, expiring scoped credential using a secure transfer method, then run:

```sh
ultty --attach work --ssh my-ssh-alias --token-file ./work-viewer.token
```

SSH uses a fixed `ultty-session attach-stdio` helper with noninteractive authentication. Session names and credentials travel in framed stdin, never in the remote shell command. The helper refuses administrator credentials. It opens only the named private Unix socket and does not start a host, install software, forward the local control socket, or enroll a device.

An ordinary SSH alias can use a user-managed Tailscale network. Ultty adds no Tailscale dependency or public listener. Diagnostics describe local tool/configuration state; they do not grant access.

Inspect through the same fixed helper without opening a window:

~~~sh
ultty-session snapshot work --ssh my-ssh-alias --token-file ./work-observer.token
ultty-session diagnostics --ssh my-ssh-alias
ultty-session diagnostics --ssh my-ssh-alias --tailscale
~~~

Diagnostics run read-only SSH configuration/executable checks, and optionally
Tailscale status in JSON, with time/output limits. Missing executables are
reported; nothing is installed or enrolled.

## Authority and reconnect behavior

The host creates a private local administrator credential. Scoped grants select `read`, `input`, `spawn` and `layout` capabilities with an expiry. Input, spawn and layout mutations additionally require the controller lease. Revocation and expiry remove access and release that lease. Administrator grants and termination remain local-only through the helper boundary.

Requests pin the owner epoch and grant identity. Mutations have monotonically increasing identities within a grant. A retained identical retry returns its original receipt; changed payloads with the same identity are refused. An expired or evicted receipt produces an unknown outcome instead of executing the old mutation again. Transport failure does not establish that a submitted mutation failed.

These grants govern attachment requests. They do not change the SSH account's
operating-system privileges or the SSH server's own authentication policy.
Splitting launches a process, so it requires both spawn and layout capabilities,
including when the split uses the default shell.

The viewer retains its last complete screen after connection failure and reports the failure in its title. A snapshot that fails validation or hydration also disconnects the viewer and discards queued input. Reopen it to reconnect. It does not silently replay input or replace a pinned owner with a restarted one. A fresh explicit invocation authenticates the named owner again.

## Local automation inside a persistent host

The host also exposes the existing private `ulttyctl` control endpoint, under
its own PID and the same owner epoch as attachment. A program launched inside
the host inherits `ULTTY_PID`; strict discovery therefore selects that host.
From outside, use the PID shown in `ultty-session snapshot NAME`:

```sh
ULTTY_PID=1234 ulttyctl --json describe
ULTTY_PID=1234 ulttyctl --json panes
ULTTY_PID=1234 ulttyctl mcp
```

Replace 1234 with the reported owner PID. `describe` lists the supported runtime
operations. Job completion, fenced shell submissions, text/event waits,
semantic actions and resumable observation use the same shared rules as the
window mode. Window management and GPU screenshots require a window and are
not advertised by the host.

This endpoint uses a distinct local administrator credential. It is never
forwarded by the SSH attachment helper. The controller lease arbitrates scoped
attachments; it does not restrict local administrative automation or the
operating-system privileges of an SSH account.

## Rendering contract

The host retains parsers, partial escape sequences, both VT screens, process state, native transports and pending native frames. Attachments receive validated semantic snapshots: VT display/history/modes and images, pane/tab identities, status, committed native trees, scroll state and inline anchors. A partial native frame never replaces its previous committed frame.

The viewer hydrates read-only replicas through the same GPU renderer as the local terminal. It owns selection and history scroll position. Selection survives replacement only while its text matches. Native inline surfaces remain noninteractive under their existing protocol contract. Full-scene native pointer targets are generation checked. Keyboard and text input follow the current focused target in the exact foreground session, so application redraws cannot discard a burst of typing. Host clipboard reads are denied and writes discarded. Explicit viewer copy/paste is local to the viewer.

Publication sequences advance when visible owner state changes. Matching snapshot cursors receive `Unchanged`, avoiding repeated hydration of idle screens. This is bounded whole-state replacement, not a delta stream or process checkpoint format.

Native image caches include session identity and committed scene revision. VT image IDs remain local to each pane, so the renderer currently clears and uploads their textures per pane. This preserves image isolation at an upload cost; no comparative rendering-performance improvement is claimed.

Resizing publishes cell and pixel dimensions to each PTY. If a PTY resize fails partway through a multi-pane update, inspect a fresh snapshot before retrying; the operating-system changes are not transactional.

## Bounds and qualification

Wire frames allow at most 1 MiB per request and 16 MiB per response. Snapshots have separate semantic allocation limits, including at most 128 panes and two million expanded VT cells across all panes and both screens. State exceeding those bounds returns a capacity error rather than a truncated successful snapshot.

The transport allows at most 32 clients, 32 MiB queued output per client and 64 MiB globally. Slow clients are disconnected. Partial requests and ordinary RPCs have five-second deadlines; SSH startup has a thirty-second limit. Mutation receipts are bounded by count, bytes and age.

Host crash recovery, process migration, simultaneous typing by multiple controllers, automatic reconnect, browser clients and public listeners are outside this mode. Real SSH/Tailscale deployment and screen-reader behavior require environment-specific qualification; local transport and tree tests do not establish those results.

See [native snapshots](NATIVE_SNAPSHOTS.md), [semantic actions](SEMANTIC_ACTIONS.md), and the [implementation receipts](research/rex-implementation-progress.md).
