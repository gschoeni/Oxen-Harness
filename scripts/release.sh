#!/usr/bin/env bash
# Tag the current version(s) and push the tag(s), which triggers the release
# workflows on GitHub Actions.
#
# Usage: scripts/release.sh <cli|app|all> [--yes]
#
# Expects the version bump to already be committed (see bump-version.sh).
# Refuses to run on a dirty tree or when the tag already exists locally or on
# origin. --yes skips the confirmation prompt (used by the cut-release
# workflow).
set -euo pipefail

usage() {
  echo "usage: $0 <cli|app|all> [--yes]" >&2
  exit 2
}

component="${1:-}"
case "$component" in cli | app | all) ;; *) usage ;; esac
assume_yes=false
[[ "${2:-}" == "--yes" ]] && assume_yes=true

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

die() {
  echo "error: $*" >&2
  exit 1
}

git diff-index --quiet HEAD -- || die "working tree is not clean; commit or stash first"

tags=()
if [[ "$component" != "app" ]]; then
  tags+=("cli-v$(scripts/version.sh cli)")
fi
if [[ "$component" != "cli" ]]; then
  tags+=("app-v$(scripts/version.sh app)")
fi

for tag in "${tags[@]}"; do
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null &&
    die "tag $tag already exists locally"
  [[ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]] &&
    die "tag $tag already exists on origin"
done

branch="$(git rev-parse --abbrev-ref HEAD)"
echo "Tagging ${tags[*]} at $(git rev-parse --short HEAD) on branch '$branch'."
if ! $assume_yes; then
  read -r -p "Push to origin and start the release build(s)? [y/N] " answer
  [[ "$answer" == [yY]* ]] || die "aborted"
fi

for tag in "${tags[@]}"; do
  git tag -a "$tag" -m "$tag"
done
git push origin "${tags[@]}"

echo
echo "Pushed: ${tags[*]}"
echo "Watch the release build(s): https://github.com/gschoeni/Oxen-Harness/actions"
