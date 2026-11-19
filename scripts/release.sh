#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
REPO="${MD_PREVIEW_GITHUB_REPO:-galen2027/md-studio}"
VERSION="$(awk -F\" '/^version = / { print $2; exit }' Cargo.toml)"
TAG="${1:-v$VERSION}"
if [[ "$TAG" == "--help" || "$TAG" == "-h" ]]; then
  echo 'Usage: scripts/release.sh [vX.Y.Z]'
  echo 'Verify, push source/tag to origin, then dispatch the Windows draft build.'
  echo 'Requires GitHub CLI authentication. Review and publish the draft separately.'
  exit 0
fi
[[ "$TAG" == "v$VERSION" ]] || { echo 'Tag and Cargo version differ.' >&2; exit 1; }
[[ "$(git branch --show-current)" == master ]] || { echo 'Release from master.' >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo 'Commit or ignore pending changes first.' >&2; exit 1; }
command -v gh >/dev/null || { echo 'GitHub CLI (gh) is required for draft workflow dispatch.' >&2; exit 1; }
./scripts/verify.sh
if git rev-parse --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  [[ "$(git rev-parse "$TAG^{}")" == "$(git rev-parse HEAD)" ]] || { echo 'Existing tag points elsewhere; never overwrite a release tag.' >&2; exit 1; }
else
  git tag -a "$TAG" -m "Markdown Studio $TAG"
fi
git push --atomic -u origin master "refs/tags/$TAG"
gh workflow run release.yml --repo "$REPO" --ref master -f "tag=$TAG"
echo "Windows draft build dispatched: https://github.com/$REPO/actions"
echo 'Review SHA-256, licenses and native behavior before publishing the draft.'
