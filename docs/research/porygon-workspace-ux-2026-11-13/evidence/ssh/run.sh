#!/usr/bin/env bash
set -euo pipefail
root=/home/travers/projects/porygon-ai/ux-workspace-verification
source_root=${PORYGON_SOURCE_ROOT:-/home/travers/projects/porygon-ai/porygon-ux-land/porygon-ai}
label=${1:?label required}
binary=${2:?frozen binary required}
width=${3:?width required}
export PORYGON_SOURCE_ROOT="$source_root" PORYGON_ARTIFACT_ROOT="$root/ssh"
export PORYGON_NATIVE_TEST_BINARY="$binary" PORYGON_ACCEPTANCE_WIDTH="$width"
export PYTHONPATH="$root:$source_root/tests:$source_root/src" PYTEST_DISABLE_PLUGIN_AUTOLOAD=1
export UV_OFFLINE=1 UV_CACHE_DIR=/tmp/porygon-native-landing-verifier/uv-cache
mkdir -p "$root/ssh/receipts"
log="$root/ssh/receipts/$label-$width.log"
sha256sum "$binary" "$root/ssh/server.py" "$root/ssh/test_ssh.py" "$root/compose_support.py" > "$log.sha256"
set +e
uv run --no-project /home/travers/projects/porygon-ai/porygon-ai/.venv/bin/python -m pytest -p pytest_asyncio.plugin -p pytest_timeout -p xdist.plugin -n0 -c "$source_root/pytest.ini" -vv -s "$root/ssh/test_ssh.py" > "$log" 2>&1
status=$?
set -e
printf '%s\n' "$status" > "$log.exit"
cat "$log"
exit "$status"
