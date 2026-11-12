#!/usr/bin/env bash
# Push user overrides into a running native app with `ulttyctl override`
# (ADR 0023 §9) in a fresh Ultty on a virtual X display, and check through
# `ulttyctl regions --json`:
#
#   - a TOML push relabels one button and hides another on the next frame;
#   - a push that does not parse is refused with the app's error, exit 1,
#     and changes nothing;
#   - a Luau push replaces the TOML one;
#   - `override clear` puts the app back;
#   - a pane with no native app is an error naming the pane.
#
#   scripts/e2e/override-push.sh [artifact-dir]
#
# Needs Xvfb and a Vulkan or GL driver (Mesa's lavapipe works without a GPU).
# Uses $ULTTY, $ULTTYCTL and $COUNTER_APP, default target/debug/*, built
# first if unset. Logs and region dumps are left in the artifact dir. Exits
# non-zero when a check fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v Xvfb >/dev/null || { echo "Xvfb is required" >&2; exit 2; }

if [ -z "${ULTTY:-}" ] || [ -z "${ULTTYCTL:-}" ] || [ -z "${COUNTER_APP:-}" ]; then
    cargo build -q --locked -p ultty --bins -p counter-app
    ULTTY=${ULTTY:-target/debug/ultty}
    ULTTYCTL=${ULTTYCTL:-target/debug/ulttyctl}
    COUNTER_APP=${COUNTER_APP:-target/debug/counter-app}
fi
ULTTY=$(realpath "$ULTTY")
ULTTYCTL=$(realpath "$ULTTYCTL")
COUNTER_APP=$(realpath "$COUNTER_APP")

