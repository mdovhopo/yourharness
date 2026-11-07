#!/bin/zsh
# Cuts a release: scripts/release.sh 0.1.0 [git commit options, e.g. --trailer "..."]
#
# 1. Runs the tests (cargo test --release).
# 2. Files CHANGELOG.md's Unreleased notes under the new version, sets VERSION, commits
#    "Release <version>" and tags v<version>, locally.
# 3. Builds the signed app and its DMG as the release workflow does (RELEASE=1
#    scripts/bundle.sh, scripts/dmg.sh, scripts/check-dmg.sh); if that fails, the commit and
#    the tag are taken back and nothing is pushed.
# 4. Pushes main and the tag. GitHub Actions (.github/workflows/release.yml) then builds the
#    DMG again and publishes the release with those notes, which every installed copy's
#    updater picks up.
# 5. Installs the app it built in /Applications.
# From main only: nothing is released from `rewrite` until it's the whole app.
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
git merge-base --is-ancestor origin/main HEAD || die "main is behind origin/main: pull first."
[[ -z "$(git rev-parse -q --verify "refs/tags/v$NEW")" ]] || die "v$NEW is already tagged."
CURRENT="$(<VERSION)"
[[ "$(printf '%s\n' "$CURRENT" "$NEW" | sort -V | tail -1)" == "$NEW" ]] || die "$NEW is older than $CURRENT."
[[ -n "$(scripts/release-notes.sh Unreleased)" ]] || die "Nothing under ## [Unreleased] in CHANGELOG.md."

export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
export PATH="$HOME/.cargo/bin:$PATH"
scripts/vendor.sh
cargo test --release --locked

print "$NEW" > VERSION
DATE="$(date +%Y-%m-%d)" NEW="$NEW" perl -0pi -e 's/^## \[Unreleased\]\n/## [Unreleased]\n\n## [$ENV{NEW}] - $ENV{DATE}\n/m' CHANGELOG.md
[[ -n "$(scripts/release-notes.sh "$NEW")" ]] || die "CHANGELOG.md has no notes under $NEW."
git add VERSION CHANGELOG.md
git commit --quiet -m "Release $NEW" "$@"
git tag -a "v$NEW" -m "YourHarness $NEW"

# Until it's pushed, a failure takes the release commit and the tag back (the tree was clean,
# and the build only writes ignored folders).
undo() {
    echo "Not released: taking back the v$NEW commit and tag." >&2
    git tag -d "v$NEW" >/dev/null
    git reset --quiet --hard HEAD^
}
trap undo ERR
RELEASE=1 scripts/bundle.sh >/dev/null
scripts/dmg.sh
if [[ -f "${YH_SIGNING_DIR:-$HOME/.yourharness/signing}/signing.p12" ]]; then
    scripts/check-dmg.sh "build/YourHarness-$NEW.dmg" "YourHarness Self-Signed"
else
    scripts/check-dmg.sh "build/YourHarness-$NEW.dmg"
fi
git push --atomic origin main "v$NEW"
trap - ERR

scripts/install.sh --no-build
echo "Released $NEW: https://github.com/mdovhopo/yourharness/actions builds the DMG."
