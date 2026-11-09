# ADR 0019: Shell Command Marks Live On Rows

## Status

Accepted 2026-11-07.

## Context

Agents and scripts driving Ultty need to ask "what did the last command
print, and how did it exit?" (`ulttyctl last-command`, slice A5 of
`docs/agent-tui-plan.md`). Shells with OSC 133 integration mark where the
prompt and command input start (A, B), where command output starts (C) and
where the command finished, with its exit code (D).

Ultty kept those marks as screen row numbers. A screen row number stops
meaning anything as soon as output scrolls, the scrollback evicts its oldest
lines, the screen is cleared, or a resize re-wraps lines.

## Decision

Each row carries three marks: input start (B), output start (C) and command
end (D). A mark is one byte: 0 for no mark, otherwise the low bits of a
per-terminal command number (1 to 255). Rows and compact scrollback rows
store the three bytes in padding they already had, so no row grows.

- The marks move with their row through scrolling, into and out of
  scrollback, and through insert and delete line.
- Erasing, clearing or recycling a row drops its marks, like its text.
- Re-wrapping gives a logical line the marks of its rows, on the first row
  of its new layout. Output start and command end are placed at column 0
  after a newline, so they always mark the start of a logical line.
- The command number advances at each output start (C). Input start (B)
  carries the number of the command about to run. A command end (D) is
  recorded only while a command is running, so the D a shell sends before
  its first prompt marks nothing.

The last command is found from the newest output-start mark carrying the
current command number. Its output runs from that row to the row carrying
the same command's end mark, or to the cursor while it is still running.
The command line is read at C from the input-start row, from the column B
was sent at, through the rows before the output.

When the marks for the current command are gone (the screen was cleared,
the terminal reset, or the lines left the scrollback), the answer says the
output is unavailable. It never pairs one command's start with another's
end: the command number rules that out for the last 255 commands.

## Consequences

- `last-command` survives scrolling, scrollback eviction of older lines,
  and window resizes.
- Output that does not end in a newline leaves the cursor mid-row when D
  arrives, and the next prompt usually continues on that row. The text
  before the cursor is saved when D arrives and appended to the output, so
  the prompt never is.
- The marks are also the basis for later features: jump to the previous
  prompt, select a command's output.
- The alternate screen's rows can carry marks too, but `last-command` reads
  the main screen, where shells run commands.
