#!/bin/zsh
# Installs the app as your main copy in /Applications: builds build/YourHarness.app
# (skip with --no-build) and swaps it in. Your threads and settings live in
# ~/Library/Application Support, so they carry over.
set -euo pipefail

ROOT="${0:A:h:h}"
APP="$ROOT/build/YourHarness.app"
DEST="/Applications/YourHarness.app"

[[ "${1:-}" == "--no-build" ]] || "$ROOT/scripts/bundle.sh"

# Copy next to it, then swap, so a copy that's running never sees a half-written bundle.
NEXT="/Applications/.YourHarness.app.new"
rm -rf "$NEXT"
ditto "$APP" "$NEXT"
rm -rf "$DEST"
mv "$NEXT" "$DEST"

VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$DEST/Contents/Info.plist")"
echo "Installed YourHarness $VERSION in $DEST"
if pgrep -qf "^$DEST/Contents/MacOS/YourHarness"; then
    echo "It's running: quit and reopen it to use the new version."
fi
