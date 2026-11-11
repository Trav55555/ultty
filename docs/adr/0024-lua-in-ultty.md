# ADR 0024: Luau Tools Opened From Ultty, With Named Grants

## Status

Proposed 2026-11-10. Follows the `ulttui-lua` prototype (`docs/lua.md`,
CHANGELOG "Unreleased"). Direction: malleable software for the terminal,
utility over adoption; tools the user and their agents write and reshape in
minutes.

## Context

A Luau tool is one file. Running it today means a shell, a path to the
`ulttui-lua` binary, and `--allow-exec`, which lets the script run any
program or none. Two things are missing: opening a tool from Ultty itself
(from `ulttyctl`, and later a keybinding) and saying what a tool may do in
terms narrower than everything.

Facts that shape the design:

- `ulttui-lua` already does the hard part: a sandboxed VM, hot reload with
  state kept, the error banner, native drawing over the tab's socket and ANSI
  everywhere else. Ultty does not link Luau.
- `ulttyctl new-tab <cmd> [args]` spawns a program in a new tab's PTY with
  the tab's protocol socket in its environment, so a ulttui app started that
  way draws natively.
- Native scenes are drawn only in a tab with one pane (spec §1.3,
  `render_multi_pane`): a native session in a split tab exists but is not
  drawn. Mapping sessions to panes is the A1 follow-up in
  `docs/agent-tui-plan.md`.
- The only way a script reaches outside the VM is `run{}` (`docs/lua.md`
  "Sandbox"). There is no file or network API yet.

## Decision

### 1. The VM runs in a child process

Ultty opens a Luau tool by spawning `ulttui-lua` in a pane, as any other
program. It speaks the native protocol over the tab's socket, exactly as it
does when started from a shell. Ultty's process never hosts a Lua VM.

Why: a script that hangs, leaks or crashes takes down one pane, not the
terminal; the sandbox, the call budget and the memory cap stay in one crate;
hot reload, the banner and ANSI fallback come for free; `ulttui-lua` keeps
working from a shell, over ssh and under tmux unchanged.

### 2. Launch: `ulttyctl lua open`

    ulttyctl lua open FILE [--split right|down] [--exec CMD]... [--allow-exec] [--log FILE]

Opens `FILE` in a new tab (the default) or in a split next to the caller's
pane, running `ulttui-lua` with the grants given in the caller's directory,
and prints the new pane's id like `new-tab` does. `ulttyctl` resolves the
binary next to its own executable (`target/debug`, an install prefix) and
otherwise leaves the name for the PATH of the Ultty process. `FILE` and
`--log` are made absolute against the caller's directory, which the pane
also starts in; `FILE` must exist.

`--split` uses a new control request, `SplitPane { direction, command, args,
working_dir, target }`, exposed as `ulttyctl split [right|down] [cmd
args...]` and useful on its own. The pane split is `--pane`, else the pane
`ulttyctl` runs in (Ultty sets `ULTTY_PANE` in every pane, next to
`ULTTY_PID`), else the focused one. Until native sessions map to panes, a
tool in a split draws with ANSI: `lua open --split` starts it with
`ulttui-lua --ansi`, and says so once on stderr. A split pane that
connected natively would be blank.

Keybindings and a `[lua]` config table (named tools with their file, grants
and placement) build on the same request later; they are not in this ADR's
slices.

### 3. Grants: named, explicit, deny by default

A script runs with a set of **grants** fixed when its pane opens. Without
any, it can draw, read keys, keep timers and nothing else. The first grant
is `exec`:

| Flag | Meaning |
|---|---|
| `--exec CMD` | `run{}` may start command lines that begin with the words of `CMD` (split on whitespace, quotes keep a word together), compared to `argv` as written: `--exec 'ulttyctl --json panes'` allows that and `ulttyctl --json panes --x`, not `ulttyctl new-tab` nor `/usr/bin/ulttyctl --json panes`. Repeatable. |
| `--allow-exec` | `run{}` may start anything. The old flag, kept. |

A bare program name (`--exec git`) is a one-word prefix: the program with
**any arguments**. For `ulttyctl` (`new-tab sh`, `send-text` into the
user's shell, `lua open --allow-exec`), `git` (`-c alias.x='!sh'`,
`core.pager`), a shell, an interpreter, `env`, `xargs`, `find -exec` and
the like, that is `--allow-exec` by another name. Grant the subcommand
(`'ulttyctl --json panes'`, `'git status'`) and remember that a prefix
grant bounds the words, not what the program does with them: a tool that
needs more than a few fixed command lines wants `--allow-exec` and the
user's eyes.

**Known program runners are refused.** A grant whose first word (by
basename, so `/bin/sh` too) is a shell, `env`, `xargs`, `sudo`, `timeout`,
`ssh`, an interpreter and the like, or `ulttyctl`/`ulttui-lua` with no
subcommand word, is an error at startup naming `--allow-exec`, so allowing
anything is always an explicit choice. The list is one constant,
`ulttui_lua::PROGRAM_RUNNERS` (plus `NEED_SUBCOMMAND`), documented in
`docs/lua.md`. It is a guard against mistakes, not a sandbox: `git` with no
subcommand is as broad and stays allowed, as does any runner not on the
list.

A denied `run{}` is a Lua error at the call, so it shows in the banner with
the file and line and a script can `pcall` it:

    run: git commit is not allowed; open the script with --exec 'git commit' (or a shorter prefix; allowed: 'ulttyctl --json panes')

