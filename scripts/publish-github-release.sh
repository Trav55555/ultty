#!/usr/bin/env bash
# Publish a tag's release files to the GitHub mirror with gh. Used by
# .gitea/workflows/release.yml after the Gitea release is out.
#
# Usage: scripts/publish-github-release.sh --tag v0.1.0 --commit SHA --notes NOTES.md --dir artifacts
#
# Needs GH_TOKEN: a fine-grained token with Contents read/write on the mirror
# repository only (the GH_RELEASE_TOKEN Actions secret). The mirror receives
# the tag by push mirroring, so this waits for it and checks that it names the
# commit that was built. A release already published there (for example by
# the GitHub release workflow) is left alone; a leftover draft fails the job.
set -euo pipefail

die() { echo "publish-github-release: $*" >&2; exit 1; }

REPO="${GITHUB_MIRROR_REPO:-Trav55555/ultty}"
TAG="" COMMIT="" NOTES="" DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?}"; shift 2 ;;
    --commit) COMMIT="${2:?}"; shift 2 ;;
    --notes) NOTES="${2:?}"; shift 2 ;;
    --dir) DIR="${2:?}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[ -n "$TAG" ] && [ -n "$COMMIT" ] && [ -f "$NOTES" ] && [ -d "$DIR" ] ||
  die "usage: $0 --tag TAG --commit SHA --notes FILE --dir DIR"
: "${GH_TOKEN:?GH_TOKEN is required (the GH_RELEASE_TOKEN secret)}"

mirrored=""
for _ in $(seq 1 20); do
  mirrored="$(gh api "repos/$REPO/commits/$TAG" --jq .sha 2>/dev/null || true)"
  [ -n "$mirrored" ] && break
  echo "waiting for $TAG to reach $REPO"
  sleep 30
done
[ -n "$mirrored" ] || die "$TAG never reached $REPO; check the push mirror"
[ "$mirrored" = "$COMMIT" ] || die "$TAG on $REPO is $mirrored, but $COMMIT was built"

if draft="$(gh release view "$TAG" --repo "$REPO" --json isDraft --jq .isDraft 2>/dev/null)"; then
  if [ "$draft" = false ]; then
    echo "::notice::$REPO already has a published $TAG release; leaving its files alone"
    exit 0
  fi
  die "$REPO has a draft $TAG release from an earlier attempt; delete it and re-run"
fi

gh release create "$TAG" --repo "$REPO" --verify-tag --title "$TAG" \
  --notes-file "$NOTES" "$DIR"/*