out="${1:-$(mktemp -d)}"
mkdir -p "$out/config/ultty"
out=$(cd "$out" && pwd)
rm -f "$out"/*.json "$out"/*.log "$out"/*.toml "$out"/*.luau "$out"/*.txt

# Unix socket paths are short (108 bytes), so the runtime dir must be too.
runtime=$(mktemp -d /tmp/ultty-ovr.XXXXXX)
chmod 700 "$runtime"
cleanup() {
    kill "${ultty_pid-}" "${xvfb_pid-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT

printf '[behavior]\ncursor_blink = false\n' >"$out/config/ultty/config.toml"

# Xvfb picks a free display and writes its number to fd 3.
Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$out/display" 2>"$out/xvfb.log" &
xvfb_pid=$!
for _ in $(seq 50); do [ -s "$out/display" ] && break; sleep 0.1; done
[ -s "$out/display" ] || { echo "Xvfb did not start" >&2; cat "$out/xvfb.log" >&2; exit 1; }
display=":$(cat "$out/display")"

lvp_icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "${VK_ICD_FILENAMES:-}" ] && [ -n "$lvp_icd" ]; then
    export VK_ICD_FILENAMES="$lvp_icd"
fi

# Without unsetting WAYLAND_DISPLAY, a Wayland session in the environment
# wins over DISPLAY and the window opens on the desktop instead of Xvfb.
# XDG_CONFIG_HOME also keeps the app away from the developer's own override
# files.
env -u WAYLAND_DISPLAY \
    DISPLAY="$display" \
    XDG_RUNTIME_DIR="$runtime" \
    XDG_CONFIG_HOME="$out/config" \
    SHELL=/bin/sh \
    PS1='$ ' \
    RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn" \
    "$ULTTY" >"$out/ultty.log" 2>&1 &
ultty_pid=$!

fail() {
    echo "FAIL: $*" >&2
    "$ULTTYCTL" panes >&2 || true
    tail -20 "$out/ultty.log" >&2
    if [ -s "$out/app.log" ]; then
        echo "== app.log" >&2
        tail -20 "$out/app.log" >&2
    fi
    exit 1
}

# Talk only to this Ultty: its own control socket, found through its pid.
export XDG_RUNTIME_DIR="$runtime" ULTTY_PID="$ultty_pid"
unset ULTTY_SOCKET ULTTY_AUTH_TOKEN
for _ in $(seq 600); do
    [ -S "$runtime/ultty-ctl-$ultty_pid.sock" ] && "$ULTTYCTL" ping >/dev/null 2>&1 && break
    kill -0 "$ultty_pid" 2>/dev/null || fail "Ultty exited during startup"
    sleep 0.1
done
"$ULTTYCTL" ping >/dev/null 2>&1 || fail "Ultty never answered on its control socket"
"$ULTTYCTL" wait-for '$' --timeout 20000 >/dev/null || fail "no shell prompt"

# `regions --json` for pane $2's tab, saved as $out/$1.json.
regions() {
    "$ULTTYCTL" --json --pane "$2" regions >"$out/$1.json" 2>/dev/null || true
}
# The button labels of the app in $1.json, as "id=label" pairs, one per
# line, and "id=hidden" for a region laid out with no size.
labels() {
    python3 -c '
import json, sys
reply = json.load(open(sys.argv[1]))
for scene in reply.get("scenes", []):
    for r in scene["regions"]:
        rid = r.get("metadata", {}).get("id")
        if rid in ("dec", "inc", "reset"):
            shown = "hidden" if r["width"] == 0 or r["height"] == 0 else (r.get("text") or "")
            print(f"{rid}={shown}")
' "$out/$1.json" 2>/dev/null | sort | tr '\n' ' '
}
# Poll until the labels of pane $2 are exactly $3 (sorted, space-joined),
# within 10 s; the dump stays in $out/$1.json.
wait_labels() {
    local name=$1 pane=$2 want=$3
    for _ in $(seq 100); do
        regions "$name" "$pane"
        if [ "$(labels "$name")" = "$want" ]; then
            return 0
        fi
        sleep 0.1
    done
    echo "wanted: $want" >&2
    echo "got:    $(labels "$name")" >&2
    return 1
}

# 1. counter-app in a tab of its own draws natively: three buttons by id.
#    Its stderr (the runner's log) goes to app.log for the failure report.
pane=$("$ULTTYCTL" new-tab sh -c "RUST_LOG=info exec '$COUNTER_APP' 2>'$out/app.log'") ||
    fail "new-tab counter-app"
[[ "$pane" =~ ^[0-9]+$ ]] || fail "new-tab printed '$pane', not a pane id"
wait_labels start "$pane" "dec=-1 inc=+1 reset=Reset " ||
    fail "counter-app did not draw its buttons natively"
echo "override-push: counter-app draws natively in pane $pane"

# 2. A TOML push relabels one button and hides another on the next frame.
printf '[widget.inc]\ntext = "More"\n[widget.reset]\nhidden = true\n' >"$out/try.toml"
"$ULTTYCTL" --pane "$pane" override push "$out/try.toml" >"$out/push.txt" ||
    fail "override push failed: $(cat "$out/push.txt")"
grep -qx 'OK' "$out/push.txt" || fail "override push printed '$(cat "$out/push.txt")', not OK"
wait_labels pushed "$pane" "dec=-1 inc=More reset=hidden " ||
    fail "the pushed override did not show"
echo "override-push: TOML push relabelled inc and hid reset"

# 3. A push that does not parse is refused with the app's error and
#    changes nothing. --json gives the same as a structured error.
printf '[widget.inc]\ntext = 3\n' >"$out/bad.toml"
if "$ULTTYCTL" --json --pane "$pane" override push "$out/bad.toml" >"$out/bad.txt"; then
    fail "a bad override was accepted: $(cat "$out/bad.txt")"
fi
python3 -c '
import json, sys
reply = json.load(open(sys.argv[1]))
assert reply["status"] == "error", reply
assert "widget.inc.text" in reply["message"], reply
' "$out/bad.txt" || fail "the bad push was not refused with the parse error: $(cat "$out/bad.txt")"
regions still "$pane"
[ "$(labels still)" = "dec=-1 inc=More reset=hidden " ] ||
    fail "a refused push changed the app: $(labels still)"
echo "override-push: bad TOML refused with the app's error"

# 4. A Luau push replaces the TOML one (counter-app is built with
#    lua-overrides).
cat >"$out/try.luau" <<'EOF'
return {
  widget = function(id, info)
    if id == "dec" then return { text = "Less" } end
  end,
}
EOF
"$ULTTYCTL" --pane "$pane" override push "$out/try.luau" >"$out/luau.txt" ||
    fail "override push of a Luau file failed: $(cat "$out/luau.txt")"
wait_labels luau "$pane" "dec=Less inc=+1 reset=Reset " ||
    fail "the Luau push did not replace the TOML one"
echo "override-push: Luau push replaced the TOML push"

# 5. clear puts the app back.
"$ULTTYCTL" --pane "$pane" override clear >/dev/null || fail "override clear failed"
wait_labels cleared "$pane" "dec=-1 inc=+1 reset=Reset " ||
    fail "override clear did not restore the app"
echo "override-push: clear restored the buttons"

# 6. A pane with no native app (the shell in pane 0) is an error naming it.
if "$ULTTYCTL" --pane 0 override clear >"$out/shell.txt" 2>&1; then
    fail "override clear on a shell pane succeeded"
fi
grep -q "No native app in pane 0" "$out/shell.txt" ||
    fail "the shell pane's error does not name it: $(cat "$out/shell.txt")"
echo "override-push: a pane without a native app is refused"

# 7. A shell pane split off next to the app, in the app's own tab, has no
#    native session bound to it (ADR 0025) and none in its foreground: a
#    push aimed at it is refused rather than landing in the app, which
#    keeps drawing unchanged.
split=$("$ULTTYCTL" --pane "$pane" split right) || fail "split next to the app"
[[ "$split" =~ ^[0-9]+$ ]] || fail "split printed '$split', not a pane id"
if "$ULTTYCTL" --pane "$split" override push "$out/try.toml" >"$out/split.txt" 2>&1; then
    fail "a push aimed at the split shell pane was accepted"
fi
grep -q "No native app in pane $split" "$out/split.txt" ||
    fail "the split pane's error does not name it: $(cat "$out/split.txt")"
regions split "$pane"
[ "$(labels split)" = "dec=-1 inc=+1 reset=Reset " ] ||
    fail "the refused push reached the app anyway: $(labels split)"
echo "override-push: the shell pane split beside the app is refused"

"$ULTTYCTL" --pane "$pane" send-text q >/dev/null 2>&1 || true
echo "override-push: ok"
