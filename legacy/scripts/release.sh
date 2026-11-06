#!/bin/zsh
# Cuts a release: scripts/release.sh 0.0.2 [git commit options, e.g. --trailer "..."]
#
# 1. Files CHANGELOG.md's Unreleased notes under the new version and sets VERSION.
# 2. Runs the tests, commits "Release <version>", tags v<version>, pushes main and the tag.
#    GitHub Actions then builds the DMG and publishes the release with those notes.
# 3. Installs the build in /Applications.
set -euo pipefail

ROOT="${0:A:h:h}"
NEW="${1:-}"
(( $# )) && shift
die() { echo "$1" >&2; exit 1 }

[[ "$NEW" =~ '^[0-9]+\.[0-9]+\.[0-9]+$' ]] || die "usage: scripts/release.sh <major.minor.patch>"
cd "$ROOT"
[[ "$(git branch --show-current)" == main ]] || die "Release from main."
[[ -z "$(git status --porcelain)" ]] || die "Commit or stash your changes first."
git fetch --quiet --tags origin
[[ -z "$(git rev-parse -q --verify "refs/tags/v$NEW")" ]] || die "v$NEW is already tagged."
CURRENT="$(<VERSION)"
[[ "$(printf '%s\n' "$CURRENT" "$NEW" | sort -V | tail -1)" == "$NEW" ]] || die "$NEW is older than $CURRENT."
[[ -n "$(scripts/release-notes.sh Unreleased)" ]] || die "Nothing under ## [Unreleased] in CHANGELOG.md."

if [[ -z "${DEVELOPER_DIR:-}" && -d /Applications/Xcode.app ]]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi
swift test

print "$NEW" > VERSION
DATE="$(date +%Y-%m-%d)" NEW="$NEW" perl -0pi -e 's/^## \[Unreleased\]\n/## [Unreleased]\n\n## [$ENV{NEW}] - $ENV{DATE}\n/m' CHANGELOG.md
git add VERSION CHANGELOG.md
git commit --quiet -m "Release $NEW" "$@"
git tag -a "v$NEW" -m "YourHarness $NEW"
git push --atomic origin main "v$NEW"

scripts/install.sh
echo "Released $NEW: https://github.com/mdovhopo/yourharness/actions builds the DMG."
