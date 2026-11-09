#!/bin/zsh
# The phone app (docs/mobile.md): generates the Xcode project, builds it, and runs it in a
# Simulator of its own, booted headless (Simulator.app never opens, nothing takes focus).
#
#   scripts/mobile.sh [--no-build] [--screenshot <png>] [--delay <s>]
#
# YH_* variables are passed to the app: YH_SERVER (ws://127.0.0.1:7878/ by default),
# YH_SELECT=<thread title or id>, YH_APPEARANCE=light|dark. YH_SIM_NAME picks the Simulator
# (made if missing). Screenshots come from simctl: the Simulator only, no screen recording.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
BUILD=1 SHOT="" DELAY=4
while (( $# )); do
  case $1 in
    --no-build) BUILD=0 ;;
    --screenshot) SHOT=$2; shift ;;
    --delay) DELAY=$2; shift ;;
    *) echo "usage: scripts/mobile.sh [--no-build] [--screenshot <png>] [--delay <s>]" >&2; exit 2 ;;
  esac
  shift
done

NAME=${YH_SIM_NAME:-"YH Mobile iPhone 17 Pro"}
UDID=$(xcrun simctl list devices available -j | python3 -c "
import json, sys
name = sys.argv[1]
for runtime, devices in json.load(sys.stdin)['devices'].items():
    for d in devices:
        if d['name'] == name: print(d['udid']); sys.exit()
" "$NAME")
if [[ -z $UDID ]]; then
  RUNTIME=$(xcrun simctl list runtimes -j | python3 -c "
import json, sys
ios = [r for r in json.load(sys.stdin)['runtimes'] if r['platform'] == 'iOS' and r['isAvailable'] and r['version'].startswith('26.')]
print(sorted(ios, key=lambda r: [int(x) for x in r['version'].split('.')])[-1]['identifier'])")
  UDID=$(xcrun simctl create "$NAME" com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro "$RUNTIME")
  echo "made Simulator $NAME ($RUNTIME)"
fi

APP="$ROOT/mobile/ios/build/DerivedData/Build/Products/Debug-iphonesimulator/YourHarness.app"
if (( BUILD )); then
  (cd "$ROOT/mobile/ios" && xcodegen generate --quiet)
  xcodebuild -project "$ROOT/mobile/ios/YourHarnessMobile.xcodeproj" -scheme YourHarnessMobile -configuration Debug \
    -destination "id=$UDID" -derivedDataPath "$ROOT/mobile/ios/build/DerivedData" CODE_SIGNING_ALLOWED=NO build -quiet
fi

xcrun simctl bootstatus "$UDID" -b >/dev/null
xcrun simctl status_bar "$UDID" override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3 >/dev/null 2>&1 || true
xcrun simctl install "$UDID" "$APP"
xcrun simctl terminate "$UDID" com.mdovhopo.yourharness.mobile >/dev/null 2>&1 || true
for var in ${(k)parameters[(I)YH_*]}; do export "SIMCTL_CHILD_$var=${(P)var}"; done
xcrun simctl launch "$UDID" com.mdovhopo.yourharness.mobile >/dev/null
echo "running in $NAME ($UDID)"
if [[ -n $SHOT ]]; then
  sleep "$DELAY"
  xcrun simctl io "$UDID" screenshot "$SHOT" >/dev/null 2>&1
  echo "screenshot: $SHOT"
fi
