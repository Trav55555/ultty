#!/usr/bin/env bash
set -euo pipefail
root=${PORYGON_VERIFIER_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)}
artifact_root=${PORYGON_ARTIFACT_ROOT:-$root}
source_root=${PORYGON_SOURCE_ROOT:-/home/travers/projects/porygon-ai/porygon-history-pages/porygon-ai}
label=${1:?usage: run-native.sh label frozen-porygon [frozen-ultty frozen-ulttyctl]}
export PORYGON_NATIVE_TEST_BINARY=${2:?supply frozen Porygon binary}
export ULTTY_BINARY=${3:-/tmp/porygon-compose-verification/ultty-v3-binary}
export ULTTYCTL_BINARY=${4:-/tmp/porygon-compose-verification/ulttyctl-v3-binary}
export PORYGON_SOURCE_ROOT="$source_root" PORYGON_ARTIFACT_ROOT="$artifact_root"
export PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 UV_OFFLINE=1
export UV_CACHE_DIR=${UV_CACHE_DIR:-$artifact_root/uv-cache}
export TMPDIR=${TMPDIR:-$artifact_root/tmp}
export TIKTOKEN_CACHE_DIR=${TIKTOKEN_CACHE_DIR:-/tmp/data-gym-cache}
verifier_python=${PORYGON_VERIFIER_PYTHON:-/home/travers/projects/porygon-ai/porygon-ai/.venv/bin/python}
mkdir -p "$TMPDIR" "$artifact_root/receipts"
log="$artifact_root/receipts/$label-native.log"
frozen="$artifact_root/receipts/$label-native-fixtures"
mkdir "$frozen"
cp "$root/test_workspace_native.py" "$frozen/"
sha256sum "$PORYGON_NATIVE_TEST_BINARY" "$ULTTY_BINARY" "$ULTTYCTL_BINARY" "$frozen/test_workspace_native.py" >"$log.sha256"
git -C "$source_root" rev-parse HEAD >"$log.daemon-revision"
git -C "$source_root" status --short >"$log.daemon-status"
set +e
uv run --no-project "$verifier_python" \
  -m pytest -p pytest_asyncio.plugin -p pytest_timeout -p xdist.plugin -n0 \
  -c "$source_root/pytest.ini" -vv -s "$frozen/test_workspace_native.py" >"$log" 2>&1
status=$?
set -e
printf '%s\n' "$status" >"$log.exit"
cat "$log"
exit "$status"
