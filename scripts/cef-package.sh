#!/bin/zsh
# Packs the Chrome engine's download: CEF's framework (the `cef` crate's version, from its
# distribution in CEF_PATH) signed with the app's own identity, zipped as the app expects it.
# Prints the zip, its size and SHA-256: the pin in crates/yh-app/src/webview/chrome.rs. Upload
# it to the `cef-<version>` GitHub release (a pre-release: the updater never offers those):
#   gh release create cef-<version> <zip> --prerelease --latest=false --title … --notes …
# Usage: scripts/cef-package.sh [out dir]
set -euo pipefail
ROOT="${0:A:h:h}"
OUT="${1:-$ROOT/build}"
CEF_PATH=${CEF_PATH:-$HOME/.cache/yourharness-cef}
VERSION="$(sed -n 's/^pub const CEF_VERSION: &str = "\(.*\)";/\1/p' "$ROOT/crates/yh-app/src/webview/chrome.rs")"
SRC="$CEF_PATH/$VERSION/cef_macos_aarch64/Chromium Embedded Framework.framework"
[[ -d $SRC ]] || { echo "no $SRC: build once (cargo build) to download CEF's distribution" >&2; exit 1; }
STAGE="$(mktemp -d)"
ditto "$SRC" "$STAGE/Chromium Embedded Framework.framework"
SIGNING="${YH_SIGNING_DIR:-$HOME/.yourharness/signing}"
IDENTITY="${YH_SIGN_IDENTITY:-}" KEYCHAIN="${YH_SIGN_KEYCHAIN:-}"
if [[ -z "$IDENTITY" && -f "$SIGNING/signing.p12" ]]; then
    KEYCHAIN="$SIGNING/signing.keychain-db"
    security unlock-keychain -p "$(<"$SIGNING/keychain-password")" "$KEYCHAIN" 2>/dev/null || \
        "$ROOT/scripts/signing-keychain.sh" "$SIGNING/signing.p12" "$(<"$SIGNING/p12-password")" "$KEYCHAIN" "$(<"$SIGNING/keychain-password")" >/dev/null
    IDENTITY="$(security find-certificate -a -Z "$KEYCHAIN" | awk '/SHA-1 hash:/ { print $3; exit }')"
fi
FW="$STAGE/Chromium Embedded Framework.framework"
if [[ -n "$IDENTITY" ]]; then
    codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$FW"/Libraries/*.dylib >/dev/null
    codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$FW" >/dev/null
fi
mkdir -p "$OUT"
ZIP="$OUT/Chromium-Embedded-Framework-$VERSION-macos-arm64.zip"
rm -f "$ZIP"
(cd "$STAGE" && ditto -c -k --keepParent "Chromium Embedded Framework.framework" "$ZIP")
rm -rf "$STAGE"
echo "$ZIP"
echo "bytes $(stat -f %z "$ZIP")"
echo "sha256 $(shasum -a 256 "$ZIP" | cut -d' ' -f1)"
