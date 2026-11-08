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
`last_command`, `wait` for `command_finished`, and the `command_*` events.
Other terminals that understand OSC 133 use the same marks; the rest ignore
them. There is no fish script yet.

### 2. Connect the agent

For Claude Code, from a shell inside Ultty:

```sh
claude mcp add ultty -- ulttyctl mcp
```

`ulttyctl` talks to the Ultty window it runs in (through `ULTTY_PID`, which
Ultty sets for its shells), or else the most recently started one. So an
agent started in an Ultty pane drives that window.

## The tools

| Tool | What it does |
|---|---|
| `ultty_list_panes` | Every pane: id, tab, focus, title, foreground program, working directory, size, status |
| `ultty_run_command` | Type a command line into a pane's shell and press Enter |
| `ultty_wait` | Wait for text to appear, the next command to finish (with its exit code), the bell, or a quiet period |
| `ultty_last_command` | The last command in a pane: command line, output (last 200 lines unless asked), exit code, still running or not |
| `ultty_read_text` | A pane's screen, optionally with scrollback |
| `ultty_type_text`, `ultty_send_key` | Type text, or press a key: `enter`, `up`, or `c` with `modifiers: "ctrl"` |
| `ultty_search` | Find text in a pane's screen and scrollback |
| `ultty_new_tab` | Open a tab, optionally running a program, and return its pane id |
| `ultty_set_status` | Show what the agent is doing in the tab bar |
| `ultty_screenshot` | The window as a PNG |
| `ultty_info` | Window size, cursor, current tab |

Tools that act on a pane take `pane` and use the focused pane without it.

### Running a command and reading the result

Start the wait before the command, so a fast command cannot finish first:

1. `ultty_new_tab` → `{"tab_index":1,"pane":4}`
2. `ultty_set_status` `{pane: 4, state: "working", label: "running tests"}`
3. `ultty_wait` `{pane: 4, until: "command_finished", timeout_ms: 600000}`,
   and while it waits, `ultty_run_command` `{pane: 4, command: "cargo test"}`
4. `ultty_last_command` `{pane: 4}` → the command line, the end of its
   output, `exit_code`
5. `ultty_set_status` `{pane: 4, state: "done"}`, or `"error"`

An MCP client that sends one call at a time can run the command first and
then wait with `until: "quiet"`, or call `ultty_last_command` and check
`running`.

A program that prints ConEmu's progress report (`OSC 9;4;1;<percent>`, as
Windows Terminal and Ghostty read it) gets the same progress marker without
the socket. A status set through the socket takes precedence.

### Asking for the user

`ultty_set_status` with `state: "waiting"` and `attention: true` puts a
yellow marker on the pane's tab and, when the window is not focused, asks
the window manager to flag it, as the bell does.

## The same from a shell

```sh
pane=$(ulttyctl new-tab)                         # prints the new pane's id
ulttyctl --pane "$pane" wait-for --command-done --timeout 600000 &
ulttyctl --pane "$pane" run cargo test
wait $!                                          # exits with cargo's status
ulttyctl --pane "$pane" last-command --tail 50   # also exits with it
ulttyctl --pane "$pane" status done "tests passed"
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
