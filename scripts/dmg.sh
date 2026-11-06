#!/bin/zsh
# Packs build/YourHarness.app (RELEASE=1 scripts/bundle.sh first) into
# build/YourHarness-<version>.dmg, with its .sha256: the app next to an Applications link.
set -euo pipefail
ROOT="${0:A:h:h}"
APP="${APP:-$ROOT/build/YourHarness.app}"
[[ -d "$APP" ]] || { echo "No $APP: run RELEASE=1 scripts/bundle.sh first." >&2; exit 1 }
VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist")"
OUT="${OUT:-$ROOT/build}"
DMG="$OUT/YourHarness-$VERSION.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/YourHarness.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -ov -volname "YourHarness $VERSION" -srcfolder "$STAGE" -format UDZO "$DMG"
(cd "$OUT" && shasum -a 256 "YourHarness-$VERSION.dmg" > "YourHarness-$VERSION.dmg.sha256")
echo "Built $DMG"
