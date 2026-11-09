#!/usr/bin/env python3
"""Endless output shaped like an agent CLI session, for the soak (D5).

What Claude Code, Codex and similar CLIs do to a terminal, on the main
screen:

- stream a reply token by token (about 50 tokens a second), as styled
  markdown: headings, bold, inline code, lists, fenced code;
- keep a status box at the bottom (spinner, elapsed time) and redraw it
  every 80 ms inside synchronized output (mode 2026), by moving the cursor
  up over it and erasing it;
- now and then dump a tool's output: a few hundred to a few thousand lines
  at once;
- now and then clear the screen and the scrollback (`/clear`).

    scripts/bench/agent-stream.py [--seed N] [--rate TOKENS_PER_S]

Runs until killed. Deterministic for a given seed.
"""

import argparse
import random
import sys
import time

WORDS = (
    "the terminal renders each region from its constraints so the layout "
    "stays stable while text streams in and the scroll position holds "
    "tests pass build fails retry with the fixed path parser grid buffer "
    "reflow scrollback cursor glyph atlas frame event socket pane tab"
).split()

SPINNER = "|/-\\"
BOX_LINES = 3


def styled_token(rng):
    word = rng.choice(WORDS)
    r = rng.random()
    if r < 0.05:
        return f"\x1b[1m{word}\x1b[22m"
    if r < 0.10:
        return f"\x1b[38;2;230;160;80m`{word}`\x1b[39m"
    if r < 0.12:
        return f"\x1b]8;;https://example.com/{word}\x1b\\{word}\x1b]8;;\x1b\\"
    return word


def status_box(tick, started, cols):
    elapsed = int(time.monotonic() - started)
    inner = cols - 4
    label = f" {SPINNER[tick % 4]} Working... ({elapsed}s, esc to interrupt)"
    return (
        "\x1b[38;5;244m+" + "-" * (cols - 2) + "+\x1b[39m\r\n"
        + "\x1b[38;5;244m|\x1b[39m " + label[:inner].ljust(inner) + " \x1b[38;5;244m|\x1b[39m\r\n"
        + "\x1b[38;5;244m+" + "-" * (cols - 2) + "+\x1b[39m"
    )


class Screen:
    """Appends transcript above a status box it keeps redrawing.

    The cursor stays at the end of the transcript; the box sits on the
    rows below it. Drawing the box first reserves those rows with IND
    (which scrolls if needed and keeps the column), so saving and restoring
    the cursor around the box is safe.
    """

    def __init__(self, out, cols):
        self.out = out
        self.cols = cols
        self.started = time.monotonic()
        self.tick = 0

    def write(self, text):
        """Transcript text: erase the box below, write, draw it again."""
        self.out.write("\x1b[?2026h\x1b[J" + text)
        self._draw_box()

    def redraw_box(self):
        self.tick += 1
        self.out.write("\x1b[?2026h\x1b[J")
        self._draw_box()

    def _draw_box(self):
        reserve = "\x1bD" * BOX_LINES + f"\x1b[{BOX_LINES}A"
        box = status_box(self.tick, self.started, self.cols)
        self.out.write(reserve + "\x1b7\r\n" + box + "\x1b8\x1b[?2026l")
        self.out.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--rate", type=float, default=50.0, help="tokens per second")
    parser.add_argument("--cols", type=int, default=80)
    args = parser.parse_args()
    rng = random.Random(args.seed)
    out = sys.stdout
    screen = Screen(out, args.cols)
    token_gap = 1.0 / args.rate
    next_box = time.monotonic()
    column = 0
    replies = 0

    while True:
        replies += 1
        screen.write(f"\r\n\x1b[1;38;5;75m## Step {replies}\x1b[0m\r\n\r\n")
        column = 0
        for _ in range(rng.randint(80, 600)):
            token = styled_token(rng)
            if column > args.cols - 12:
                token = "\r\n" + ("- " if rng.random() < 0.3 else "") + token
                column = 0
            else:
                token = " " + token
            column += 8
            screen.write(token)
            now = time.monotonic()
            if now >= next_box:
                screen.redraw_box()
                next_box = now + 0.08
            time.sleep(token_gap)
        screen.write("\r\n")

        r = rng.random()
        if r < 0.3:
            # A tool's output: a fenced block of many lines at once.
            lines = rng.randint(200, 3000)
            body = "".join(
                f"\x1b[38;5;108m{n:5}\x1b[39m  {' '.join(rng.choices(WORDS, k=8))}\r\n"
                for n in range(lines)
            )
            screen.write("\x1b[2m```\x1b[22m\r\n" + body + "\x1b[2m```\x1b[22m\r\n")
        elif r < 0.33:
            # /clear: screen and scrollback.
            screen.write("\x1b[H\x1b[2J\x1b[3J")


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
