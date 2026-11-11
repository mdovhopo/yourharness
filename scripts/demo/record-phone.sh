#!/bin/zsh
# A Mac scene and the phone at once: the Mac app serves phones over Tailscale only (the demo data
# is paired with the demo Simulator), the Simulator films itself with `simctl io recordVideo`, and
# both start times go in <take>-sync.json for the edit to line them up.
#   scripts/demo/record-phone.sh <mac scene.json> <phone steps.json> [mac appearance] [phone appearance]
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
demo=$root/test-harness-root/demo
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
scene=${1:A}; steps=${2:A}; mac_look=${3:-dark}; phone_look=${4:-$mac_look}
name=${scene:t:r}-$mac_look
udid=$(cat "$demo/phone-udid")
takes=$demo/takes; mkdir -p "$takes"
phone_app=$demo/phone-dd/Build/Products/Debug-iphonesimulator/YourHarness.app
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl status_bar "$udid" override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3
xcrun simctl install "$udid" "$phone_app"
xcrun simctl terminate "$udid" com.mdovhopo.yourharness.mobile 2>/dev/null || true

rm -f "$takes/$name.log" "$takes/$name-phone.start"
EXTRA_ENV="--env YH_PHONES=1 --env YH_PHONES_PORT=47979 --env YH_PHONES_TAILSCALE_ONLY=1 ${EXTRA_ENV:-}" \
  "$here/record.sh" "$scene" "$mac_look" &
mac=$!
log=$takes/$name.log
until grep -q "DEMO recording at" "$log" 2>/dev/null; do sleep 0.2; done
mac_start=$(grep -o "DEMO recording at [0-9.]*" "$log" | awk '{print $4}')

# The Simulator's film, its start stamped when simctl says it began.
rm -f "$takes/$name-phone.mp4"
python3 -c '
import subprocess, sys, time
p = subprocess.Popen(["xcrun", "simctl", "io", sys.argv[1], "recordVideo", "--codec", "h264", "--force", sys.argv[2]], stderr=subprocess.PIPE, text=True)
for line in p.stderr:
    if "Recording started" in line:
        open(sys.argv[3], "w").write("%.3f" % time.time())
p.wait()
' "$udid" "$takes/$name-phone.mp4" "$takes/$name-phone.start" &
rec=$!
until [[ -s $takes/$name-phone.start ]]; do sleep 0.1; done
SIMCTL_CHILD_YH_PHONE_DEMO="$(cat "$steps")" SIMCTL_CHILD_YH_APPEARANCE="$phone_look" \
  xcrun simctl launch --terminate-running-process --stdout="$takes/$name-phone.log" --stderr="$takes/$name-phone.log" "$udid" com.mdovhopo.yourharness.mobile >/dev/null
wait $mac
pkill -INT -f "simctl io $udid recordVideo" || true
wait $rec || true
printf '{"mac_start": %s, "phone_start": %s}\n' "$mac_start" "$(cat "$takes/$name-phone.start")" > "$takes/$name-sync.json"
cat "$takes/$name-sync.json"
grep -E "DEMO .*done|no thread|unknown" "$takes/$name-phone.log" || true