The grants are fixed when the pane opens: a reload runs the new version with
the same ones, so editing the file does not by itself change what it may
run. It is not a sandbox against the grants themselves; with a grant that
reaches `ulttyctl lua open`, a shell or `git`, a script can open anything.

The user sees the grants where they gave them: on the `lua open` command
line, in a script's own header comment (the convention `examples/lua` uses),
and in the banner when one is missing. When launches come from config or a
keybinding, where the user typed nothing, the pane needs a line of its own
showing them; that comes with those launchers.

Later grants follow the same shape, one flag and one API each: `--read
PATH` with a `read_file` global; network stays denied by having no API.
A declaration in the script header (`-- needs: exec ulttyctl`) that the
launcher checks against the grants, and warns about up front, is a
follow-up once there are two kinds of grant.

### 4. Hot reload

Unchanged: the child watches the file. Opening from Ultty adds nothing and
removes nothing.

### 5. Out of scope

Lua in Ultty's process; a `[lua]` config table and keybindings; `read` and
`net` grants; interactive grant prompts; an MCP tool for `lua open`; an ADR
0020 inline card for a script (it would be `ulttui-lua --inline ROWS` on an
`inline::Console`, a separate slice once a tool wants it); native drawing in
split panes (A1 follow-up).

## Alternatives considered

- **Lua VM inside Ultty.** Saves a process and a socket round trip per
  frame; costs crash and hang isolation, a second sandbox surface in the
  terminal's process, and a copy of reload and banner logic. Rejected.
- **Grants in the script header, trusted by the launcher.** Fastest to use:
  `lua open tool.luau` and it works. But the author of the file grants
  itself, which is not deny by default for a file from elsewhere. Header
  declarations stay as a check against explicit grants, not as the grant.
- **A config allowlist per script path.** Avoids retyping grants for one's
  own tools. Deferred to the `[lua]` config table, which needs the same
  entries anyway.
- **Match `exec` grants on the program's basename or resolved path.** A
  basename match lets `/tmp/x/git` through a `git` grant; a resolved path
  needs the child's PATH at grant time. Exact text is explicit and the error
  tells the user the exact flag to add.
- **Program-only grants.** The first cut; `--exec ulttyctl` turned out to be
  `--allow-exec` through `ulttyctl new-tab sh` or `lua open --allow-exec`.
  Prefixes let the common case (`'ulttyctl --json panes'`) be narrow without
  a grammar for arguments.

## Consequences

- Opening a Luau tool from Ultty is one command; an agent in a pane can open
  a tool for the user with `ulttyctl lua open`.
- `--allow-exec` scripts keep working; scripts that need a few fixed
  command lines state them, and get a precise error otherwise. A grant on a
  program that runs programs is as wide as `--allow-exec`; the known ones
  are refused in favour of that flag, the rest the docs name.
- `ulttyctl split` exists for every program, not only Lua; every pane
  carries `ULTTY_PANE`, so a program can name its own pane to `ulttyctl`.
- A tool in a split draws ANSI until sessions map to panes; the two modes
  already have to agree (ADR 0013), so a tool looks the same apart from
  `align`/`justify`.
- `ScriptOptions::allow_exec` becomes `ScriptOptions::exec: ExecGrant`.

## Slices

1. **Grants in `ulttui-lua`.** `ExecGrant { Deny, Only(prefixes), Any }`,
   `--exec CMD`, the error text above, tests for parsing and enforcement
   (a denied command line is a Lua error the script sees; a granted one
   becomes a `Command::Run`; a prefix matches only from the first word).
2. **`SplitPane`.** Control request with a target pane,
   `ControlData::PaneOpened`, `terminal_manager` split of any pane with a
   command, `ULTTY_PANE`, `ulttyctl split`, auth and JSON round-trip tests.
3. **`ulttyctl lua open`.** Resolution of the binary and the file, the
   caller's directory, grants forwarded, `--split` with `--ansi` and the
   note. Parser tests.
4. **E2E.** `scripts/e2e/lua-open.sh`: a headless Ultty; `lua open
   panes.luau --exec 'ulttyctl --json panes'` draws a list with a row per
   pane in the caller's directory; `lua open panes.luau` without grants
   shows the banner naming the flag; `--pane 0 lua open hello.luau --split
   right` draws in a split pane next to pane 0.
   *Slices 1 to 4 done 2026-11-10.* As built: the first run showed every
   idle socket-mode `ulttui` app, Luau or not, dropped by the terminal 40 s
   after its last frame, because the native backend never answered `Ping`;
   fixed alongside (`ulttui/src/backend/native.rs`). A denied `run{}`
   reached through `pcall(run, ...)` carries no file and line (the caller
   at that stack level is `pcall`); through `pcall(function() ... end)` or
   in the banner it does.
5. **Later, each its own slice:** `[lua]` config and keybindings with the
   grants line in the pane; header `needs:` check; `--read`; inline card.

## Open questions

1. **Whether `lua open --split` should wait for native panes** rather than
   fall back to ANSI. The fallback is honest and useful now; it may teach
   users that splits are second class.
2. **An MCP tool.** `ulttyctl mcp` serves `new_tab`; whether agents want
   `lua_open` as a tool or are fine shelling out.
3. **Grant text.** A tool that pipes through `sh -c` needs `--exec sh`,
   which is `--allow-exec` in effect. Whether to refuse known program
   runners (`sh`, `env`, `ulttyctl` bare) or let the docs say what it means.

   *Resolved 2026-11-11: refused.* `ulttui-lua` rejects such grants at
   startup with an error naming `--allow-exec` (§3, `PROGRAM_RUNNERS`); the
   broad flag is the only spelling of "anything".
