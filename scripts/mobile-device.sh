#!/bin/zsh
# The phone app on a real iPhone or iPad (docs/mobile.md, "On your own iPhone or iPad"): builds it
# signed with your Apple team, installs it on the device and opens it.
#
#   scripts/mobile-device.sh [device name or UDID]
#
# Needs, once: Xcode signed in to your Apple account (Xcode → Settings → Accounts), the device
# connected by cable (or on the same Wi‑Fi after pairing it in Xcode once), trusted, and in
# Developer Mode (Settings → Privacy & Security → Developer Mode on the device).
# TEAM=<team ID> picks the team (else the one in your Apple Development certificate);
# BUNDLE_ID=<id> when that ID is taken under a free team.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
WANT=${1:-}

# The team: given, or read from your Apple Development certificate (its OU).
TEAM=${TEAM:-}
if [[ -z $TEAM ]]; then
  TEAM=$(security find-certificate -a -c "Apple Development" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null | sed -nE 's/.*OU ?= ?([A-Z0-9]{10}).*/\1/p' | head -1 || true)
fi
if [[ -z $TEAM ]]; then
  cat >&2 <<'MSG'
No Apple team found. Open Xcode → Settings → Accounts, add your Apple ID, select it and
click "Manage Certificates…" → + → Apple Development. Then run this again (or pass TEAM=<id>,
the 10 characters shown next to your team in Accounts).
MSG
  exit 1
fi

# The device: named, or the first real one this Mac can reach.
LIST=$(mktemp)
xcrun devicectl list devices --json-output "$LIST" >/dev/null 2>&1 || true
DEVICE=$(python3 -I - "$LIST" "$WANT" <<'PY'
import json, sys
devices = json.load(open(sys.argv[1])).get("result", {}).get("devices", [])
want = sys.argv[2]
for d in devices:
    props, hw, conn = d.get("deviceProperties", {}), d.get("hardwareProperties", {}), d.get("connectionProperties", {})
    if hw.get("reality") != "physical":
        continue
    name, udid = props.get("name", ""), hw.get("udid", "")
    if want and want not in (name, udid):
        continue
    reachable = conn.get("tunnelState") in ("connected", "disconnected") and conn.get("pairingState") == "paired"
    print(f"{udid}\t{name}\t{'yes' if reachable else 'no'}")
    break
PY
)
rm -f "$LIST"
if [[ -z $DEVICE ]]; then
  echo "No iPhone or iPad found${WANT:+ named $WANT}. Connect it by cable, unlock it and tap Trust." >&2
  exit 1
fi
UDID=${DEVICE%%$'\t'*}; REST=${DEVICE#*$'\t'}; NAME=${REST%%$'\t'*}
echo "device: $NAME ($UDID), team $TEAM"

(cd "$ROOT/mobile/ios" && xcodegen generate --quiet)
BUILT="$ROOT/mobile/ios/build/Device"
EXTRA=()
[[ -n ${BUNDLE_ID:-} ]] && EXTRA+=(PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID")
xcodebuild -project "$ROOT/mobile/ios/YourHarnessMobile.xcodeproj" -scheme YourHarnessMobile -configuration Debug \
  -destination "id=$UDID" -derivedDataPath "$BUILT" -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
  DEVELOPMENT_TEAM="$TEAM" CODE_SIGN_STYLE=Automatic "${EXTRA[@]}" build -quiet
APP="$BUILT/Build/Products/Debug-iphoneos/YourHarness.app"
xcrun devicectl device install app --device "$UDID" "$APP"
ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Info.plist")
xcrun devicectl device process launch --device "$UDID" "$ID" >/dev/null || \
  echo "Installed. If it doesn't open: on the device, Settings → General → VPN & Device Management → trust your developer app, then open YourHarness."
echo "On $NAME: YourHarness. On the Mac: YourHarness → Settings → Phone → Let phones connect, then scan the code."
