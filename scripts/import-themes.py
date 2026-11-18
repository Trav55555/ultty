#!/usr/bin/env python3
"""Convert Ghostty-format colour schemes into Ultty theme files.

Reads a checkout of iTerm2-Color-Schemes
(https://github.com/mbadolato/iTerm2-Color-Schemes): its `ghostty/` directory
holds the schemes and its `licenses.yml` records each scheme's licence. Writes
one TOML file per selected theme into crates/ultty/themes/, a notice listing
every theme's licence, and a copy of the upstream licence.

Only schemes whose recorded licence is on the allow list below are converted.
A selected scheme with an unknown, non-free or copyleft licence is an error,
so the bundled set cannot grow past what the licences allow by accident.

Usage:
    scripts/import-themes.py --upstream PATH/TO/iTerm2-Color-Schemes

See docs/configuration.md ("Themes") for the selection file format.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_SELECTION = REPO / "scripts" / "themes" / "selection.txt"
DEFAULT_SOURCES = REPO / "scripts" / "themes" / "sources"
DEFAULT_OUT = REPO / "crates" / "ultty" / "themes"

# SPDX identifiers whose terms allow redistribution inside a binary that is
# itself MIT OR Apache-2.0. Copyleft (GPL, LGPL, CC-BY-SA), `unknown` and
# `nonfree` are deliberately absent.
ALLOWED_LICENSES = frozenset(
    {
        "0BSD",
        "Apache-2.0",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "CC0-1.0",
        "ISC",
        "MIT",
        "Unlicense",
        "WTFPL",
        "Zlib",
    }
)

UPSTREAM_NAME = "iTerm2-Color-Schemes"
UPSTREAM_URL = "https://github.com/mbadolato/iTerm2-Color-Schemes"
PALETTE_NAMES = (
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
)

# A selection has to be seen and its text has to be read. The Rust test
# `bundled_themes_have_a_legible_selection` (crates/ultty/src/themes.rs)
# holds every theme to the same two numbers; keep them in step.
#
# MIN_SELECTION_DISTINCT: WCAG contrast of the selection against the
# background. 1.3:1 is the least that shows as a distinct band; below it
# (Solarized is 1.15) the selection is barely there. It is deliberately far
# under WCAG's 3:1 for UI components: the selection is a fill behind text,
# not a control, and a 3:1 fill would hide the theme's own look.
# MIN_SELECTION_TEXT: WCAG contrast of selected text, 3:1 (the AA floor for
# large text and UI), the least that stays readable on a themed fill.
MIN_SELECTION_DISTINCT = 1.3
MIN_SELECTION_TEXT = 3.0


class SchemeError(Exception):
    """A scheme cannot be converted."""


@dataclass
class Scheme:
    name: str
    slug: str
    license: str
    source: str
    foreground: str
    background: str
    cursor: str
    selection: str
    selection_foreground: str | None
    palette: list[str]
    selection_note: str | None = None


def slugify(name: str) -> str:
    """`Catppuccin Mocha` -> `catppuccin-mocha`. Keep in step with
    `normalize_name` in crates/ultty/src/themes.rs."""
    slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
    if not slug:
        raise SchemeError(f"theme name {name!r} has no letters or digits")
    return slug


def parse_hex(value: str, context: str) -> str:
    text = value.strip().lstrip("#").lower()
    if not re.fullmatch(r"[0-9a-f]{6}", text):
        raise SchemeError(f"{context}: {value!r} is not a #rrggbb colour")
    return "#" + text


def parse_ghostty(text: str, name: str) -> dict:
    """Read a Ghostty colour scheme: `key = value` lines, `#` comments."""
    palette: dict[int, str] = {}
    keys: dict[str, str] = {}
    meta: dict[str, str] = {}
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        if line.startswith("#"):
            # Extra sources carry their licence in header comments.
            match = re.fullmatch(r"#\s*(license|source)\s*:\s*(.+)", line)
            if match:
                meta[match.group(1)] = match.group(2).strip()
            continue
        key, sep, value = line.partition("=")
        if not sep:
            raise SchemeError(f"{name}:{number}: expected `key = value`")
        key, value = key.strip(), value.strip()
        if key == "palette":
            index, sep, colour = value.partition("=")
            if not sep or not index.strip().isdigit() or int(index) > 15:
                raise SchemeError(f"{name}:{number}: bad palette entry {value!r}")
            palette[int(index)] = parse_hex(colour, f"{name}:{number}")
        else:
            keys[key] = value
    missing = [str(i) for i in range(16) if i not in palette]
    if missing:
        raise SchemeError(f"{name}: palette entries {', '.join(missing)} missing")
    for required in ("background", "foreground"):
        if required not in keys:
            raise SchemeError(f"{name}: no {required}")
    foreground = parse_hex(keys["foreground"], f"{name} foreground")
    background = parse_hex(keys["background"], f"{name} background")
    cursor = parse_hex(keys.get("cursor-color", foreground), f"{name} cursor-color")
    selection = parse_hex(
        keys.get("selection-background", foreground), f"{name} selection-background"
    )
    selection_foreground = None
    if "selection-foreground" in keys:
        selection_foreground = parse_hex(
            keys["selection-foreground"], f"{name} selection-foreground"
        )
    return {
        "foreground": foreground,
        "background": background,
        "cursor": cursor,
        "selection": selection,
        "selection_foreground": selection_foreground,
        "palette": [palette[i] for i in range(16)],
        "meta": meta,
    }


def _luminance(colour: str) -> float:
    def channel(value: int) -> float:
        c = value / 255
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (int(colour[i : i + 2], 16) for i in (1, 3, 5))
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)


def contrast(a: str, b: str) -> float:
    """WCAG 2.x contrast ratio of two `#rrggbb` colours."""
    la, lb = _luminance(a), _luminance(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def mix(a: str, b: str, steps: int, of: int) -> str:
    """`a` moved `steps`/`of` of the way to `b`, channel by channel."""
    out = "#"
    for i in (1, 3, 5):
        x, y = int(a[i : i + 2], 16), int(b[i : i + 2], 16)
        out += f"{x + ((y - x) * steps + of // 2) // of:02x}"
    return out


def legible_selection(scheme: Scheme) -> str | None:
    """Make the selection visible and its text readable, deterministically.

    Several upstream schemes pick a selection colour next to the background
    (Solarized, Everblush, Hybrid, ...) or a selection foreground that reads
    poorly on it. A selection that is too close to the background is moved
    towards the theme's foreground, 1/32 at a time, until it shows. A
    selection foreground below MIN_SELECTION_TEXT on the result is dropped:
    Ultty then picks a readable text colour per cell. Returns a one-line note
    of what changed, for the theme file, or None.
    """
    notes = []
    if contrast(scheme.selection, scheme.background) < MIN_SELECTION_DISTINCT:
        original = scheme.selection
        for step in range(1, 33):
            scheme.selection = mix(original, scheme.foreground, step, 32)
            if contrast(scheme.selection, scheme.background) >= MIN_SELECTION_DISTINCT:
                break
        notes.append(f"selection moved from {original} to show against the background")
    if scheme.selection_foreground and (
        contrast(scheme.selection_foreground, scheme.selection) < MIN_SELECTION_TEXT
    ):
        notes.append(f"upstream selection foreground {scheme.selection_foreground} dropped: low contrast")
        scheme.selection_foreground = None
    return "; ".join(notes) or None


def parse_licenses(text: str) -> dict[str, dict[str, str]]:
    """Read upstream's licenses.yml: top-level `Name:` keys, two-space
    indented `license:` and `source:` values. (No YAML library needed.)"""
    entries: dict[str, dict[str, str]] = {}
    current = None
    for raw in text.splitlines():
        if not raw or raw.startswith("#"):
            continue
        if not raw.startswith(" "):
            key = raw.rstrip()
            if not key.endswith(":"):
                continue
            key = key[:-1]
            if len(key) >= 2 and key[0] == key[-1] and key[0] in "'\"":
                key = key[1:-1].replace("''", "'")
            current = entries.setdefault(key, {})
        elif current is not None and ":" in raw:
            field, _, value = raw.strip().partition(":")
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
                value = value[1:-1]
            current[field] = value
    return entries


def license_allowed(expression: str) -> bool:
    """True when every term of an SPDX `A OR B` expression is allowed. (An
    OR expression would let us pick either term; requiring all is stricter
    and good enough for the licences upstream records.)"""
    terms = [t.strip() for t in re.split(r"\s+OR\s+", expression)]
    return bool(terms) and all(t in ALLOWED_LICENSES for t in terms)


@dataclass
class Selected:
    name: str
    slug: str
    license: str | None  # an override; None means use licenses.yml
    source: str | None


def parse_selection(text: str) -> list[Selected]:
    """`Name`, `Name => slug`, or `Name | SPDX | source-url` (an override,
    for schemes licenses.yml misses but whose licence has been checked)."""
    selected = []
    for number, raw in enumerate(text.splitlines(), 1):
        # Only a leading `#` starts a comment: URLs may contain one.
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        override_license = override_source = None
        if "|" in line:
            parts = [p.strip() for p in line.split("|")]
            if len(parts) != 3 or not all(parts):
                raise SchemeError(
                    f"selection line {number}: expected `Name | SPDX | source`"
                )
            line, override_license, override_source = parts
        slug = None
        if "=>" in line:
            line, _, slug = (p.strip() for p in line.partition("=>"))
        selected.append(
            Selected(line, slugify(slug or line), override_license, override_source)
        )
    return selected


def build_schemes(
    upstream: Path, selection: list[Selected], sources: Path | None
) -> tuple[list[Scheme], list[str]]:
    licenses = parse_licenses((upstream / "licenses.yml").read_text(encoding="utf-8"))
    schemes: list[Scheme] = []
    errors: list[str] = []

    def add(name: str, slug: str, text: str, license_: str | None, source: str | None):
        try:
            parsed = parse_ghostty(text, name)
        except SchemeError as error:
            errors.append(str(error))
            return
        license_ = license_ or parsed["meta"].get("license")
        if not license_ or not license_allowed(license_):
            errors.append(f"{name}: licence {license_ or 'missing'!r} is not on the allow list")
            return
        schemes.append(
            Scheme(
                name=name,
                slug=slug,
                license=license_,
                source=source or parsed["meta"].get("source", ""),
                foreground=parsed["foreground"],
                background=parsed["background"],
                cursor=parsed["cursor"],
                selection=parsed["selection"],
                selection_foreground=parsed["selection_foreground"],
                palette=parsed["palette"],
            )
        )

    for item in selection:
        path = upstream / "ghostty" / item.name
        if not path.is_file():
            errors.append(f"{item.name}: no such scheme in {upstream / 'ghostty'}")
            continue
        entry = licenses.get(item.name, {})
        add(
            item.name,
            item.slug,
            path.read_text(encoding="utf-8"),
            item.license or entry.get("license"),
            item.source or entry.get("source") or UPSTREAM_URL,
        )
    # Local sources carry their licence in `# license:` / `# source:` headers.
    if sources and sources.is_dir():
        for path in sorted(sources.iterdir()):
            if path.is_file() and not path.name.startswith("."):
                add(path.name, slugify(path.name), path.read_text(encoding="utf-8"), None, None)
    for scheme in schemes:
        scheme.selection_note = legible_selection(scheme)
    slugs: dict[str, str] = {}
    for scheme in schemes:
        if scheme.slug in slugs:
            errors.append(f"{scheme.name}: slug {scheme.slug!r} already used by {slugs[scheme.slug]}")
        slugs[scheme.slug] = scheme.name
    return schemes, errors


def toml_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render_theme(scheme: Scheme) -> str:
    lines = [
        f"# {scheme.name}",
        f"# Converted by scripts/import-themes.py from {UPSTREAM_NAME} ({UPSTREAM_URL}).",
        f"# Theme licence: {scheme.license}. Source: {scheme.source or 'not recorded'}",
        "# See THIRD_PARTY_THEMES.md in this directory.",
    ]
    if scheme.selection_note:
        lines.append(f"# Selection: {scheme.selection_note}.")
    lines += [
        f"name = {toml_string(scheme.name)}",
        f'foreground = "{scheme.foreground}"',
        f'background = "{scheme.background}"',
        f'cursor = "{scheme.cursor}"',
        f'selection = "{scheme.selection}"',
    ]
    if scheme.selection_foreground:
        lines.append(f'selection_foreground = "{scheme.selection_foreground}"')
    lines += [
        "",
        "[palette]",
    ]
    lines += [f'{n} = "{c}"' for n, c in zip(PALETTE_NAMES, scheme.palette)]
    return "\n".join(lines) + "\n"


def render_notice(schemes: list[Scheme], upstream_license: str) -> str:
    rows = "\n".join(
        f"| {s.name} | `{s.slug}` | {s.license} | {s.source or 'not recorded'} |"
        for s in sorted(schemes, key=lambda s: s.slug)
    )
    return f"""\
