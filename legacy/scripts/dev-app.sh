#!/bin/zsh
# Copies build/YourHarness.app to <dir>/YourHarness Dev.app with its own name and bundle ID,
# so a test instance never shares permissions, notifications or browser data with your
# installed app. Launch it with YH_DATA_DIR (a scratch state) and YH_PATH (your PATH), e.g.
#   open -g -n --env YH_DATA_DIR=/tmp/state --env YH_PATH="$PATH" "<dir>/YourHarness Dev.app"
# Usage: scripts/dev-app.sh <dir>
set -euo pipefail
ROOT="${0:A:h:h}"
OUT="${1:?usage: scripts/dev-app.sh <dir>}"
APP="$OUT/YourHarness Dev.app"
rm -rf "$APP"
mkdir -p "$OUT"
cp -R "$ROOT/build/YourHarness.app" "$APP"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.mdovhopo.yourharness.dev" \
    -c "Set :CFBundleName YourHarness Dev" -c "Set :CFBundleDisplayName YourHarness Dev" "$APP/Contents/Info.plist"
codesign --force --sign - "$APP" >/dev/null
echo "$APP"
