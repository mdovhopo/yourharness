#!/bin/zsh
# Packs build/YourHarness.app into build/YourHarness-<version>.dmg: the app next to an
# Applications link to drag it onto. Run scripts/bundle.sh first.
set -euo pipefail

ROOT="${0:A:h:h}"
APP="$ROOT/build/YourHarness.app"
[[ -d "$APP" ]] || { echo "No $APP: run scripts/bundle.sh first." >&2; exit 1 }

VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="$ROOT/build/YourHarness-$VERSION.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

ditto "$APP" "$STAGE/YourHarness.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -ov -volname "YourHarness $VERSION" -srcfolder "$STAGE" -format UDZO "$DMG"
echo "Built $DMG"
