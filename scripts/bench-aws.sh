#!/usr/bin/env bash
# Run the benchmark suite on an AWS instance that is already up and reachable
# over SSH. This script never launches, stops or terminates instances.
#
#   scripts/bench-aws.sh --host HOST --user USER --key KEY --role gpu|cpu [options]
#
#   --role gpu   NVIDIA instance (g4dn.xlarge: T4, or g5.xlarge: A10G), Ubuntu
#                24.04. Installs the NVIDIA driver (rebooting once if needed),
#                Ghostty, GNOME Terminal and Alacritty, then runs the comparative
#                suite under X11 (headless Xorg on the GPU) and Wayland (headless
#                sway on the GPU), plus headless VT numbers.
#   --role cpu   Dedicated-core instance (c7a.2xlarge or larger), Ubuntu 24.04.
#                Runs the headless VT suite with an A/A noise floor and records
#                instruction counts for this toolchain.
#
# Options:
#   --ref REF           git ref to build (default HEAD); sent as a git bundle,
#                       so the instance needs no access to the repository
#   --port N            SSH port (default 22)
#   --out DIR           local results directory (default target/bench-results/aws-<role>-<date>)
#   --compare-args STR  extra arguments for scripts/bench/compare.sh (gpu role),
#                       e.g. "--rounds 5 --k2p-samples 300"
#   --runs N            headless blocks (default 10)
#   --skip-setup        assume the instance is already set up
#   --known-hosts FILE  record the instance's host key in FILE instead of
#                       ~/.ssh/known_hosts (useful for throwaway instances)
#
# Idempotent: setup skips what is installed, the build reuses the checkout and
# target directory, and each run writes a new timestamped results directory.
# Any failed step stops the script with a non-zero exit.
#
# Results come back as DIR/<stamp>/ (results.jsonl, versions.json, report.md
# per session) and DIR/REPORT.md, which concatenates the per-session reports.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
die() { echo "bench-aws: $*" >&2; exit 1; }
log() { echo "[bench-aws $(date +%T)] $*" >&2; }

HOST="" USER_NAME="" KEY="" ROLE="" REF=HEAD PORT=22 OUT="" COMPARE_ARGS="" RUNS=10 SETUP=1 KNOWN_HOSTS=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) HOST="${2:?}"; shift 2 ;;
    --user) USER_NAME="${2:?}"; shift 2 ;;
    --key) KEY="${2:?}"; shift 2 ;;
    --role) ROLE="${2:?}"; shift 2 ;;
    --ref) REF="${2:?}"; shift 2 ;;
    --port) PORT="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --compare-args) COMPARE_ARGS="${2?}"; shift 2 ;;
    --runs) RUNS="${2:?}"; shift 2 ;;
    --skip-setup) SETUP=0; shift ;;
    --known-hosts) KNOWN_HOSTS="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done
[[ -n "$HOST" && -n "$USER_NAME" && -n "$KEY" ]] || die "--host, --user and --key are required"
[[ "$ROLE" == gpu || "$ROLE" == cpu ]] || die "--role must be gpu or cpu"
[[ -r "$KEY" ]] || die "cannot read key $KEY"
[[ "$RUNS" =~ ^[0-9]+$ ]] || die "--runs must be a number"
SHA=$(git -C "$ROOT" rev-parse --verify "$REF^{commit}") || die "unknown ref $REF"
OUT="${OUT:-$ROOT/target/bench-results/aws-$ROLE-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$OUT"

SSH_OPTS=(-i "$KEY" -p "$PORT" -o BatchMode=yes -o StrictHostKeyChecking=accept-new
  -o ServerAliveInterval=30 -o ConnectTimeout=15)
SCP_OPTS=(-i "$KEY" -P "$PORT" -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
if [[ -n "$KNOWN_HOSTS" ]]; then
  SSH_OPTS+=(-o "UserKnownHostsFile=$KNOWN_HOSTS")
  SCP_OPTS+=(-o "UserKnownHostsFile=$KNOWN_HOSTS")
fi
TARGET="$USER_NAME@$HOST"
# The command goes over stdin, so it is parsed only by the remote shell.
remote() { ssh "${SSH_OPTS[@]}" "$TARGET" bash -s <<<"$1"; }

wait_for_ssh() {
  local deadline=$(( $(date +%s) + ${1:-600} ))
  until remote true 2>/dev/null; do
    (( $(date +%s) < deadline )) || die "$HOST did not answer SSH in time"
    sleep 10
  done
}

log "checking $TARGET"
wait_for_ssh 120
remote 'mkdir -p ubench'

# Ship the code as a bundle of exactly the commit under test.
BUNDLE="$OUT/ultty-${SHA:0:12}.bundle"
# A bundle carries refs, so REF must be a name (HEAD, a branch or a tag).
git -C "$ROOT" bundle create "$BUNDLE" "$REF" >/dev/null 2>&1 \
  || die "could not bundle $REF; pass a branch, tag or HEAD rather than a bare commit"
git -C "$ROOT" bundle verify "$BUNDLE" >/dev/null || die "bundle verification failed"
scp "${SCP_OPTS[@]}" "$BUNDLE" "$ROOT/scripts/bench/aws-remote.sh" "$TARGET:ubench/"
remote 'chmod +x ubench/aws-remote.sh'

if [[ "$SETUP" == 1 ]]; then
  log "setup ($ROLE)"
  remote "ubench/aws-remote.sh setup $ROLE"
  if remote 'ubench/aws-remote.sh needs-reboot'; then
    log "rebooting $HOST to load the driver"
    remote 'sudo systemctl reboot' || true
    sleep 30
    wait_for_ssh 900
    remote "ubench/aws-remote.sh setup $ROLE"
    if remote 'ubench/aws-remote.sh needs-reboot'; then
      die "still needs a reboot after rebooting; check the driver install on $HOST"
    fi
  fi
fi

log "building $SHA"
remote "ubench/aws-remote.sh build ubench/$(basename "$BUNDLE") $SHA"

log "running the $ROLE suite"
status=0
remote "RUNS=$RUNS COMPARE_ARGS=$(printf '%q' "$COMPARE_ARGS") ubench/aws-remote.sh run $ROLE" || status=$?

STAMP=$(remote 'cat ubench/results/latest' 2>/dev/null || true)
[[ -n "$STAMP" ]] || die "the run produced no results directory (exit $status)"
log "copying results $STAMP"
scp -r "${SCP_OPTS[@]}" "$TARGET:ubench/results/$STAMP" "$OUT/"

{
  echo "# AWS $ROLE run, $(date -u +%Y-%m-%dT%H:%MZ)"
  echo
  echo "- Host: $HOST (role $ROLE)"
  echo "- Revision: $SHA"
  echo "- Remote results: ubench/results/$STAMP"
  echo
  find "$OUT/$STAMP" \( -name report.md -o -name summary.md \) | sort | while read -r f; do
    echo "## ${f#"$OUT/$STAMP/"}"
    echo
    cat "$f"
    echo
  done
} >"$OUT/REPORT.md"

log "results: $OUT/REPORT.md"
(( status == 0 )) || die "the remote run reported failures (exit $status); partial results are in $OUT"
