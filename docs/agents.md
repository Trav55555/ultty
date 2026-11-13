# Drive Ultty from an agent

An AI agent, or any script, can work in Ultty the way you do: open panes,
run commands, wait for them, read what they printed, and show you what it is
doing. Every pane has a stable id, so the agent can use one pane while you
type in another.

There are two ways in:

- **MCP**: `ulttyctl mcp` serves the tools below to an MCP client such as
  Claude Code.
- **`ulttyctl`**: the same operations as shell commands, plus an event
  stream.

Both talk to Ultty's local control socket, which only your user can open,
with a token Ultty writes at start. Nothing leaves your machine.

## Set up

### 1. Shell integration

Knowing when a command finished, and how, needs your shell to mark its
prompts and commands (OSC 133). Ultty ships scripts for bash 4.4+ and zsh.
Add one line at the end of `~/.bashrc` or `~/.zshrc`, after any prompt
theme:

```sh
# install.sh puts them here:
source ~/.local/share/ultty/shell-integration/ultty.zsh   # or ultty.bash
# .deb and .rpm packages: /usr/share/ultty/shell-integration/
# make install: $PREFIX/share/ultty/shell-integration/
# Homebrew: $(brew --prefix)/share/ultty/shell-integration/
```

They work alongside oh-my-zsh, pyenv and similar prompt hooks, and do
nothing visible. Without them, everything below works except
`last_command`, fenced `run_and_wait`, `wait` for `command_finished`, and the `command_*` events. Direct-child jobs work without shell integration.
Other terminals that understand OSC 133 use the same marks; the rest ignore
them. There is no fish script yet.

### 2. Connect the agent

For Claude Code, from a shell inside Ultty:

```sh
claude mcp add ultty -- ulttyctl mcp
```

`ulttyctl --pid <pid>` selects an instance explicitly. Otherwise it uses
`ULTTY_PID`, which Ultty sets for its shells. Either selection fails if that
instance exits or has no control socket; it never selects another instance.
An invalid `ULTTY_PID` also fails unless `--pid` overrides it. When both
selections are absent, the CLI discovers the most recently started instance.

MCP authenticates discovery once, selects the window's random owner epoch and
authentication token, and pins both for every call. It requires control schema 2. Restart MCP explicitly after replacing that
window. Pane IDs are stable within that owner's lifetime; they are not
global identities across terminal restarts.

Global flags precede the command, including MCP: `ulttyctl --pid <pid> mcp`.
When `--pid` selects an owner, `split` uses that owner's focused pane unless
`--pane` explicitly names one; it does not inherit another owner's `ULTTY_PANE`.

## The tools

| Tool | What it does |
|---|---|
| `ultty_list_panes` | Every pane: id, tab, focus, title, foreground program, working directory, size, status |
| `ultty_run_command` | Type shell text and Enter; success means input acceptance |
| `ultty_run_and_wait` | Submit one printable shell line at an observed empty OSC 133 prompt and await its fenced completion |
| `ultty_job_status`, `ultty_wait_job` | Inspect or wait for a direct child using the returned pane ID as its job ID |
| `ultty_wait` | Wait for text to appear, the next command to finish (with its exit code), the bell, or a quiet period |
| `ultty_last_command` | The last command in a pane: command line, output (last 200 lines unless asked), exit code, still running or not |
| `ultty_read_text` | A pane's screen, optionally with scrollback |
| `ultty_regions` | Native regions, exact widget IDs, declared actions and displayed generation |
| `ultty_invoke_action` | Focus or activate an exact native widget; success requires application acknowledgement |
| `ultty_push_override`, `ultty_clear_override` | Reshape a native app by widget id without editing its files (colour, hide, relabel, remap keys; [docs/overrides.md](overrides.md)), and take it back |
| `ultty_type_text`, `ultty_send_key` | Type text, or press a key: `enter`, `up`, or `c` with `modifiers: "ctrl"` |
| `ultty_search` | Find text in a pane's screen and scrollback |
| `ultty_new_tab` | Open a tab, optionally running a program, and return its pane id |
| `ultty_split` | Split a target pane right/down, optionally launching a program with an `args` array, and return its pane id |
| `ultty_program_status` | Complete current OSC 7501 program records and generation |
| `ultty_focus_pane`, `ultty_zoom_pane`, `ultty_swap_panes` | Focus, zoom or swap explicit pane IDs; swaps stay within one tab |
| `ultty_workspace_list`, `ultty_workspace_save` | List or save named layout recipes; loading starts fresh shells |
| `ultty_set_status` | Show what the agent is doing in the tab bar |
| `ultty_screenshot` | The window as a PNG |
| `ultty_info` | Window size, cursor, current tab |

Tools that act on a pane take `pane` and use the focused pane without it.
Set `pane` explicitly for unattended work.

Input success means the bytes were accepted into the target's queue. A full
or closed queue returns an error; success does not establish execution or
completion. A native delivery error may report partial delivery to earlier
recipients, so retrying can duplicate input.

`ultty_run_command` types shell text and Enter into the foreground PTY.
It assumes that program is a shell and does not preserve argument boundaries.
Use `ultty_new_tab` or `ultty_split` with `command` and an `args` array for
direct process launch. Arguments and a working directory require an explicit
command. A successful reply means no setup or exec failure was reported during
the startup receipt; application readiness and completion are separate gates.

### Running a command and reading the result

