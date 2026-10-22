#!/bin/zsh
# Builds YourHarness.app (Apple Silicon only) into ./build.
# Usage: scripts/bundle.sh [debug|release]   (default: release)
set -euo pipefail

CONFIG="${1:-release}"
ROOT="${0:A:h:h}"
APP="$ROOT/build/YourHarness.app"

# Use full Xcode even when xcode-select points at the Command Line Tools.
if [[ -z "${DEVELOPER_DIR:-}" && -d /Applications/Xcode.app ]]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

cd "$ROOT"
# The marketing version is VERSION (semver); the build number counts commits, so every
# build of a later commit sorts after the last.
VERSION="$(<"$ROOT/VERSION")"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 1)"

swift build -c "$CONFIG" --arch arm64
BIN="$(swift build -c "$CONFIG" --arch arm64 --show-bin-path)/YourHarness"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/YourHarness"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>YourHarness</string>
    <key>CFBundleDisplayName</key><string>YourHarness</string>
    <key>CFBundleIdentifier</key><string>com.mdovhopo.yourharness</string>
    <key>CFBundleExecutable</key><string>YourHarness</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>LSArchitecturePriority</key><array><string>arm64</string></array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticTermination</key><false/>
</dict>
</plist>
PLIST

# Signed with the app's own identity when it's here (YH_SIGN_IDENTITY and YH_SIGN_KEYCHAIN
# from scripts/signing-keychain.sh, as the release workflow does, or ~/.yourharness/signing):
# every build is then the same app to macOS, which keeps the permissions you gave it, and
# the updater only installs builds signed the same way. Otherwise ad hoc.
SIGNING="${YH_SIGNING_DIR:-$HOME/.yourharness/signing}"
IDENTITY="${YH_SIGN_IDENTITY:-}"
KEYCHAIN="${YH_SIGN_KEYCHAIN:-}"
if [[ -z "$IDENTITY" && -f "$SIGNING/signing.p12" ]]; then
    KEYCHAIN="$SIGNING/signing.keychain-db"
    if ! security unlock-keychain -p "$(<"$SIGNING/keychain-password")" "$KEYCHAIN" 2>/dev/null; then
        "$ROOT/scripts/signing-keychain.sh" "$SIGNING/signing.p12" "$(<"$SIGNING/p12-password")" "$KEYCHAIN" "$(<"$SIGNING/keychain-password")" >/dev/null
    fi
    IDENTITY="$(security find-certificate -a -Z "$KEYCHAIN" | awk '/SHA-1 hash:/ { print $3; exit }')"
fi
if [[ -n "$IDENTITY" ]]; then
    codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$APP" >/dev/null
    SIGNED="signed as $(security find-certificate -a -Z "$KEYCHAIN" | awk -F'"' '/"labl"/ { print $4; exit }')"
else
    codesign --force --sign - "$APP" >/dev/null
    SIGNED="signed ad hoc"
fi
echo "Built $APP ($VERSION, build $BUILD, $SIGNED)"
