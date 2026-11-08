#!/usr/bin/env bash
# Run install.sh against a fake release served from localhost and check what
# it installs, including the desktop entry's Exec= line for an install
# directory full of characters that need quoting, and the terminfo entry.
# Also check that it refuses platforms with no release build.
#
#   scripts/install-sh-test.sh [path/to/install.sh]
#
# Linux only (the desktop entry is Linux only). Needs python3 and curl.
set -euo pipefail

INSTALL_SH="$(realpath "${1:-$(dirname "$0")/../install.sh}")"
[ "$(uname -s)" = Linux ] || { echo "skip: Linux only"; exit 0; }

work="$(mktemp -d)"
server_pid=""
cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# Platforms without a release build fail before downloading anything.
mkdir -p "$work/fake-uname"
cat >"$work/fake-uname/uname" <<'SH'
#!/bin/sh
case "$1" in
  -s) echo Darwin ;;
  -m) echo arm64 ;;
esac
SH
chmod 755 "$work/fake-uname/uname"
if out="$(PATH="$work/fake-uname:$PATH" HOME="$work/home" ULTTY_VERSION=v0.0.0-test \
  ULTTY_RELEASES="http://127.0.0.1:9" sh "$INSTALL_SH" 2>&1)"; then
  echo "install.sh accepted Darwin arm64: $out" >&2
  exit 1
fi
case "$out" in
  *"Linux x86_64 only"*) ;;
  *) echo "unclear error for an unsupported platform: $out" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) ;;
  *) echo "skip: release builds are x86_64 only"; exit 0 ;;
esac
version=v0.0.0-test
archive="ultty-$version-x86_64-unknown-linux-gnu.tar.gz"

# A fake release: two stub binaries, the real desktop entry and terminfo.
stage="$work/stage"
mkdir -p "$stage/dist/terminfo" "$stage/dist/shell-integration" "$work/www/download/$version"
for bin in ultty ulttyctl; do
  printf '#!/bin/sh\necho %s\n' "$bin" >"$stage/$bin"
done
cp "$(dirname "$INSTALL_SH")/dist/ultty.desktop" "$stage/dist/ultty.desktop"
cp "$(dirname "$INSTALL_SH")/dist/terminfo/ultty.terminfo" "$stage/dist/terminfo/"
cp "$(dirname "$INSTALL_SH")"/dist/shell-integration/ultty.* "$stage/dist/shell-integration/"
tar czf "$work/www/download/$version/$archive" -C "$stage" .
(cd "$work/www/download/$version" && sha256sum "$archive" >SHA256SUMS)

python3 -I -u -m http.server 0 --bind 127.0.0.1 --directory "$work/www" \
  >"$work/server.log" 2>&1 &
server_pid=$!
port=""
for _ in $(seq 50); do
  port="$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$work/server.log" | head -1)"
  [ -n "$port" ] && break
  sleep 0.1
done
[ -n "$port" ] || { echo "http server did not start" >&2; exit 1; }

# Spaces, sed delimiters and specials, and every character the Desktop
# Entry spec says must be escaped inside a quoted argument.
install_dir="$work/odd dir|a&b\\c\"d\$e\`f%g/bin"
data_dir="$work/data"
HOME="$work/home" XDG_DATA_HOME="$data_dir" \
  ULTTY_RELEASES="http://127.0.0.1:$port" ULTTY_VERSION="$version" \
  ULTTY_INSTALL_DIR="$install_dir" sh "$INSTALL_SH"

for bin in ultty ulttyctl; do
  [ "$("$install_dir/$bin")" = "$bin" ] || { echo "$bin not installed" >&2; exit 1; }
done

for script in ultty.bash ultty.zsh; do
  cmp "$data_dir/ultty/shell-integration/$script" \
    "$(dirname "$INSTALL_SH")/dist/shell-integration/$script" ||
    { echo "shell integration $script not installed" >&2; exit 1; }
done

if command -v tic >/dev/null 2>&1; then
  TERMINFO="$work/home/.terminfo" infocmp -x xterm-ultty >/dev/null ||
    { echo "xterm-ultty terminfo entry not installed" >&2; exit 1; }
fi

entry="$data_dir/applications/ultty.desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$entry"
fi

# Decode Exec= the way a launcher does and compare with the real path.
python3 -I - "$entry" "$install_dir/ultty" <<'PY'
import sys

entry, expected = sys.argv[1], sys.argv[2]
line = next(l for l in open(entry, encoding="utf-8") if l.startswith("Exec="))
value = line[len("Exec="):].rstrip("\n")

# String-value escapes first.
out, i = [], 0
while i < len(value):
    c = value[i]
    if c == "\\" and i + 1 < len(value):
        out.append({"s": " ", "n": "\n", "t": "\t", "r": "\r", "\\": "\\"}.get(value[i + 1], "\\" + value[i + 1]))
        i += 2
    else:
        out.append(c)
        i += 1
value = "".join(out)

# Then the Exec quoting rules: split arguments, honour double quotes.
args, cur, quoted, i = [], [], False, 0
while i < len(value):
    c = value[i]
    if quoted:
        if c == "\\" and i + 1 < len(value) and value[i + 1] in '"`$\\':
            cur.append(value[i + 1])
            i += 1
        elif c == '"':
            quoted = False
        else:
            cur.append(c)
    elif c == '"':
        quoted = True
    elif c == " ":
        if cur:
            args.append("".join(cur))
            cur = []
    else:
        cur.append(c)
    i += 1
if cur:
    args.append("".join(cur))
program = args[0].replace("%%", "%")
if program != expected:
    sys.exit(f"Exec= decodes to {program!r}, expected {expected!r}\n  line: {line!r}")
print("Exec= decodes to the installed binary")
PY
echo "install.sh test passed"