For a direct process, use structured arguments and wait by job identity:

1. `ultty_new_tab` `{command: "cargo", args: ["test"], cwd: "/path/to/project"}`
   returns its pane ID, for example `4`.
2. `ultty_wait_job` `{job: 4, timeout_ms: 600000}` returns the child outcome,
   including when the child finished before the wait was registered.

The owner retains up to 256 finished or explicitly closed jobs. Jobs do not
retain output or process argument lists. Unknown or evicted IDs fail explicitly;
closing a pane reports `closed` without claiming a process exit code (`ulttyctl`
returns failure for that outcome). A job ID is
valid only within its owner epoch.

For an existing integrated shell, use one fenced operation:

1. `ultty_run_and_wait` `{pane: 4, command: "cargo test", timeout_ms: 600000}`
   submits text and reserves its completion under one PTY lock.
2. `ultty_last_command` `{pane: 4}` reads the shell's captured output.

The submission requires an observed empty OSC 133 prompt, no pending editable
input or queued PTY output, and no active submission. Commands containing control
characters are refused. Completion requires the reserved command's OSC 133 C/D
boundary. Prompt/reset/exit cancels the reservation; a timeout does not cancel the
command. Output remains a bounded live-terminal inspection, not a durable job log.

The older `ultty_wait` with `until: "command_finished"` watches the next future
completion; it does not identify a command. The MCP server handles calls serially,
so a blocking wait prevents a later launch on the same connection. A quiet period
proves only that output stopped. Prefer the identity-based operations above for
launch-and-wait workflows.

A program that prints ConEmu's progress report (`OSC 9;4;1;<percent>`, as
Windows Terminal and Ghostty read it) gets the same progress marker without
the socket. A status set through the socket takes precedence.

### Native widget actions

Read `ultty_regions`, then pass its explicit `pane`, `session`, `shown_generation`
(as `expected_generation`), `region`, `widget_id` and `action` to
`ultty_invoke_action`. Focus works for unique visible focusable widgets. Activation
requires the application to register a handler; a button label alone does not
authorize an action. The first stage supports full-scene native widgets.

A stale, hidden, ambiguous, unsupported or disabled target fails. The app checks
its committed frame again before handling the request. Success means the app
acknowledged dispatch, not that an asynchronous task finished. After a timeout or
connection error, reinspect state before retrying: the action may have applied.
See [Control protocol](CONTROL_PROTOCOL.md#native-semantic-actions).

### Asking for the user

`ultty_set_status` with `state: "waiting"` and `attention: true` puts a
yellow marker on the pane's tab and, when the window is not focused, asks
the window manager to flag it, as the bell does.

## The same from a shell

```sh
job=$(ulttyctl new-tab cargo test)
ulttyctl wait-job "$job" --timeout 600000  # exits with the child's observed status

# In an existing, idle shell with OSC 133 integration:
ulttyctl --pane 4 run-wait --timeout 600000 'cargo test'
ulttyctl --pane 4 last-command --tail 50
```

`ulttyctl --json <command>` prints the full reply as JSON. `--pane` goes
before the command.

### Events

`ulttyctl subscribe` (or `--pane <id> subscribe`) prints one JSON object
per line as things happen, until it is stopped:

```json
{"event":"pane_opened","pane":1,"tab_index":1}
{"event":"command_started","pane":1}
{"event":"command_finished","pane":1,"exit_code":3}
{"event":"bell","pane":1}
{"event":"pane_closed","pane":1}
```

Also `notification`, `title_changed` and `process_exited`. A subscriber
costs nothing while nothing happens; one that stops reading for more than
1 MiB of events is disconnected.

### Owner identity and replay

`ulttyctl describe` reports control schema version 2, the owner's PID and random
epoch, and supported operations. The historical `version` response describes the
build/native protocol and remains separate. Custom workflows should wrap requests
in `with_owner` after authenticated discovery; a mismatch fails before dispatch.

`ulttyctl subscribe-v2` prints an initial response containing the current cursor
and pane metadata/status snapshot, followed by sequenced event records. It includes
`program_status_changed`; query `program-status` for current complete records.
The snapshot and subscription registration share one owner dispatch boundary.
They do not capture an atomic raw grid across every PTY parser.

Resume with `ulttyctl subscribe-v2 --cursor '<cursor JSON>'`. The server keeps at
most 512 events and 1 MiB of history even with no subscribers. A restarted owner,
expired history or future cursor returns `resync_required` and closes the stream;
request a fresh snapshot without a cursor. Replay, filter and checkpoint rules are
specified in [Control protocol](CONTROL_PROTOCOL.md).

## What to know

- **Anything that can run `ulttyctl` as you can drive your terminal,**
  including typing into the shell you are using. An agent with these tools
  has that power. The socket and its token are readable only by your user.
- **Native-mode apps** (built with `ulttui`, running inside Ultty) draw
  through Ultty's protocol, not the text grid, so `read_text` and `wait`
  for text do not see their content yet. Run them in ANSI mode to read them
  as text; see [Test your TUI with ulttyctl](testing-tuis.md).
- **tmux, screen and zellij** show Ultty one pane, whatever they split it
  into, and whether a shell's OSC 133 marks reach Ultty through them
  depends on the multiplexer and its settings.
- **`last_command` after a clear**: once the screen and scrollback holding
  the output are cleared, it says the output is unavailable rather than
  showing something else.
- **Screenshots** capture the whole window.
