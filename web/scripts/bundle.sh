#!/bin/zsh
# Builds the web experiment as build/YourHarnessWeb.app: the page (web/ui → Resources/ui)
# and the host (release, arm64), signed ad hoc. Its own bundle ID, so it never shares
# permissions, defaults or web data with YourHarness.
set -euo pipefail
ROOT="${0:A:h:h:h}"
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
(cd "$ROOT/web/ui" && { [[ -d node_modules ]] || pnpm install --frozen-lockfile; } && pnpm build >/dev/null)
(cd "$ROOT" && swift build -c release --product YourHarnessWeb 2>&1 | grep -E "error|warning: .*web/host|Compiling YourHarnessWeb|Build complete" || true)
BIN="$ROOT/.build/release/YourHarnessWeb"
[[ -x $BIN ]] || { echo "build failed" >&2; exit 1; }
# APP=<path>: somewhere else, so a test build never replaces the copy you have open.
APP="${APP:-$ROOT/build/YourHarnessWeb.app}"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/YourHarnessWeb"
cp -R "$ROOT/web/ui/dist" "$APP/Contents/Resources/ui"
VERSION=$(cat "$ROOT/VERSION")
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>YourHarnessWeb</string>
    <key>CFBundleIdentifier</key><string>com.mdovhopo.yourharness.web</string>
    <key>CFBundleName</key><string>YourHarness Web</string>
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
