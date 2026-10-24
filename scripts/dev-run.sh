#!/bin/zsh
# Launches a Dev copy against a scratch data folder, the safe way: its own bundle ID, your
# PATH (no login shell), notifications to stderr, in the background so it never takes focus.
# Logs go to <data>/dev.log, its process ID to <data>/dev.pid (scripts/dev-quit.sh quits it).
# Extra YH_* variables pass through: VAR=value after the folder.
# Usage: scripts/dev-run.sh <data dir> [YH_VAR=value ...]
set -euo pipefail
ROOT="${0:A:h:h}"
DATA="${${1:?usage: scripts/dev-run.sh <data dir> [YH_VAR=value ...]}:A}"
shift
APP_DIR="$ROOT/test-harness-root/app"
[[ -d "$APP_DIR/YourHarness Dev.app" && "$APP_DIR/YourHarness Dev.app" -nt "$ROOT/build/YourHarness.app/Contents/MacOS/YourHarness" ]] \
    || "$ROOT/scripts/dev-app.sh" "$APP_DIR" >/dev/null
env=(--env "YH_DATA_DIR=$DATA" --env "YH_PATH=$PATH" --env YH_NOTIFICATIONS=log)
for variable in "$@"; do env+=(--env "$variable"); done
BEFORE=($(pgrep -f "$APP_DIR/YourHarness Dev.app/Contents/MacOS/YourHarness" || true))
open -g -n --stdout "$DATA/dev.log" --stderr "$DATA/dev.log" "${env[@]}" "$APP_DIR/YourHarness Dev.app"
for _ in {1..50}; do
    for pid in $(pgrep -f "$APP_DIR/YourHarness Dev.app/Contents/MacOS/YourHarness" || true); do
        (( ${BEFORE[(Ie)$pid]} )) || { echo $pid > "$DATA/dev.pid"; break 2 }
    done
    sleep 0.1
done
echo "Running against $DATA (log: $DATA/dev.log)"
