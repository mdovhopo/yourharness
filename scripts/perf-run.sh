#!/bin/zsh
# The heavy performance run: a Dev copy on the `extreme` preset, with a fake `claude` that
# streams turns through the app's real path (no tokens). Records frames, CPU and memory per
# phase to <results>/phases.json and seconds.csv, the log to <results>/dev.log, a crash report
# if it crashed, and prints a table. Refuses to run on battery: the numbers would be throttled
# and the run could drain the Mac (FORCE=1 runs anyway).
# Usage: scripts/perf-run.sh <results dir> [YH_VAR=value ...]
#   YH_PERF_SECONDS=30 per phase; YH_PERF_PHASES=stream-8,settled; YH_PERF_RATE=1;
#   FRESH=1 makes the state again (kept in test-harness-root/extreme, copied for every run).
set -euo pipefail
ROOT="${0:A:h:h}"
RESULTS="${${1:?usage: scripts/perf-run.sh <results dir> [YH_VAR=value ...]}:A}"
shift
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
if pmset -g batt | grep -q "Battery Power" && [[ ${FORCE:-} != 1 ]]; then
    echo "On battery: plug in for a run worth comparing (FORCE=1 to run anyway)." >&2
    exit 2
fi
SEED="$ROOT/test-harness-root/extreme"
[[ -d $SEED && ${FRESH:-} != 1 ]] || "$ROOT/scripts/preset.sh" extreme
# Moved into the database and indexed once, so runs measure use, not the upgrade.
if [[ ! -f $SEED/.moved ]]; then
    "$ROOT/scripts/dev-run.sh" "$SEED" YH_SEARCH_PROBE=timezone
    until grep -q '^SEARCH moved' "$SEED/dev.log"; do sleep 2; done
    "$ROOT/scripts/dev-quit.sh" "$SEED"
    touch "$SEED/.moved"
fi
DATA="$ROOT/test-harness-root/extreme-run"
rm -rf "$DATA"
cp -R "$SEED" "$DATA"
# Paths in the state point at the seed folder.
find "$DATA" -maxdepth 1 -name state.json -exec sed -i '' "s|$SEED|$DATA|g" {} +
mkdir -p "$RESULTS"
START=$(date +%s)
PATH="$ROOT/presets/extreme/fake-claude:$PATH" "$ROOT/scripts/dev-run.sh" "$DATA" \
    YH_PERF_PROBE="$RESULTS" YH_PERF_QUIT=1 CLAUDE_CONFIG_DIR="$DATA/claude-config" "$@"
PID=$(cat "$DATA/dev.pid")
while kill -0 "$PID" 2>/dev/null; do sleep 2; done
cp "$DATA/dev.log" "$RESULTS/dev.log"
grep '^PERF' "$RESULTS/dev.log" || true
grep -q 'PERF DONE' "$RESULTS/dev.log" && echo "Finished." || echo "THE APP DIED BEFORE FINISHING (pid $PID)."
sleep 5
for report in ~/Library/Logs/DiagnosticReports/YourHarness*.ips(N.om[1,3]); do
    if (( $(stat -f %m "$report") >= START )); then
        cp "$report" "$RESULTS/"
        echo "Crash report: $report"
    fi
done
