#!/usr/bin/env bash
set -euo pipefail
root=/home/travers/projects/porygon-ai/ux-workspace-verification
source_root=${PORYGON_SOURCE_ROOT:-/home/travers/projects/porygon-ai/porygon-history-pages/porygon-ai}
label=${1:?usage: run.sh label frozen-binary [80|160]}
binary=${2:?usage: run.sh label frozen-binary [80|160]}
width=${3:-80}
export PORYGON_SOURCE_ROOT="$source_root" PORYGON_ARTIFACT_ROOT="$root"
export PORYGON_NATIVE_TEST_BINARY="$binary" PORYGON_ACCEPTANCE_WIDTH="$width"
export PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 UV_OFFLINE=1
export UV_CACHE_DIR=/tmp/porygon-native-landing-verifier/uv-cache
export TMPDIR=/home/travers/projects/porygon-ai/ux-tmp
export TIKTOKEN_CACHE_DIR=/tmp/data-gym-cache
mkdir -p "$TMPDIR"
log="$root/receipts/$label-$width.log"
frozen="$root/receipts/$label-$width-fixtures"
mkdir "$frozen"
cp "$root/test_workspace_pty.py" "$root/compose_support.py" "$frozen/"
sha256sum "$binary" "$frozen/test_workspace_pty.py" "$frozen/compose_support.py" >"$log.sha256"
git -C "$source_root" rev-parse HEAD >"$log.daemon-revision"
git -C "$source_root" status --short >"$log.daemon-status"
set +e
uv run --no-project /home/travers/projects/porygon-ai/porygon-ai/.venv/bin/python \
  -m pytest -p pytest_asyncio.plugin -p pytest_timeout -p xdist.plugin -n0 \
  -c "$source_root/pytest.ini" -vv -s "$frozen/test_workspace_pty.py" >"$log" 2>&1
status=$?
set -e
printf '%s\n' "$status" >"$log.exit"
cat "$log"
exit "$status"
