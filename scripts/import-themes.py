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
# Ultty draws selections as a translucent overlay and clamps the alpha to
# 0.55-0.72, so a fixed 0xC0 keeps the upstream colour readable.
SELECTION_ALPHA = "c0"


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
    palette: list[str]


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
    return {
        "foreground": foreground,
        "background": background,
        "cursor": cursor,
        "selection": selection + SELECTION_ALPHA,
        "palette": [palette[i] for i in range(16)],
        "meta": meta,
    }


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
        f"name = {toml_string(scheme.name)}",
        f'foreground = "{scheme.foreground}"',
        f'background = "{scheme.background}"',
        f'cursor = "{scheme.cursor}"',
        f'selection = "{scheme.selection}"',
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
