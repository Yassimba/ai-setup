#!/usr/bin/env bash
set -euo pipefail

: "${TAG:?TAG is required}"
: "${RELEASE_SHA:?RELEASE_SHA is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"

if existing_sha="$(git rev-list -n 1 "$TAG" 2>/dev/null)"; then
  if [[ "$existing_sha" != "$RELEASE_SHA" ]]; then
    echo "release tag $TAG points to $existing_sha, expected $RELEASE_SHA" >&2
    exit 1
  fi
else
  git tag "$TAG" "$RELEASE_SHA"
  git push origin "refs/tags/$TAG"
fi

gh release view "$TAG" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1 \
  || gh release create "$TAG" --repo "$GITHUB_REPOSITORY" \
       --verify-tag --generate-notes --title "$TAG"
