#!/usr/bin/env python3
"""B1's four numbers for the agent console (ADR 0031 section 4).

    scripts/bench/b1-agent-console.py --out DIR --bin DIR --cfg DIR \
        [--targets LIST] [--memory-minutes N] [--memory-warmup N]

Run by scripts/bench/b1-agent-console.sh, which builds the binaries, starts
a private Xvfb (DISPLAY) and writes the terminals' configs; this script
launches the terminals on that display, drives them from outside and
writes DIR/results.json and DIR/report.md.

Targets: ultty-native (the console's inline cards), ultty-ansi (the same
binary with ULTTY_SOCKET unset and ULTTY_IN_BAND=0), alacritty and ghostty
(ANSI mode). Each launch runs a wrapper script in the terminal that reports
`stty size`, draws two calibration cells, waits for a go file, clears the
screen and scrollback and replays fixtures, then sleeps so the screen stays.

Every number is taken the same way in every terminal unless noted:

1. Scroll held. While a fixture streams (short.jsonl at recorded pace, then
   long-tool-output.jsonl after a fast replay of short.jsonl), wheel up 20
   lines (the clicks that takes, from each terminal's lines per click
   measured before the stream), then screenshot the cell grid every 100 ms
   (or as fast as the display allows) until the stream ends. Each shot is aligned with the one taken
   just after the wheel by matching pixel rows; the offset in rows is how
   far the view moved. At the end the view is wheeled to the bottom: if
   nothing changes, the view had been yanked to the bottom.
2. Copy fidelity. After short.jsonl ends, each prose block, code block
   and the diff is selected by dragging the mouse over its rows and the
   selection is read (Ultty: `ulttyctl selection`; others: the X PRIMARY
   selection). It is compared with the text assembled from the fixture's
   events. Rows come from Ultty's own text of the screen (`ulttyctl
   text`), wrapped at the terminal's width; the reference terminals reuse
   Ultty ANSI's rows, since the console writes the same bytes to them.
3. Resize. The window goes through 80, 150 and 100 columns while
   short.jsonl streams and 120, 80, 150, 100 after it ends. The result
   is compared with a fresh run at 100 columns: in Ultty the whole
   transcript (`ulttyctl text --scrollback`, logical lines); in every
   terminal the visible screen at the bottom and at the top of scrollback,
   as screenshots cut into cell rows and matched in order (a diff of rows).
4. Memory. All targets replay two-hours.jsonl at --speed 10 at once, with
   less scrollback (2,000 lines, full about 7 minutes in, inside the
   default 8-minute warm-up); RSS,
   anonymous memory and swap of each terminal process are sampled every
   30 s into a soak.sh-style samples.csv and summarized by
   scripts/bench/soak-report.py.
"""

import argparse
import ctypes
import ctypes.util
import datetime
import difflib
import json
import os
import re
import shlex
import signal
import subprocess
import sys
import time
import unicodedata
from pathlib import Path

from PIL import Image, ImageChops, ImageFilter, ImageGrab

# Test and benchmark runs stay out of the user's Ultty log file.
os.environ.setdefault("ULTTY_LOG_FILE", "0")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
FIXTURES = ROOT / "examples" / "agent-console" / "fixtures"

SCROLL_LINES = 20
SCROLL_ROWS = 30
COPY_ROWS = 100
COLS = 120
RESIZE_DURING = (80, 150, 100)
RESIZE_AFTER = (120, 80, 150, 100)
SAMPLE_S = 0.1


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


# ------------------------------------------------------------ X11 helpers

def xdotool(*args, check=True):
    return subprocess.run(["xdotool", *map(str, args)], check=check,
                          capture_output=True, text=True).stdout


class _SelectionEvent(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong),
                ("send_event", ctypes.c_int), ("display", ctypes.c_void_p),
                ("requestor", ctypes.c_ulong), ("selection", ctypes.c_ulong),
                ("target", ctypes.c_ulong), ("property", ctypes.c_ulong),
                ("time", ctypes.c_ulong)]


class _Event(ctypes.Union):
    _fields_ = [("type", ctypes.c_int), ("xselection", _SelectionEvent),
                ("pad", ctypes.c_long * 24)]


def _xlib():
    x = ctypes.CDLL(ctypes.util.find_library("X11"))
    p, ul = ctypes.c_void_p, ctypes.c_ulong
    x.XOpenDisplay.restype = p
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XDefaultRootWindow.restype = ul
    x.XDefaultRootWindow.argtypes = [p]
    x.XCreateSimpleWindow.restype = ul
    x.XCreateSimpleWindow.argtypes = [p, ul, ctypes.c_int, ctypes.c_int, ctypes.c_uint,
                                      ctypes.c_uint, ctypes.c_uint, ul, ul]
    x.XInternAtom.restype = ul
    x.XInternAtom.argtypes = [p, ctypes.c_char_p, ctypes.c_int]
    x.XConvertSelection.argtypes = [p, ul, ul, ul, ul, ul]
    x.XPending.argtypes = [p]
    x.XNextEvent.argtypes = [p, p]
    x.XGetWindowProperty.argtypes = [p, ul, ul, ctypes.c_long, ctypes.c_long, ctypes.c_int, ul,
                                     ctypes.POINTER(ul), ctypes.POINTER(ctypes.c_int),
                                     ctypes.POINTER(ul), ctypes.POINTER(ul),
                                     ctypes.POINTER(p)]
    x.XFree.argtypes = [p]
    x.XCloseDisplay.argtypes = [p]
    return x


