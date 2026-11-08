#!/bin/zsh
# The performance run: the extreme preset (cloned, never written), a fake `claude` streaming
# 20 huge threads, the probe's phases (crates/yh-app/src/probe.rs). Results in
# <results>/phases.json and clicks.json, the log in <results>/dev.log.
# Usage: scripts/perf-run.sh <results dir> [YH_VAR=value ...]
#   YH_PERF_SECONDS=30 per phase; YH_PERF_PHASES=stream-all,stream-all-switch; YH_PERF_FRONT=1
#   takes over the screen (only when the user is away). SKIP_BUILD=1 reuses the perf build.
#   PERF_SEED=<folder with harness.sqlite>; FORCE=1 on battery.
#   YH_PERF_PHASES=stream-all-everything (every kind of click, timed; YH_PERF_ONLY=<groups>),
#   stream-all-chaos (anything in any order; YH_PERF_CHAOS_SEED=<n> plays a run again).
#   PERF_APP=<path.app> builds and runs that copy instead (several runs side by side).
set -euo pipefail
ROOT="${0:A:h:h}"
RESULTS="${${1:?usage: scripts/perf-run.sh <results dir> [YH_VAR=value ...]}:A}"
shift
if pmset -g batt | grep -q "Battery Power" && [[ ${FORCE:-} != 1 ]]; then
    echo "On battery: plug in for a run worth comparing (FORCE=1 to run anyway)." >&2
    exit 2
fi
SEED="${PERF_SEED:-$ROOT/test-harness-root/extreme}"
[[ -f $SEED/harness.sqlite ]] || "$ROOT/scripts/preset.sh" extreme
APP="${PERF_APP:-$ROOT/test-harness-root/perf-build/YourHarness Next.app}"
[[ ${SKIP_BUILD:-} == 1 && -d $APP ]] || APP="$APP" "$ROOT/scripts/bundle.sh" >/dev/null
# A data folder of its own each run, so runs side by side never share one.
DATA="$ROOT/test-harness-root/perf-run-$(date +%Y%m%d-%H%M%S)-$$"
mkdir -p "$DATA" "$RESULTS"
# A clone: instant, and the seed is never written.
for f in harness.sqlite harness.sqlite-wal harness.sqlite-shm; do [[ -f $SEED/$f ]] && cp -c "$SEED/$f" "$DATA/"; done
echo "data: $DATA"
ARGS=(--env YH_DATA_DIR="$DATA" --env YH_NO_CLAUDE=1 --env YH_PERF="$RESULTS" --env YH_PERF_QUIT=1 --env YH_NOTIFICATIONS=log
      --env YH_PATH="$ROOT/presets/extreme/fake-claude:$PATH" --env CLAUDE_CONFIG_DIR="$DATA/claude-config")
for kv in "$@"; do ARGS+=(--env "$kv"); done
caffeinate -u -t 2
open -g -n -W "${ARGS[@]}" --stderr "$RESULTS/dev.log" "$APP" &
OPENER=$!
caffeinate -d -i -w $OPENER &!
( while kill -0 $OPENER 2>/dev/null; do caffeinate -u -t 1; sleep 5; done ) &!
wait $OPENER || true
grep '^PERF' "$RESULTS/dev.log" | grep -v "PERF scroll:\|PERF chaos [0-9]\|PERF everything:" || true
grep -q '^PERF INVALID' "$RESULTS/dev.log" && echo "NOT A VALID RUN: see the INVALID lines."
grep -q 'PERF DONE' "$RESULTS/dev.log" && echo "Finished." || echo "THE APP DIED BEFORE FINISHING."
