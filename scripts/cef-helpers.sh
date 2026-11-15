#!/bin/zsh
# Puts the Chrome engine's helper apps in a built bundle: scripts/cef-helpers.sh <app> [identity
# keychain]. CEF starts its processes from `Contents/Frameworks/<exe> Helper[ (GPU|Renderer|Plugin|
# Alerts)].app`, one binary (yh-cef-helper) in each. Signed, then the app again around them
# (SKIP_OUTER_SIGN=1: bundle.sh signs it). Ad hoc without an identity.
set -euo pipefail
ROOT="${0:A:h:h}"
APP="$1" IDENTITY="${2:-}" KEYCHAIN="${3:-}"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/yh-cef-helper"
[[ -x $BIN ]] || { echo "no $BIN: cargo build --release -p yh-cef-helper" >&2; exit 1; }
EXE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
BASE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
sign() {
    if [[ -n "$IDENTITY" ]]; then codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$1" >/dev/null
    else codesign --force --sign - "$1" >/dev/null 2>&1; fi
}
mkdir -p "$APP/Contents/Frameworks"
for flavor in "" " (GPU)" " (Renderer)" " (Plugin)" " (Alerts)"; do
    name="$EXE Helper$flavor"
    suffix="${${${flavor// /}//[()]/}:l}"
    helper="$APP/Contents/Frameworks/$name.app"
    rm -rf "$helper"
    mkdir -p "$helper/Contents/MacOS"
    cp "$BIN" "$helper/Contents/MacOS/$name"
    cat > "$helper/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundleIdentifier</key><string>$BASE_ID.helper${suffix:+.$suffix}</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>LSUIElement</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
    sign "$helper"
done
[[ -n "${SKIP_OUTER_SIGN:-}" ]] || sign "$APP"
echo "helpers in $APP/Contents/Frameworks" >&2
