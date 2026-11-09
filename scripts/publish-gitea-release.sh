#!/usr/bin/env bash
# Create (or complete) the Gitea release for a tag and attach every file in a
# directory. Used by .gitea/workflows/release.yml.
#
# Usage: scripts/publish-gitea-release.sh --tag v0.1.0 --notes NOTES.md --dir artifacts
#
# Needs GITEA_TOKEN (the job's automatic token), GITHUB_SERVER_URL and
# GITHUB_REPOSITORY, which Gitea Actions sets. Safe to re-run: an existing
# release is reused and files it already has are not uploaded again.
set -euo pipefail

die() { echo "publish-gitea-release: $*" >&2; exit 1; }

TAG="" NOTES="" DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?}"; shift 2 ;;
    --notes) NOTES="${2:?}"; shift 2 ;;
    --dir) DIR="${2:?}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[ -n "$TAG" ] && [ -f "$NOTES" ] && [ -d "$DIR" ] || die "usage: $0 --tag TAG --notes FILE --dir DIR"
: "${GITEA_TOKEN:?GITEA_TOKEN is required}"
API="${GITHUB_SERVER_URL:?}/api/v1/repos/${GITHUB_REPOSITORY:?}"

# The token goes to curl on a config stream, never on its command line.
api() {
  curl -fsS --config <(printf 'header = "Authorization: token %s"\n' "$GITEA_TOKEN") "$@"
}

release="$(mktemp)"
trap 'rm -f "$release"' EXIT
status="$(api -o "$release" -w '%{http_code}' "$API/releases/tags/$TAG" 2>/dev/null || true)"
case "$status" in
  200) echo "reusing the Gitea release for $TAG" ;;
  404)
    jq -n --arg tag "$TAG" --rawfile body "$NOTES" \
      '{tag_name: $tag, name: $tag, body: $body, draft: false, prerelease: false}' |
      api -o "$release" -H 'Content-Type: application/json' --data-binary @- "$API/releases"
    echo "created the Gitea release for $TAG"
    ;;
  *) die "looking up the release for $TAG returned HTTP $status" ;;
esac

id="$(jq -r .id "$release")"
for file in "$DIR"/*; do
  [ -f "$file" ] || continue
  name="$(basename "$file")"
  if jq -e --arg n "$name" '.assets // [] | any(.name == $n)' "$release" >/dev/null; then
    echo "already attached: $name"
    continue
  fi
  api -o /dev/null -F "attachment=@$file" "$API/releases/$id/assets?name=$name"
  echo "attached: $name"
done
