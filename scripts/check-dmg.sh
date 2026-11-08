#!/bin/zsh
# Checks a release disk image the way the updaters (state/updater.rs, and 0.0.x's)
# will: scripts/check-dmg.sh build/YourHarness-<version>.dmg [<signing identity's name>]
#   - <dmg>.sha256 sits next to it and matches it
#   - mounted read-only, it holds YourHarness.app and an Applications link, nothing else
#   - the app is com.mdovhopo.yourharness at VERSION, an arm64 executable named YourHarness
#     that links only system libraries
#   - codesign --verify --deep --strict passes; with an identity, it's signed by it (not ad hoc)
set -euo pipefail
ROOT="${0:A:h:h}"
DMG="${1:?usage: scripts/check-dmg.sh <dmg> [<signing identity name>]}"
AUTHORITY="${2:-}"
VERSION="$(<"$ROOT/VERSION")"
fail() { echo "check-dmg: $1" >&2; exit 1 }

[[ "${DMG:t}" == "YourHarness-$VERSION.dmg" ]] || fail "${DMG:t} isn't named YourHarness-$VERSION.dmg"
[[ -f "$DMG.sha256" ]] || fail "no ${DMG:t}.sha256"
read -r digest file < "$DMG.sha256"
[[ "$file" == "${DMG:t}" ]] || fail "${DMG:t}.sha256 names $file"
[[ "$(shasum -a 256 "$DMG" | cut -d' ' -f1)" == "$digest" ]] || fail "${DMG:t} doesn't match its .sha256"

MOUNT="$(mktemp -d)"
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" "$DMG"
trap 'hdiutil detach -quiet -force "$MOUNT" 2>/dev/null; rmdir "$MOUNT" 2>/dev/null || true' EXIT

contents="$(ls -A "$MOUNT" | grep -v -E '^\.(fseventsd|Trashes|DS_Store)$' | sort | tr '\n' ' ')"
[[ "$contents" == "Applications YourHarness.app " ]] || fail "the image holds: $contents"
[[ "$(readlink "$MOUNT/Applications")" == /Applications ]] || fail "Applications isn't a link to /Applications"

APP="$MOUNT/YourHarness.app"
plist() { /usr/bin/plutil -extract "$1" raw -o - "$APP/Contents/Info.plist" }
[[ "$(plist CFBundleIdentifier)" == com.mdovhopo.yourharness ]] || fail "bundle ID is $(plist CFBundleIdentifier)"
[[ "$(plist CFBundleShortVersionString)" == "$VERSION" ]] || fail "version is $(plist CFBundleShortVersionString), VERSION says $VERSION"
[[ "$(plist CFBundleExecutable)" == YourHarness ]] || fail "executable is $(plist CFBundleExecutable)"
BIN="$APP/Contents/MacOS/YourHarness"
[[ "$(lipo -archs "$BIN")" == arm64 ]] || fail "the executable is $(lipo -archs "$BIN")"
others="$(otool -L "$BIN" | tail -n +2 | awk '{ print $1 }' | grep -v -E '^(/usr/lib/|/System/Library/)' || true)"
[[ -z "$others" ]] || fail "the executable links libraries outside the system: $others"

codesign --verify --deep --strict "$APP" || fail "codesign --verify --deep --strict fails"
info="$(codesign -dvv "$APP" 2>&1)"
[[ "$info" == *"Identifier=com.mdovhopo.yourharness"* ]] || fail "signed as another identifier"
if [[ -n "$AUTHORITY" ]]; then
    [[ "$info" == *"Authority=$AUTHORITY"* ]] || fail "not signed by $AUTHORITY"
    [[ "$info" != *"Signature=adhoc"* ]] || fail "signed ad hoc"
fi
signed="$(print -r -- "$info" | awk -F= '/^Authority=/ { print $2; exit }')"
echo "check-dmg: ${DMG:t} OK (com.mdovhopo.yourharness $VERSION, build $(plist CFBundleVersion), signed ${signed:-ad hoc})"
