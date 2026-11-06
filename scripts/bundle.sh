#!/bin/zsh
# Builds the app (release, arm64) as build/YourHarness Next.app, signed ad hoc.
#   APP=<path.app>    somewhere else (test copies go in test-harness-root/…, never build/,
#                     which may be the copy the user has open)
#   FEATURES=snapshots  YH_SNAPSHOT support, for test copies (not for perf runs)
# Its own bundle ID until the switch, so it never shares permissions or data with the user's
# app (/Applications/YourHarness.app, the frozen Swift 0.0.16).
set -euo pipefail
ROOT="${0:A:h:h}"
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
export PATH="$HOME/.cargo/bin:$PATH"
"$ROOT/scripts/vendor.sh"
(cd "$ROOT" && cargo build --release -p yh-app ${FEATURES:+--features=$FEATURES} 2>&1 | grep -E "^(error|warning: unused)|Finished" -A6 || true)
BIN="$ROOT/target/release/YourHarness"
[[ -x $BIN ]] || { echo "build failed" >&2; exit 1; }
APP="${APP:-$ROOT/build/YourHarness Next.app}"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/YourHarness"
VERSION=$(cat "$ROOT/VERSION")
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>YourHarness</string>
    <key>CFBundleIdentifier</key><string>com.mdovhopo.yourharness.next</string>
    <key>CFBundleName</key><string>YourHarness Next</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1
echo "$APP"
