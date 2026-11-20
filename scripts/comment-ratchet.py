#!/usr/bin/env python3
"""Ratchet on plain code comments in the Rust sources.

A plain comment is a `//` or `/* */` comment that is not documentation
(`///`, `//!`, `/** */`, `/*! */`) and not a `// SAFETY:` justification for
`unsafe`. Each line that holds one counts once.

    scripts/comment-ratchet.py            # check: fail if any file has more
                                          # plain comment lines than its baseline
    scripts/comment-ratchet.py --update   # lower the baseline to today's counts

The baseline (scripts/comment-baseline.json) may only go down: --update
refuses to raise a file's count or to admit a new file with comments. Keep a
comment by explaining why it must stay in the pull request and raising the
file's entry by hand; delete the rest. See .claude/skills/delete-comment.
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "scripts" / "comment-baseline.json"
PATHSPECS = ["crates/*.rs", "examples/*.rs", "tools/*.rs"]


def comment_lines(source: str) -> set[int]:
    """Line numbers (1-based) that carry a plain comment."""
    lines: set[int] = set()
    i, n, line = 0, len(source), 1

    def advance(to: int) -> None:
        nonlocal i, line
        line += source.count("\n", i, to)
        i = to

    while i < n:
        c = source[i]
        nxt = source[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            end = source.find("\n", i)
            end = n if end == -1 else end
            text = source[i:end]
            doc = text.startswith("///") and not text.startswith("////") or text.startswith("//!")
            safety = text[2:].lstrip().startswith("SAFETY:")
            if not doc and not safety:
                lines.add(line)
            advance(end)
        elif c == "/" and nxt == "*":
            doc = source.startswith("/**", i) and not source.startswith("/**/", i) and not source.startswith("/***", i) or source.startswith("/*!", i)
            depth, j, start_line = 1, i + 2, line
            while j < n and depth:
                if source.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif source.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            end_line = start_line + source.count("\n", i, j)
            if not doc:
                lines.update(range(start_line, end_line + 1))
            advance(j)
        elif c == "r" and (nxt == '"' or nxt == "#") and not (i and (source[i - 1].isalnum() or source[i - 1] == "_")):
            j = i + 1
            hashes = 0
            while j < n and source[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and source[j] == '"':
                close = '"' + "#" * hashes
                end = source.find(close, j + 1)
                advance(n if end == -1 else end + len(close))
            else:
                advance(i + 1)
        elif c == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            advance(min(j + 1, n))
        elif c == "'":
            # A char literal ('x', '\n', '\u{1F600}') or a lifetime ('a).
            if nxt == "\\":
                end = source.find("'", i + 2)
                advance(n if end == -1 else end + 1)
            elif i + 2 < n and source[i + 2] == "'":
                advance(i + 3)
            else:
                advance(i + 1)
        else:
            advance(i + 1)
    return lines


def current_counts() -> dict[str, int]:
    files = subprocess.run(
        ["git", "ls-files", "--", *PATHSPECS], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout.split()
    counts = {}
    for name in sorted(files):
        source = (ROOT / name).read_text(encoding="utf-8", errors="replace")
        count = len(comment_lines(source))
        if count:
            counts[name] = count
    return counts


def main() -> int:
    update = "--update" in sys.argv[1:]
    counts = current_counts()
    baseline = json.loads(BASELINE.read_text()) if BASELINE.exists() else None
    if baseline is None:
        if not update:
            print(f"comment-ratchet: no baseline at {BASELINE}; run with --update to create it", file=sys.stderr)
            return 1
        BASELINE.write_text(json.dumps(counts, indent=1, sort_keys=True) + "\n")
        print(f"comment-ratchet: wrote baseline, {sum(counts.values())} lines in {len(counts)} files")
        return 0

    over = {f: (c, baseline.get(f, 0)) for f, c in counts.items() if c > baseline.get(f, 0)}
    if over:
        for f, (c, b) in sorted(over.items()):
            print(f"comment-ratchet: {f}: {c} plain comment lines, baseline {b}", file=sys.stderr)
        print(
            "comment-ratchet: delete the new comments, or move the reason into a name, type,"
            " test or ADR. If a comment must stay, raise that file's entry in"
            " scripts/comment-baseline.json in the same pull request and say why.",
            file=sys.stderr,
        )
        return 1
    lowered = {f: c for f, c in counts.items() if c < baseline.get(f, 0)}
    gone = [f for f in baseline if f not in counts]
    if update:
        BASELINE.write_text(json.dumps(counts, indent=1, sort_keys=True) + "\n")
        print(f"comment-ratchet: baseline lowered to {sum(counts.values())} lines in {len(counts)} files")
    elif lowered or gone:
        print(
            f"comment-ratchet: {len(lowered) + len(gone)} files are below their baseline;"
            " run scripts/comment-ratchet.py --update to lock that in"
        )
    print(f"comment-ratchet: ok, {sum(counts.values())} plain comment lines (baseline {sum(baseline.values())})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
