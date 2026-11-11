#!/bin/zsh
# The demo Simulator paired with the demo data's Mac, over Tailscale only (the pairing kept in
# data-seed, so every take's copy knows the phone).
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
demo=$root/test-harness-root/demo
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
udid=$(cat "$demo/phone-udid")
echo '{"record": false, "lead": 1, "tail": 0, "steps": [{"wait": 40}]}' > "$demo/pair-wait.json"
rm -f "$demo/pair.log"
open -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR="$demo/data-seed" --env YH_PATH="$PATH" --env YH_NOTIFICATIONS=log \
  --env YH_PHONES=1 --env YH_PHONES_PORT=47979 --env YH_PHONES_TAILSCALE_ONLY=1 --env YH_PHONES_LOG=1 \
  --env YH_DEMO="$demo/pair-wait.json" --stderr "$demo/pair.log" "$demo/app/YourHarness Next.app"
until grep -q "yourharness://pair" "$demo/pair.log" 2>/dev/null; do sleep 0.5; done
link=$(grep -o "yourharness://pair[^ ]*" "$demo/pair.log" | head -1)
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl install "$udid" "$demo/phone-dd/Build/Products/Debug-iphonesimulator/YourHarness.app"
# A clean phone: no Macs from before.
xcrun simctl uninstall "$udid" com.mdovhopo.yourharness.mobile 2>/dev/null || true
xcrun simctl install "$udid" "$demo/phone-dd/Build/Products/Debug-iphonesimulator/YourHarness.app"
SIMCTL_CHILD_YH_PHONE_PAIR="$link" xcrun simctl launch --terminate-running-process "$udid" com.mdovhopo.yourharness.mobile >/dev/null
until grep -q "paired as" "$demo/pair.log"; do sleep 0.5; done
grep "paired as" "$demo/pair.log"