def primary_selection(timeout=2.0):
    """The X PRIMARY selection as UTF-8 text, or None when nobody owns it.

    xclip and xsel are not assumed, so this asks for it through libX11.
    """
    x = _xlib()
    d = x.XOpenDisplay(os.environ["DISPLAY"].encode())
    if not d:
        raise RuntimeError("cannot open the X display")
    try:
        win = x.XCreateSimpleWindow(d, x.XDefaultRootWindow(d), 0, 0, 1, 1, 0, 0, 0)
        prop = x.XInternAtom(d, b"B1_SELECTION", 0)
        x.XConvertSelection(d, x.XInternAtom(d, b"PRIMARY", 0),
                            x.XInternAtom(d, b"UTF8_STRING", 0), prop, win, 0)
        ev = _Event()
        deadline = time.monotonic() + timeout
        while True:
            if x.XPending(d):
                x.XNextEvent(d, ctypes.byref(ev))
                if ev.type == 31:  # SelectionNotify
                    break
            elif time.monotonic() > deadline:
                return None
            else:
                time.sleep(0.01)
        if ev.xselection.property == 0:
            return None
        rtype, fmt = ctypes.c_ulong(), ctypes.c_int()
        n, after, data = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_void_p()
        x.XGetWindowProperty(d, win, prop, 0, 1 << 24, 1, 0, ctypes.byref(rtype),
                             ctypes.byref(fmt), ctypes.byref(n), ctypes.byref(after),
                             ctypes.byref(data))
        if not data:
            return ""
        text = ctypes.string_at(data, n.value).decode("utf-8", "replace")
        x.XFree(data)
        return text
    finally:
        x.XCloseDisplay(d)


# ------------------------------------------------------------ fixtures

