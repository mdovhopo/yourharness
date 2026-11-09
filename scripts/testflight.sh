#!/bin/zsh
# The phone app to TestFlight (docs/mobile.md, "TestFlight"): archives a Release build signed
# for distribution and uploads it to App Store Connect, for internal testers only.
#
#   scripts/testflight.sh            archive and upload
#   DRY=1 scripts/testflight.sh      archive and export the .ipa only (mobile/ios/build/TestFlight)
#
# Needs, once: Xcode signed in to your Apple account (it uploads as you; or ASC_KEY_ID,
# ASC_ISSUER_ID and ASC_KEY_PATH, an App Store Connect API key, for CI), and the app made in App
# Store Connect with bundle ID com.mdovhopo.yourharness.mobile.
# The version is the repo's VERSION; the build number the commit count, so every upload is
# higher than the last. The icon is Next's (night blue); APP_ICON=AppIcon for the App Store.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}

TEAM=${TEAM:-}
if [[ -z $TEAM ]]; then
  TEAM=$(security find-certificate -a -c "Apple Development" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null | sed -nE 's/.*OU ?= ?([A-Z0-9]{10}).*/\1/p' | head -1 || true)
fi
[[ -n $TEAM ]] || { echo "No Apple team found: sign Xcode in to your Apple account (Settings → Accounts), or pass TEAM=<id>." >&2; exit 1; }
VERSION=$(cat "$ROOT/VERSION")
BUILD=$(git -C "$ROOT" rev-list --count HEAD)
ICON=${APP_ICON:-AppIconNext}
OUT="$ROOT/mobile/ios/build/TestFlight"
ARCHIVE="$OUT/YourHarness.xcarchive"
echo "TestFlight: YourHarness $VERSION ($BUILD), team $TEAM, icon $ICON"

AUTH=()
if [[ -n ${ASC_KEY_ID:-} ]]; then
  AUTH=(-authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID" -authenticationKeyPath "$ASC_KEY_PATH")
fi

(cd "$ROOT/mobile/ios" && xcodegen generate --quiet)
rm -rf "$OUT"
mkdir -p "$OUT"
xcodebuild -project "$ROOT/mobile/ios/YourHarnessMobile.xcodeproj" -scheme YourHarnessMobile -configuration Release \
  -destination generic/platform=iOS -archivePath "$ARCHIVE" -derivedDataPath "$OUT/derived" \
  -allowProvisioningUpdates "${AUTH[@]}" \
  DEVELOPMENT_TEAM="$TEAM" CODE_SIGN_STYLE=Automatic \
  MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$BUILD" ASSETCATALOG_COMPILER_APPICON_NAME="$ICON" \
  archive -quiet

DESTINATION=$([[ -n ${DRY:-} ]] && echo export || echo upload)
cat > "$OUT/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key><string>app-store-connect</string>
    <key>destination</key><string>$DESTINATION</string>
    <key>teamID</key><string>$TEAM</string>
    <key>signingStyle</key><string>automatic</string>
    <key>uploadSymbols</key><true/>
    <key>manageAppVersionAndBuildNumber</key><false/>
    <key>testFlightInternalTestingOnly</key><true/>
</dict>
</plist>
PLIST
xcodebuild -exportArchive -archivePath "$ARCHIVE" -exportOptionsPlist "$OUT/ExportOptions.plist" -exportPath "$OUT/export" \
  -allowProvisioningUpdates "${AUTH[@]}" -quiet
if [[ -n ${DRY:-} ]]; then
  echo "exported: $OUT/export (not uploaded)"
else
  echo "Uploaded $VERSION ($BUILD). It shows in App Store Connect → TestFlight once processed (10–30 min); internal testers get it there."
fi
