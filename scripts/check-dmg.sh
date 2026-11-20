#!/bin/zsh
# Checks a release disk image the way the updaters (state/updater.rs, and 0.0.x's)
# will: scripts/check-dmg.sh build/YourHarness-<version>.dmg [<signing identity's name>]
#   - <dmg>.sha256 sits next to it and matches it
#   - mounted read-only, it holds YourHarness.app and an Applications link, nothing else
#   - the app is com.mdovhopo.yourharness at VERSION, an arm64 executable named YourHarness
#     that links only system libraries and the app's own in Contents/Frameworks (voice input's
#     sherpa-onnx), found through @executable_path only (no build machine's path left in)
#   - codesign --verify --deep --strict passes; with an identity, it's signed by it (not ad hoc)
#   - the Chrome engine's five helper apps are there, arm64, system-only, signed the same way
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
# Each library outside the system is @rpath/<name>, shipped in Frameworks, arm64, and itself
# links only the system and its neighbours there.
FRAMEWORKS="$APP/Contents/Frameworks"
# otool reads `name (GPU)` as an archive member: a path with parentheses goes through a link.
outside() {
    local f="$1"
    if [[ "$f" == *"("* ]]; then f="$(mktemp -d)/bin"; ln -s "$1" "$f"; fi
    otool -L "$f" | tail -n +2 | awk '{ print $1 }' | grep -v -E '^(/usr/lib/|/System/Library/)' || true
}
for lib in $(outside "$BIN"); do
    name="${lib#@rpath/}"
    [[ "$lib" == @rpath/* && "$name" != */* && -f "$FRAMEWORKS/$name" ]] || fail "the executable links $lib, which isn't in the app's Frameworks"
    [[ "$(lipo -archs "$FRAMEWORKS/$name")" == arm64 ]] || fail "$name is $(lipo -archs "$FRAMEWORKS/$name")"
    for dep in $(outside "$FRAMEWORKS/$name"); do
        [[ "$dep" == "@rpath/$name" || ( "$dep" == @rpath/* && -f "$FRAMEWORKS/${dep#@rpath/}" ) ]] || fail "$name links $dep, which isn't in the app's Frameworks"
    done
done
rpaths="$(otool -l "$BIN" | awk '/LC_RPATH/ { getline; getline; print $2 }')"
for r in ${(f)rpaths}; do
    [[ "$r" == @executable_path/* ]] || fail "the executable looks for libraries in $r"
done

codesign --verify --deep --strict "$APP" || fail "codesign --verify --deep --strict fails"
info="$(codesign -dvv "$APP" 2>&1)"
[[ "$info" == *"Identifier=com.mdovhopo.yourharness"* ]] || fail "signed as another identifier"
for lib in "$FRAMEWORKS"/*.dylib(N); do
    codesign --verify --strict "$lib" || fail "${lib:t} isn't signed"
    [[ -z "$AUTHORITY" || "$(codesign -dvv "$lib" 2>&1)" == *"Authority=$AUTHORITY"* ]] || fail "${lib:t} isn't signed by $AUTHORITY"
done
# The Chrome engine's helper apps (scripts/cef-helpers.sh): all five, arm64, linking only the
# system (the framework itself is downloaded, never in the app), signed like the app.
for flavor in "" " (GPU)" " (Renderer)" " (Plugin)" " (Alerts)"; do
    helper="$FRAMEWORKS/YourHarness Helper$flavor.app"
    exe="$helper/Contents/MacOS/YourHarness Helper$flavor"
    [[ -x $exe ]] || fail "YourHarness Helper$flavor.app is missing"
    [[ "$(lipo -archs "$exe")" == arm64 ]] || fail "YourHarness Helper$flavor is $(lipo -archs "$exe")"
    [[ -z "$(outside "$exe")" ]] || fail "YourHarness Helper$flavor links $(outside "$exe" | head -1)"
    codesign --verify --strict "$helper" || fail "YourHarness Helper$flavor.app isn't signed"
    [[ -z "$AUTHORITY" || "$(codesign -dvv "$helper" 2>&1)" == *"Authority=$AUTHORITY"* ]] || fail "YourHarness Helper$flavor.app isn't signed by $AUTHORITY"
done
[[ ! -e "$FRAMEWORKS/Chromium Embedded Framework.framework" ]] || fail "the CEF framework is in the app (it's downloaded on first use)"
if [[ -n "$AUTHORITY" ]]; then
    [[ "$info" == *"Authority=$AUTHORITY"* ]] || fail "not signed by $AUTHORITY"
    [[ "$info" != *"Signature=adhoc"* ]] || fail "signed ad hoc"
fi
signed="$(print -r -- "$info" | awk -F= '/^Authority=/ { print $2; exit }')"
echo "check-dmg: ${DMG:t} OK (com.mdovhopo.yourharness $VERSION, build $(plist CFBundleVersion), signed ${signed:-ad hoc})"
