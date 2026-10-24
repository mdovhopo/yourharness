#!/bin/zsh
# Quits the Dev copy scripts/dev-run.sh started on <data dir>, the way ⌘Q does (so it saves
# and stops its turns), and waits for it to exit. Only that process: other Dev copies,
# other sessions' included, are left alone.
# Usage: scripts/dev-quit.sh <data dir>
set -euo pipefail
PID_FILE="${1:?usage: scripts/dev-quit.sh <data dir>}/dev.pid"
[[ -f $PID_FILE ]] || exit 0
PID=$(<"$PID_FILE")
rm -f "$PID_FILE"
kill -0 $PID 2>/dev/null || exit 0
osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier($PID).terminate" >/dev/null
for _ in {1..100}; do kill -0 $PID 2>/dev/null || exit 0; sleep 0.1; done
echo "Dev copy $PID didn't quit" >&2
exit 1
