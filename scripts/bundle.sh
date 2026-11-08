#!/bin/zsh
# Builds the app (release, arm64) as build/YourHarness Next.app, signed ad hoc.
#   RELEASE=1         the real app: build/YourHarness.app, bundle ID com.mdovhopo.yourharness,
#                     signed with the app's own identity when it's here (YH_SIGN_IDENTITY and
#                     YH_SIGN_KEYCHAIN from scripts/signing-keychain.sh, as the release workflow
#                     does, or ~/.yourharness/signing): the updater only installs builds signed
#                     the same way. Only for releases and the switch.
#   APP=<path.app>    somewhere else (test copies go in test-harness-root/…, never build/,
#                     which may be the copy the user has open)
#   FEATURES=snapshots  YH_SNAPSHOT support, for test copies (not for perf runs)
#   SIGN=1            a Next build signed with the app's own identity, not ad hoc, so macOS keeps
#                     what it allowed across rebuilds: the user's own Next copy in /Applications
# Its own bundle ID until the switch, so it never shares permissions or data with the user's
# app (/Applications/YourHarness.app, the frozen Swift 0.0.16).
set -euo pipefail
ROOT="${0:A:h:h}"
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
export PATH="$HOME/.cargo/bin:$PATH"
# The binary's minimum macOS (LC_BUILD_VERSION minos) matches LSMinimumSystemVersion below.
export MACOSX_DEPLOYMENT_TARGET=26.0
"$ROOT/scripts/vendor.sh"
# A failed build stops here, so a binary left by an earlier build never gets bundled.
LOG="$(mktemp)"
if ! (cd "$ROOT" && cargo build --release -p yh-app ${FEATURES:+--features=$FEATURES}) >"$LOG" 2>&1; then
    cat "$LOG" >&2; rm -f "$LOG"
    echo "build failed" >&2; exit 1
fi
grep -E "^(error|warning: unused)|Finished" -A6 "$LOG" >&2 || true
rm -f "$LOG"
BIN="$ROOT/target/release/YourHarness"
[[ -x $BIN ]] || { echo "build failed" >&2; exit 1; }
if [[ -n "${RELEASE:-}" ]]; then
    NAME="YourHarness"; BUNDLE_ID="com.mdovhopo.yourharness"
    APP="${APP:-$ROOT/build/YourHarness.app}"
else
    NAME="YourHarness Next"; BUNDLE_ID="com.mdovhopo.yourharness.next"
    APP="${APP:-$ROOT/build/YourHarness Next.app}"
fi
BUILD="$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)"
COMMIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)$([[ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]] && echo -dirty || true)"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/YourHarness"
VERSION="${VERSION_OVERRIDE:-$(cat "$ROOT/VERSION")}"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>YourHarness</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleName</key><string>$NAME</string>
    <key>CFBundleDisplayName</key><string>$NAME</string>
    <key>YHGitCommit</key><string>$COMMIT</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>LSArchitecturePriority</key><array><string>arm64</string></array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSSupportsAutomaticTermination</key><false/>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST
SIGNED="signed ad hoc"
if [[ -n "${RELEASE:-}" || -n "${SIGN:-}" ]]; then
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
fi
if [[ -n "${IDENTITY:-}" ]]; then
    codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$APP" >/dev/null
    SIGNED="signed as $(security find-certificate -a -Z "$KEYCHAIN" | awk -F'"' '/"labl"/ { print $4; exit }')"
else
    codesign --force --sign - "$APP" >/dev/null 2>&1
fi
echo "Built $NAME $VERSION (build $BUILD, $COMMIT, $SIGNED)" >&2
echo "$APP"
