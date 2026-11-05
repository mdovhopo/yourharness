#!/bin/zsh
# The web experiment's performance run, the twin of scripts/perf-run.sh: the same extreme
# preset and fake `claude`, the same phases (web/ui/src/probe.ts). Results in <results>/
# phases.json and clicks.json, the log in <results>/dev.log.
# Usage: web/scripts/perf-run.sh <results dir> [YHW_VAR=value ...]
#   YHW_PERF_SECONDS=30 per phase; YHW_PERF_PHASES=stream-all,stream-all-switch; YHW_PERF_FRONT=1
#   takes over the screen as scripts/perf-run.sh's YH_PERF_FRONT does. SKIP_BUILD=1 reuses build/.
set -euo pipefail
ROOT="${0:A:h:h:h}"
RESULTS="${${1:?usage: web/scripts/perf-run.sh <results dir> [YHW_VAR=value ...]}:A}"
shift
if pmset -g batt | grep -q "Battery Power" && [[ ${FORCE:-} != 1 ]]; then
    echo "On battery: plug in for a run worth comparing (FORCE=1 to run anyway)." >&2
    exit 2
fi
SEED="${PERF_SEED:-$ROOT/test-harness-root/extreme}"
[[ -f $SEED/harness.sqlite ]] || "$ROOT/scripts/preset.sh" extreme
[[ ${SKIP_BUILD:-} == 1 ]] || "$ROOT/web/scripts/bundle.sh" >/dev/null
DATA="$ROOT/test-harness-root/web-perf-run"
rm -rf "$DATA"
mkdir -p "$DATA" "$RESULTS"
# A clone: instant, and the seed is never written.
for f in harness.sqlite harness.sqlite-wal harness.sqlite-shm; do [[ -f $SEED/$f ]] && cp -c "$SEED/$f" "$DATA/"; done
ARGS=(--env YHW_DB_DIR="$DATA" --env YHW_NO_CLAUDE=1 --env YHW_PERF="$RESULTS" --env YHW_PERF_QUIT=1
      --env YH_PATH="$ROOT/presets/extreme/fake-claude:$PATH" --env CLAUDE_CONFIG_DIR="$DATA/claude-config")
for kv in "$@"; do ARGS+=(--env "$kv"); done
caffeinate -u -t 2
open -g -n -W "${ARGS[@]}" --stderr "$RESULTS/dev.log" "$ROOT/build/YourHarnessWeb.app" &
OPENER=$!
caffeinate -d -i -w $OPENER &!
( while kill -0 $OPENER 2>/dev/null; do caffeinate -u -t 1; sleep 5; done ) &!
wait $OPENER || true
grep '^PERF' "$RESULTS/dev.log" || true
grep -q '^PERF INVALID' "$RESULTS/dev.log" && echo "NOT A VALID RUN: see the INVALID lines."
grep -q 'PERF DONE' "$RESULTS/dev.log" && echo "Finished." || echo "THE APP DIED BEFORE FINISHING."