# Third-party notices: bundled colour themes

The colour themes in this directory were converted by
`scripts/import-themes.py` from {UPSTREAM_NAME}
({UPSTREAM_URL}), plus the schemes kept in `scripts/themes/sources/`, which
were written from their own project's published palette. {UPSTREAM_NAME} is
distributed under the licence below. Each theme's own licence belongs to its author and is listed in the
table; Apache-2.0 themes are covered by the text in `LICENSE-APACHE` at the
repository root.

Only themes whose recorded licence permits redistribution were included.
Themes with an unknown, non-free or copyleft licence were left out.

## {UPSTREAM_NAME} licence

```
{upstream_license.rstrip()}
```

## Themes

| Theme | File | Licence | Source |
|-------|------|---------|--------|
{rows}
"""


def write_output(schemes: list[Scheme], out: Path, upstream_license: str) -> None:
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.toml"):
        old.unlink()
    for scheme in schemes:
        (out / f"{scheme.slug}.toml").write_text(render_theme(scheme), encoding="utf-8")
    (out / "THIRD_PARTY_THEMES.md").write_text(
        render_notice(schemes, upstream_license), encoding="utf-8"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--upstream", required=True, type=Path, help="iTerm2-Color-Schemes checkout")
    parser.add_argument("--selection", type=Path, default=DEFAULT_SELECTION)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)

    selection = parse_selection(args.selection.read_text(encoding="utf-8"))
    schemes, errors = build_schemes(args.upstream, selection, args.sources)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    upstream_license = (args.upstream / "LICENSE").read_text(encoding="utf-8")
    write_output(schemes, args.out, upstream_license)
    print(f"wrote {len(schemes)} themes to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