def events(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def source_blocks(path):
    """The blocks a reader would copy, as the fixture's events give them.

    Prose is the turn's text deltas joined; any other event ends a run of
    prose. A run splits into blocks at blank lines outside code fences; a
    fenced block keeps its fences. A diff is its hunk's lines.
    """
    runs, prose = [], ""
    for e in events(path):
        if e["type"] == "text_delta":
            prose += e["text"]
            continue
        if e["type"] in ("status", "tool_output"):
            continue
        if prose:
            runs.append(prose)
            prose = ""
        if e["type"] == "diff":
            runs.append(("diff", e["diff"].rstrip("\n").split("\n")))
    if prose:
        runs.append(prose)
    out = []
    for run in runs:
        if isinstance(run, tuple):
            out.append(run)
            continue
        blocks = []
        current, fenced = [], False
        for line in run.rstrip("\n").split("\n"):
            if line.startswith("```"):
                if not fenced and current:
                    blocks.append(current)
                    current = []
                current.append(line)
                fenced = not fenced
                if not fenced:
                    blocks.append(current)
                    current = []
            elif not fenced and line.strip() == "":
                if current:
                    blocks.append(current)
                current = []
            else:
                current.append(line)
        if current:
            blocks.append(current)
        out += [("code" if b[0].startswith("```") else "prose", b) for b in blocks]
    return out


def char_width(ch):
    if unicodedata.combining(ch) or ch in "‍️":
        return 0
    return 2 if unicodedata.east_asian_width(ch) in "WF" else 1


def wrapped_rows(line, cols):
    """Screen rows a logical line takes when the terminal wraps it."""
    rows, col = 1, 0
    for ch in line:
        w = char_width(ch)
        if col + w > cols:
            rows += 1
            col = 0
        col += w
    return rows


def locate(lines, block, start):
    """Index of the block's first and last logical line in `lines`."""
    def key(s):
        return s.strip()[:40]
    first = next((i for i in range(start, len(lines))
                  if key(block[0]) and lines[i].strip().startswith(key(block[0]))), None)
    if first is None:
        return None
    tail = block[-1].strip()[-40:]
    for i in range(first + len(block) - 1, min(len(lines), first + len(block) + 20)):
        if lines[i].rstrip().endswith(tail):
            return first, i
    return None


def levenshtein(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def copy_errors(copied, source_lines):
    """How a copied block differs from its source."""
    source = "\n".join(source_lines)
    text = copied[:-1] if copied.endswith("\n") else copied
    got = text.split("\n")
    trailing = sum(1 for line in got if line != line.rstrip(" \t"))
    # Pair lines by content without leading blanks to judge indentation.
    want = {}
    for line in source_lines:
        want.setdefault(line.lstrip(), []).append(len(line) - len(line.lstrip()))
    indent = 0
    for line in got:
        body = line.lstrip()
        if body and body in want:
            depth = len(line) - len(body)
            if depth not in want[body]:
                indent += 1
    breaks = 0
    matcher = difflib.SequenceMatcher(None, source, text, autojunk=False)
    for op, i1, i2, j1, j2 in matcher.get_opcodes():
        if op != "equal":
            breaks += max(0, text[j1:j2].count("\n") - source[i1:i2].count("\n"))
    return {
        "chars_wrong": levenshtein(text, source),
        "trailing_space_lines": trailing,
        "indent_not_in_source": indent,
        "breaks_not_in_source": breaks,
        "lines": len(got),
        "source_lines": len(source_lines),
    }


def lines_differing(a, b):
    n = 0
    for op, i1, i2, j1, j2 in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes():
        if op != "equal":
            n += max(i2 - i1, j2 - j1)
    return n


# ------------------------------------------------------------ screenshots

def grab(geom):
    x, y, w, h = geom
    return ImageGrab.grab(bbox=(x, y, x + w, y + h), xdisplay=os.environ["DISPLAY"]).convert("RGB")


def pixel_rows(img):
    w = img.width * 3
    data = img.tobytes()
    return [data[i * w:(i + 1) * w] for i in range(img.height)]


def view_shift(base, cur, top):
    """Pixel offset at which the top `top` rows of `base` appear in `cur`.

    Positive: the content moved down (the view went further back); negative:
    it moved up (toward newer output). None when too few rows match anywhere,
    which means the content left the screen. The rightmost 24 pixels are
    left out: Ghostty draws its scrollbar there, which moves as output
    grows even when the view holds.
    """
    def body(img):
        return img.crop((0, 0, img.width - 24, img.height))
    b, c = pixel_rows(body(base))[:top], pixel_rows(body(cur))
    counts = {}
    for row in b:
        counts[row] = counts.get(row, 0) + 1
    where = {}
    for y, row in enumerate(c):
        where.setdefault(row, []).append(y)
    votes, usable = {}, 0
    for y, row in enumerate(b):
        if counts[row] > 2:
            continue
        usable += 1
        for at in where.get(row, ()):
            votes[at - y] = votes.get(at - y, 0) + 1
    if not votes or usable == 0:
        return None
    shift, n = max(votes.items(), key=lambda kv: (kv[1], -abs(kv[0])))
    return shift if n >= 0.5 * usable else None


def differing_rows(a, b, cell):
    """Cell rows of shot `a` with no counterpart in shot `b`, in order.

    Rows are matched as a diff of their pixels, so a screen that is the
    same but one row higher counts one row (the extra blank one), not all.
    The rightmost 24 pixels are left out for Ghostty's scrollbar, as in
    view_shift.
    """
    _, _, cw, ch, cols, rows = cell
    width = min(round(cols * cw), a.width, b.width) - 24

    def strips(img):
        return [img.crop((0, round(r * ch), width, round((r + 1) * ch))).tobytes()
                for r in range(rows)]
    return lines_differing(strips(a), strips(b))


# ------------------------------------------------------------ launches

WRAPPER = r"""#!/bin/sh
d={dir}
printf '\033]2;%s\007' {title}
# Until go: draw what $d/cal asks for (two inverse cells, or nothing) on a
# clear screen, again after every resize, and report the size each time.
draw() {{
    seq 1 300
    printf '\033[?25l\033[H\033[2J'
    [ "$1" = on ] && printf '\033[7m \033[0m\033[11;21H\033[7m \033[0m'
    stty size >"$d/size.tmp" && mv "$d/size.tmp" "$d/size"
    echo "$1" >"$d/drawn.tmp" && mv "$d/drawn.tmp" "$d/drawn"
}}
shown=none
trap 'shown=stale' WINCH
while [ ! -e "$d/go" ]; do
    want=$(cat "$d/cal" 2>/dev/null || echo off)
    [ "$want" = "$shown" ] || {{ draw "$want"; shown=$want; }}
    sleep 0.05
done
trap - WINCH
printf '\033[?25h\033[H\033[2J\033[3J'
{steps}
touch "$d/done"
exec sleep 100000
"""


class Launch:
    """One terminal window running the console through the wrapper."""

    counter = 0

    def __init__(self, h, target, name, fixtures, cols, rows, cfg=None):
        Launch.counter += 1
        self.h, self.target, self.name = h, target, name
        self.cfg = cfg or h.cfg
        self.title = f"b1-{name}"
        self.dir = h.out / "run" / name
        self.dir.mkdir(parents=True, exist_ok=True)
        for f in self.dir.iterdir():
            f.unlink()
        self.runtime = h.runtime / str(Launch.counter)
        self.runtime.mkdir(mode=0o700)
        steps = []
        env = "" if target == "ultty-native" else "env -u ULTTY_SOCKET ULTTY_IN_BAND=0 "
        for i, (fixture, pace) in enumerate(fixtures, 1):
            steps.append(f"touch \"$d/start-{i}\"")
            steps.append(f"{env}{shlex.quote(str(h.bin / 'agent-console'))} replay "
                         f"{shlex.quote(str(fixture))} {pace} --auto-approve 1 "
                         f"2>>\"$d/console.log\"")
        script = self.dir / "wrapper.sh"
        script.write_text(WRAPPER.format(dir=shlex.quote(str(self.dir)),
                                         title=shlex.quote(self.title),
                                         steps="\n".join(steps)))
        script.chmod(0o755)
        self.proc = self._spawn(script)
        h.live.append(self)
        self.pid = None
        self.win = self._find_window()
        self.cell = None
        self.sizes = {}
        self.fit(cols, rows)
        self.calibrate()

    def _spawn(self, script):
        env = dict(os.environ, XDG_RUNTIME_DIR=str(self.runtime), B1_TOKEN=self.title)
        env.pop("ULTTY_SOCKET", None)
        env.pop("ULTTY_PID", None)
        log_file = open(self.dir / "terminal.log", "w")
        kind = self.target.split("-")[0]
        if kind == "ultty":
            env.update(XDG_CONFIG_HOME=str(self.cfg / "ultty"), SHELL=str(script),
                       RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn,ultty::perf=info")
            cmd = [str(self.h.bin / "ultty")]
        elif kind == "alacritty":
            cmd = ["alacritty", "--config-file", str(self.cfg / "alacritty.toml"),
                   "-e", str(script)]
        elif kind == "ghostty":
            # As in compare.sh: a snap Ghostty needs the real session bus;
            # no portal and no single-instance name keep it to itself.
            env.update(GTK_USE_PORTAL="0", GDK_DEBUG="no-portals")
            cmd = [self.h.ghostty, "--gtk-single-instance=false", "--config-default-files=false",
                   f"--config-file={self.cfg / 'ghostty.conf'}", "-e", str(script)]
        else:
            raise ValueError(self.target)
        return subprocess.Popen(cmd, env=env, stdout=log_file, stderr=subprocess.STDOUT,
                                start_new_session=True)

    def _find_window(self):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"{self.name}: the terminal exited at startup")
            out = xdotool("search", "--onlyvisible", "--name", f"^{self.title}$", check=False)
            if out.split() and (self.dir / "size").exists():
                return out.split()[0]
            time.sleep(0.2)
        raise RuntimeError(f"{self.name}: no window titled {self.title}")

    def owner_pid(self):
        """The process whose memory is the terminal's."""
        if self.pid:
            return self.pid
        if not self.target.startswith("ghostty"):
            self.pid = self.proc.pid
            return self.pid
        for pid in subprocess.run(["pgrep", "-x", "ghostty"], capture_output=True,
                                  text=True).stdout.split():
            try:
                env = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
            except OSError:
                continue
            if f"B1_TOKEN={self.title}".encode() in env:
                self.pid = int(pid)
                return self.pid
        raise RuntimeError(f"{self.name}: no ghostty process carries its token")

    def ctl(self, *args):
        env = dict(os.environ, XDG_RUNTIME_DIR=str(self.runtime))
        return subprocess.run([str(self.h.bin / "ulttyctl"), "--pid", str(self.proc.pid), *args],
                              env=env, check=True, capture_output=True, text=True).stdout

    def geometry(self):
        out = xdotool("getwindowgeometry", "--shell", self.win)
        g = dict(line.split("=", 1) for line in out.split())
        return int(g["X"]), int(g["Y"]), int(g["WIDTH"]), int(g["HEIGHT"])

    def size(self):
        rows, cols = (self.dir / "size").read_text().split()
        return int(cols), int(rows)

    def _resize(self, w, h):
        before = (self.dir / "size").stat().st_mtime_ns
        xdotool("windowsize", self.win, w, h)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if (self.dir / "size").stat().st_mtime_ns != before:
                time.sleep(0.1)
                return
            time.sleep(0.05)

    def fit(self, cols, rows):
        """Resize to `cols` x `rows` cells, checked with stty in the window."""
        _, _, w, h = self.geometry()
        c, r = self.size()
        cw, ch = w / c, h / r
        if self.cell:
            cw, ch = self.cell[2], self.cell[3]
        for _ in range(12):
            if (c, r) == (cols, rows):
                self.sizes[cols] = (w, h)
                return
            w = round(w + (cols - c) * cw + (cw / 2 if c != cols else 0))
            h = round(h + (rows - r) * ch + (ch / 2 if r != rows else 0))
            self._resize(w, h)
            _, _, w, h = self.geometry()
            c, r = self.size()
        raise RuntimeError(f"{self.name}: cannot fit {cols}x{rows}, have {c}x{r}")

    def show(self, state):
        """Ask the wrapper to draw `state` (on: the calibration cells)."""
        drawn = self.dir / "drawn"
        if drawn.exists() and drawn.read_text().strip() == state:
            time.sleep(0.4)
            return
        before = drawn.stat().st_mtime_ns if drawn.exists() else 0
        (self.dir / "cal").write_text(state + "\n")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if drawn.exists() and drawn.stat().st_mtime_ns != before:
                time.sleep(0.4)
                return
            time.sleep(0.05)
        raise RuntimeError(f"{self.name}: the wrapper did not draw {state}")

    def calibrate(self):
        """Cell origin and pitch, from a shot with two inverse cells (row 1
        column 1 and row 11 column 21) against one without them, and the
        lines one wheel click scrolls."""
        self.show("off")
        blank = grab(self.geometry())
        self.show("on")
        img = grab(self.geometry())
        # Only solid areas of the cells' colour count: Ultty animates the
        # title in its header bar, which differs between the shots too, and
        # Ghostty's scrollbar (the rightmost pixels) moves.
        changed = ImageChops.difference(img, blank).convert("L").point(lambda v: 255 if v else 0)
        changed.paste(0, (img.width - 24, 0, img.width, img.height))
        colours = Image.new("RGB", img.size)
        colours.paste(img, mask=changed)
        cell_colour = max(c for c in colours.getcolors(img.width * img.height)
                          if c[1] != (0, 0, 0))[1]
        same = ImageChops.difference(img, Image.new("RGB", img.size, cell_colour)).convert("L")
        mask = ImageChops.multiply(changed, same.point(lambda v: 0 if v else 255))
        box = mask.filter(ImageFilter.MinFilter(3)).getbbox()
        if not box:
            raise RuntimeError(f"{self.name}: calibration cells not found")
        x0, y0, x1, y1 = max(0, box[0] - 1), max(0, box[1] - 1), box[2] + 1, box[3] + 1
        cw, ch = (x1 - x0) / 21, (y1 - y0) / 11
        self.cell = (x0, y0, cw, ch)
        cols, rows = self.size()
        self.cell_grid = (*self.cell, cols, rows)
        # The second cell's column below the grid's top holds nothing else,
        # so where it is shows how far the wrapper's `seq` output above it
        # was scrolled into view.
        strip = (round(x0 + 20 * cw), round(y0), round(x0 + 21 * cw), img.height)

        def cell_top(shot):
            found = ImageChops.difference(shot.crop(strip), blank.crop(strip)).getbbox()
            return found[1] if found else None
        start = cell_top(img)
        # Ghostty sometimes ignores the wheel for a moment after a resize.
        for _ in range(4):
            self.wheel(True, 4)
            time.sleep(0.4)
            scrolled = self.shot(grid=False)
            moved = cell_top(scrolled)
            self.wheel(False, 40)
            time.sleep(0.3)
            if moved is not None and moved != start:
                break
            time.sleep(1)
        if start is None or moved is None or moved == start:
            img.save(self.dir / "calibrate.png")
            scrolled.save(self.dir / "calibrate-wheel.png")
            raise RuntimeError(f"{self.name}: the wheel did not scroll (cell {self.cell})")
        self.lines_per_click = (moved - start) / ch / 4

    def resize_to(self, cols):
        """Resize to a width fitted before the stream started."""
        w, h = self.sizes[cols]
        xdotool("windowsize", self.win, w, h)

    def go(self):
        cols, rows = self.size()
        self.cell_grid = (*self.cell, cols, rows)
        (self.dir / "go").touch()

    def wait(self, name, timeout=300):
        deadline = time.monotonic() + timeout
        while not (self.dir / name).exists():
            if time.monotonic() > deadline:
                raise RuntimeError(f"{self.name}: {name} not reached in {timeout} s")
            if self.proc.poll() is not None:
                raise RuntimeError(f"{self.name}: the terminal exited")
            time.sleep(0.02)

    def done(self):
        return (self.dir / "done").exists()

    def shot(self, grid=True):
        """The window, or only its cell grid (without title bar or padding)."""
        x, y, w, h = self.geometry()
        if not grid:
            return grab((x, y, w, h))
        x0, y0, cw, ch, cols, rows = self.cell_grid
        return grab((x + round(x0), y + round(y0), min(w - round(x0), round(cols * cw)),
                     min(h - round(y0), round(rows * ch))))

    def point(self, col, row):
        x0, y0, cw, ch = self.cell
        return round(x0 + (col + 0.5) * cw), round(y0 + (row + 0.5) * ch)

    def wheel(self, up, clicks):
        x, y = self.point(COLS // 4, 3)
        # A real motion first, so the terminal knows the pointer is in it.
        xdotool("mousemove", "--window", self.win, x + 5, y + 5)
        time.sleep(0.05)
        xdotool("mousemove", "--window", self.win, x, y)
        xdotool("click", "--repeat", clicks, "--delay", 15, 4 if up else 5)

    def drag(self, row0, row1, cols):
        """Select rows row0..row1 whole, pressing at the end of row1 and
        releasing at the start of row0. Pressing at the end keeps the press
        out of Ultty's 8-pixel window resize band at the left edge, where
        its first column lies; the window's right edge has slack."""
        x0, y0, cw, ch = self.cell
        width = self.geometry()[2]
        xa = min(round(x0 + (cols - 0.3) * cw), width - 9)
        ya = round(y0 + (row1 + 0.5) * ch)
        xb = round(x0 + 0.2 * cw)
        yb = round(y0 + (row0 + 0.5) * ch)
        xdotool("mousemove", "--window", self.win, xa, ya)
        xdotool("mousedown", 1)
        steps = 6
        for i in range(1, steps + 1):
            time.sleep(0.03)
            xdotool("mousemove", "--window", self.win, round(xa + (xb - xa) * i / steps),
                    round(ya + (yb - ya) * i / steps))
        time.sleep(0.05)
        xdotool("mouseup", 1)
        time.sleep(0.3)

    def selection(self):
        if self.target.startswith("ultty"):
            return self.ctl("selection")
        return primary_selection()

    def close(self):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGTERM)
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait()
        if self in self.h.live:
            self.h.live.remove(self)


# ------------------------------------------------------------ measurements

def measure_scroll(h, target, name, fixtures, wait_for, delay):
    run = Launch(h, target, name, fixtures, COLS, SCROLL_ROWS)
    try:
        run.go()
        run.wait(wait_for)
        time.sleep(delay)
        ch = run.cell[3]
        clicks = max(1, round(SCROLL_LINES / run.lines_per_click))
        before = run.shot()
        run.wheel(True, clicks)
        time.sleep(0.1)
        base = run.shot()
        top = round((SCROLL_ROWS // 2) * ch)
        # Content that moved up or stayed put means the view was back at
        # the bottom before the first shot.
        first = view_shift(before, base, top)
        samples, at = [], []
        started = time.monotonic()
        while not run.done():
            time.sleep(SAMPLE_S)
            s = view_shift(base, run.shot(), top)
            at.append(time.monotonic() - started)
            samples.append(None if s is None else s / ch)
        time.sleep(0.5)
        end = run.shot()
        last = view_shift(base, end, top)
        run.wheel(False, 400)
        time.sleep(0.5)
        bottom = run.shot()
        for label, img in (("before", before), ("scrolled", base), ("end", end),
                           ("bottom", bottom)):
            img.save(h.out / "shots" / f"{name}-{label}.png")
        moved = [abs(round(s)) for s in samples if s is not None]
        lost = sum(1 for s in samples if s is None)
        return {
            "lines_per_click": round(run.lines_per_click, 2),
            "clicks": clicks,
            "first_shot_shift_rows": None if first is None else round(first / ch),
            "back_at_bottom_before_first_shot": first is not None and first <= 0,
            "samples": len(samples),
            "sampled_s": round(time.monotonic() - started, 2),
            "max_rows_moved": None if lost else (max(moved) if moved else 0),
            "samples_lost": lost,
            "first_move_at_s": next((round(t, 2) for t, s in zip(at, samples, strict=True)
                                     if s is None or round(s) != 0), None),
            "final_rows_moved": None if last is None else round(last / ch),
            # Wheeling down changed nothing: the view was already at the bottom.
            "yanked_to_bottom": not ImageChops.difference(end, bottom).getbbox(),
        }
    finally:
        run.close()


def screen_text(run):
    return run.ctl("text").rstrip("\n").split("\n")


def measure_copy(h, target, layout):
    blocks = source_blocks(FIXTURES / "short.jsonl")
    run = Launch(h, target, f"{target}-copy", [(FIXTURES / "short.jsonl", "--speed 4")],
                 COLS, COPY_ROWS)
    try:
        run.go()
        run.wait("done", 120)
        time.sleep(1.0)
        if target.startswith("ultty"):
            lines = screen_text(run)
            (run.dir / "screen.txt").write_text("\n".join(lines) + "\n")
        else:
            lines = layout
        if lines is None:
            raise RuntimeError("the reference terminals need ultty-ansi's layout first")
        rows_before, row = [], 0
        for line in lines:
            rows_before.append(row)
            row += wrapped_rows(line, COLS)
        results, start = [], 0
        for kind, block in blocks:
            where = locate(lines, block, start)
            if where is None:
                results.append({"kind": kind, "first": block[0][:60], "found": False})
                continue
            first, last = where
            start = last + 1
            r0 = rows_before[first]
            r1 = rows_before[last] + wrapped_rows(lines[last], COLS) - 1
            if r1 >= COPY_ROWS:
                results.append({"kind": kind, "first": block[0][:60], "found": False,
                                "why": "below the screen"})
                continue
            run.drag(r0, r1, COLS)
            copied = run.selection()
            entry = {"kind": kind, "first": block[0][:60], "found": True, "rows": [r0, r1]}
            if copied is None:
                entry["error"] = "no selection"
            else:
                entry.update(copy_errors(copied, block))
                entry["copied"] = copied
            results.append(entry)
        return {"blocks": results}, lines
    finally:
        run.close()


def screens(run):
    """The screen at the bottom and at the top of scrollback, as shots."""
    bottom = run.shot()
    run.wheel(True, 600)
    time.sleep(0.5)
    top = run.shot()
    run.wheel(False, 600)
    time.sleep(0.3)
    return bottom, top


def measure_resize(h, target):
    fixture = [(FIXTURES / "short.jsonl", "--speed 1")]
    final = RESIZE_AFTER[-1]
    run = Launch(h, target, f"{target}-resize", fixture, COLS, SCROLL_ROWS)
    try:
        for cols in (*RESIZE_DURING, *RESIZE_AFTER):
            if cols not in run.sizes:
                run.fit(cols, SCROLL_ROWS)
        run.fit(COLS, SCROLL_ROWS)
        run.go()
        run.wait("start-1")
        for cols in RESIZE_DURING:
            time.sleep(2.4)
            run.resize_to(cols)
        run.wait("done", 120)
        for cols in RESIZE_AFTER:
            time.sleep(0.6)
            run.resize_to(cols)
        time.sleep(1.5)
        # The wrapper reports no sizes once the console runs; the window's
        # pixel size is the one fitted to `final` columns before the stream.
        if run.geometry()[2:] != run.sizes[final]:
            raise RuntimeError(f"{run.name}: ended at {run.geometry()[2:]} pixels, "
                               f"not {run.sizes[final]}")
        run.cell_grid = (*run.cell, final, SCROLL_ROWS)
        text = run.ctl("text", "--scrollback") if target.startswith("ultty") else None
        shots = screens(run)
        grid = (0, 0, run.cell[2], run.cell[3], final, SCROLL_ROWS)
    finally:
        run.close()
    fresh = Launch(h, target, f"{target}-fresh", fixture, final, SCROLL_ROWS)
    try:
        fresh.go()
        fresh.wait("done", 120)
        time.sleep(1.0)
        fresh_text = fresh.ctl("text", "--scrollback") if text is not None else None
        fresh_shots = screens(fresh)
    finally:
        fresh.close()
    result = {
        "final_cols": final,
        "bottom_rows_differing": differing_rows(shots[0], fresh_shots[0], grid),
        "top_rows_differing": differing_rows(shots[1], fresh_shots[1], grid),
        "screen_rows": SCROLL_ROWS,
    }
    for label, img in (("bottom", shots[0]), ("top", shots[1])):
        img.save(h.out / "shots" / f"{target}-resize-{label}.png")
    for label, img in (("bottom", fresh_shots[0]), ("top", fresh_shots[1])):
        img.save(h.out / "shots" / f"{target}-fresh-{label}.png")
    if text is not None:
        a = text.rstrip("\n").split("\n")
        b = fresh_text.rstrip("\n").split("\n")
        (h.out / "run" / f"{target}-resize" / "text.txt").write_text(text)
        (h.out / "run" / f"{target}-fresh" / "text.txt").write_text(fresh_text)
        result["transcript_lines_differing"] = lines_differing(a, b)
        result["transcript_lines"] = len(b)
    return result


def memory_sample(pid):
    status = Path(f"/proc/{pid}/status").read_text()

    def field(name):
        m = re.search(rf"^{name}:\s+(\d+)", status, re.M)
        return int(m.group(1)) if m else 0
    rss = 0
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        if line.startswith("Rss:"):
            rss = int(line.split()[1])
    gpu, memfd = 0, False
    for line in Path(f"/proc/{pid}/smaps").read_text().splitlines():
        if re.match(r"^[0-9a-f]+-[0-9a-f]+ ", line):
            memfd = "memfd" in line
        elif memfd and line.startswith("Rss:"):
            gpu += int(line.split()[1])
    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    cpu = (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")
    return rss, field("RssAnon"), field("VmSwap"), gpu, cpu


def heap_kib(run):
    path = run.dir / "terminal.log"
    plain = re.sub(r"\x1b\[[0-9;]*m", "", path.read_text(errors="replace"))
    found = re.findall(r"heap_live_bytes=(\d+)", plain)
    return int(found[-1]) // 1024 if found else ""


def measure_memory(h, targets, minutes, warmup):
    fixture = [(FIXTURES / "two-hours.jsonl", "--speed 10")]
    runs = [Launch(h, t, f"{t}-memory", fixture, COLS, 40, cfg=h.cfg / "memory")
            for t in targets]
    results = {}
    try:
        for run in runs:
            run.go()
        files = {}
        for run in runs:
            f = open(h.out / f"memory-{run.target}.csv", "w")
            f.write("minute,rss_kib,anon_kib,swap_kib,gpu_kib,cpu_s,frames,frame_p50_us,"
                    "frame_p99_us,width,height,heap_kib\n")
            files[run.target] = f
        started = time.monotonic()
        while time.monotonic() - started < minutes * 60:
            time.sleep(30 - (time.monotonic() - started) % 30)
            minute = (time.monotonic() - started) / 60
            for run in runs:
                rss, anon, swap, gpu, cpu = memory_sample(run.owner_pid())
                heap = heap_kib(run) if run.target.startswith("ultty") else ""
                files[run.target].write(f"{minute:.2f},{rss},{anon},{swap},{gpu},{cpu:.2f},"
                                        f"0,0,0,0,0,{heap}\n")
                files[run.target].flush()
            if all(run.done() for run in runs):
                break
        for run in runs:
            files[run.target].close()
            out = subprocess.run([sys.executable, "-I", str(HERE / "soak-report.py"),
                                  str(h.out / f"memory-{run.target}.csv"),
                                  "--warmup", str(warmup)], capture_output=True, text=True)
            trend = re.search(r"memory trend after warm-up: ([-+0-9.]+)", out.stdout)
            heap = re.search(r"Rust heap .* trend ([-+0-9.]+)", out.stdout)
            results[run.target] = {
                "replay_finished": run.done(),
                "minutes": round((time.monotonic() - started) / 60, 1),
                "anon_trend_mib_per_hour": float(trend.group(1)) if trend else None,
                "heap_trend_mib_per_hour": float(heap.group(1)) if heap else None,
                "report": out.stdout.strip(),
            }
    finally:
        for run in runs:
            run.close()
    return results


# ------------------------------------------------------------ report

def fmt(value, none="not measured"):
    return none if value is None else str(value)


def summary(results):
    """The four numbers per target, for the table."""
    rows = {}
    for t, r in results["targets"].items():
        s = r.get("scroll", {})
        scroll = []
        for label, k in (("short", "short"), ("long", "long")):
            v = s.get(k)
            if not v or "error" in v:
                scroll.append(f"{label}: not measured")
                continue
            if v["yanked_to_bottom"] or v["back_at_bottom_before_first_shot"]:
                text = f"yanked to bottom (>= {SCROLL_LINES} rows)"
            elif v["max_rows_moved"] is None:
                text = "moved off the screen"
            else:
                text = f"{v['max_rows_moved']} rows"
            scroll.append(f"{label}: {text}")
        c = r.get("copy", {})
        copy = "not measured"
        if c.get("blocks"):
            found = [b for b in c["blocks"] if b.get("found") and "chars_wrong" in b]
            if found:
                total = {k: sum(b[k] for b in found) for k in
                         ("chars_wrong", "trailing_space_lines", "indent_not_in_source",
                          "breaks_not_in_source")}
                copy = (f"{total['chars_wrong']} chars wrong, {total['trailing_space_lines']} "
                        f"trailing-space lines, {total['indent_not_in_source']} indented, "
                        f"{total['breaks_not_in_source']} extra breaks "
                        f"({len(found)}/{len(c['blocks'])} blocks)")
        z = r.get("resize", {})
        resize = "not measured"
        if "bottom_rows_differing" in z:
            resize = (f"screen rows differing: bottom {z['bottom_rows_differing']}, "
                      f"top {z['top_rows_differing']} of {z['screen_rows']}")
            if "transcript_lines_differing" in z:
                resize = (f"{z['transcript_lines_differing']} of {z['transcript_lines']} "
                          f"transcript lines; " + resize)
        m = results.get("memory", {}).get(t)
        memory = "not measured"
        if m and m.get("anon_trend_mib_per_hour") is not None:
            memory = f"{m['anon_trend_mib_per_hour']:+.2f} MiB/h anon"
            if m.get("heap_trend_mib_per_hour") is not None:
                memory += f", {m['heap_trend_mib_per_hour']:+.2f} MiB/h heap"
        rows[t] = {"scroll": "; ".join(scroll), "copy": copy, "resize": resize, "memory": memory}
    return rows


def write_report(h, results):
    rows = summary(results)
    targets = list(rows)
    lines = [f"# B1 agent console: {results['date']}", "",
             f"Ultty {results['ultty_rev']}, display {results['display']} (Xvfb), "
             f"{COLS} columns.", "",
             "| Number | " + " | ".join(targets) + " |",
             "|---|" + "---|" * len(targets)]
    for key, label in (("scroll", "Scroll held (rows moved)"), ("copy", "Copy fidelity"),
                       ("resize", "Resize (differing)"), ("memory", "Memory trend")):
        lines.append(f"| {label} | " + " | ".join(rows[t][key] for t in targets) + " |")
    lines += ["", "## Errors", ""]
    errs = [f"- {t}: {k}: {v['error']}" for t, r in results["targets"].items()
            for k, v in r.items() if isinstance(v, dict) and "error" in v]
    errs += [f"- {t}: scroll {k}: {v['error']}" for t, r in results["targets"].items()
             for k, v in r.get("scroll", {}).items() if isinstance(v, dict) and "error" in v]
    lines += errs or ["none"]
    lines += ["", "## Memory reports", ""]
    for t, m in results.get("memory", {}).items():
        lines += [f"### {t}", "", "```", m.get("report", m.get("error", "")), "```", ""]
    (h.out / "report.md").write_text("\n".join(lines) + "\n")


# ------------------------------------------------------------ main

class Harness:
    def __init__(self, args):
        self.out = Path(args.out).resolve()
        self.bin = Path(args.bin).resolve()
        self.cfg = Path(args.cfg).resolve()
        self.runtime = Path(args.runtime).resolve()
        self.ghostty = args.ghostty
        self.live = []
        (self.out / "shots").mkdir(parents=True, exist_ok=True)


def attempt(fn, *args):
    try:
        return fn(*args)
    except Exception as e:  # one failed measurement must not lose the others
        log(f"failed: {fn.__name__}{args[1:2]}: {e}")
        return {"error": str(e)}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--out", required=True)
    p.add_argument("--bin", required=True)
    p.add_argument("--cfg", required=True)
    p.add_argument("--runtime", required=True)
    p.add_argument("--ghostty", default="ghostty")
    p.add_argument("--targets", default="ultty-native,ultty-ansi,alacritty,ghostty")
    p.add_argument("--numbers", default="scroll,copy,resize,memory")
    p.add_argument("--memory-minutes", type=float, default=12.5)
    p.add_argument("--memory-warmup", type=int, default=8)
    args = p.parse_args()
    h = Harness(args)
    targets = args.targets.split(",")
    numbers = args.numbers.split(",")
    results = {
        "date": datetime.date.today().isoformat(),
        "display": os.environ.get("DISPLAY"),
        "ultty_rev": subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short=12", "HEAD"],
                                    capture_output=True, text=True).stdout.strip(),
        "targets": {t: {} for t in targets},
    }
    layout = None
    order = sorted(targets, key=lambda t: 0 if t == "ultty-ansi" else 1)
    try:
        for t in order:
            r = results["targets"][t]
            if "scroll" in numbers:
                log(f"{t}: scroll")
                r["scroll"] = {
                    "short": attempt(measure_scroll, h, t, f"{t}-scroll-short",
                                     [(FIXTURES / "short.jsonl", "--speed 1")], "start-1", 5.0),
                    "long": attempt(measure_scroll, h, t, f"{t}-scroll-long",
                                    [(FIXTURES / "short.jsonl", "--fast"),
                                     (FIXTURES / "long-tool-output.jsonl", "--speed 1")],
                                    "start-2", 4.0),
                }
            if "copy" in numbers:
                log(f"{t}: copy")
                got = attempt(measure_copy, h, t, layout)
                if isinstance(got, tuple):
                    r["copy"], lines = got
                    if t == "ultty-ansi":
                        layout = lines
                else:
                    r["copy"] = got
            if "resize" in numbers:
                log(f"{t}: resize")
                r["resize"] = attempt(measure_resize, h, t)
            (h.out / "results.json").write_text(json.dumps(results, indent=2))
        if "memory" in numbers:
            log(f"memory: {args.memory_minutes} minutes")
            results["memory"] = attempt(measure_memory, h, targets, args.memory_minutes,
                                        args.memory_warmup)
            if "error" in results["memory"]:
                results["memory"] = {t: results["memory"] for t in targets}
    finally:
        for run in list(h.live):
            run.close()
        (h.out / "results.json").write_text(json.dumps(results, indent=2))
        write_report(h, results)
    log(f"wrote {h.out / 'results.json'} and {h.out / 'report.md'}")


if __name__ == "__main__":
    main()
