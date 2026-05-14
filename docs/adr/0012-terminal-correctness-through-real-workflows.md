# ADR 0012: Terminal Correctness Through Real Workflows

Ultty proves terminal correctness through real interactive terminal workflows, not parser unit tests alone. Shell editing, job control, full-screen terminal apps, resize, alternate screen behavior, mouse, paste, clipboard, scrollback, search, and common interactive tools should become replayable traces or integration checks when they catch regressions, and native app work must not regress ordinary ANSI terminal behavior.
