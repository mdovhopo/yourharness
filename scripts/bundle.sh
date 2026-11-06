#!/bin/zsh
# Builds the web experiment as build/YourHarness Next.app: the page (app/ui → Resources/ui)
# and the host (release, arm64), signed ad hoc. Its own bundle ID, so it never shares
# permissions, defaults or web data with YourHarness.
set -euo pipefail
ROOT="${0:A:h:h}"
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
(cd "$ROOT/app/ui" && { [[ -d node_modules ]] || pnpm install --frozen-lockfile; } && pnpm build >/dev/null)
(cd "$ROOT" && swift build -c release --product YourHarness 2>&1 | grep -E "error|warning: .*web/host|Compiling YourHarness|Build complete" || true)
BIN="$ROOT/.build/release/YourHarness"
[[ -x $BIN ]] || { echo "build failed" >&2; exit 1; }
# APP=<path>: somewhere else, so a test build never replaces the copy you have open.
APP="${APP:-$ROOT/build/YourHarness Next.app}"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/YourHarness"
cp -R "$ROOT/app/ui/dist" "$APP/Contents/Resources/ui"
VERSION=$(cat "$ROOT/VERSION")
cat > "$APP/Contents/Info.plist" <<EOF
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
EOF
codesign --force --sign - "$APP" >/dev/null
echo "$APP"
