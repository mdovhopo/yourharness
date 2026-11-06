#!/bin/zsh
# The GPUI experiment's performance run, the twin of web/scripts/perf-run.sh and
# scripts/perf-run.sh: the same extreme preset and fake `claude`, the same phases (src/probe.rs).
# Results in <results>/phases.json and clicks.json, the log in <results>/dev.log.
# Usage: gpui/scripts/perf-run.sh <results dir> [YHG_VAR=value ...]
#   YHG_PERF_SECONDS=30 per phase; YHG_PERF_PHASES=stream-all,stream-all-switch; YHG_PERF_FRONT=1
#   takes over the screen. SKIP_BUILD=1 reuses build/. PERF_SEED=<dir with harness.sqlite>.
set -euo pipefail
DIR="${0:A:h:h}"
ROOT="${DIR:h}"
RESULTS="${${1:?usage: gpui/scripts/perf-run.sh <results dir> [YHG_VAR=value ...]}:A}"
shift
if pmset -g batt | grep -q "Battery Power" && [[ ${FORCE:-} != 1 ]]; then
    echo "On battery: plug in for a run worth comparing (FORCE=1 to run anyway)." >&2
    exit 2
fi
# The preset lives in the main checkout; a worktree may not have it.
SEED="${PERF_SEED:-$ROOT/test-harness-root/extreme}"
[[ -f $SEED/harness.sqlite ]] || SEED="${ROOT:h}/yourharness/test-harness-root/extreme"
[[ -f $SEED/harness.sqlite ]] || "$ROOT/scripts/preset.sh" extreme
FAKE="$ROOT/presets/extreme/fake-claude"
[[ ${SKIP_BUILD:-} == 1 ]] || "$DIR/scripts/bundle.sh" >/dev/null
DATA="$ROOT/test-harness-root/gpui-perf-run"
rm -rf "$DATA"
mkdir -p "$DATA" "$RESULTS"
# A clone: instant, and the seed is never written (the app opens it read-only anyway).
for f in harness.sqlite harness.sqlite-wal harness.sqlite-shm; do [[ -f $SEED/$f ]] && cp -c "$SEED/$f" "$DATA/"; done
ARGS=(--env YHG_DB_DIR="$DATA" --env YHG_NO_CLAUDE=1 --env YHG_PERF="$RESULTS" --env YHG_PERF_QUIT=1
      --env YH_PATH="$FAKE:$PATH" --env CLAUDE_CONFIG_DIR="$DATA/claude-config")
for kv in "$@"; do ARGS+=(--env "$kv"); done
caffeinate -u -t 2
open -g -n -W "${ARGS[@]}" --stderr "$RESULTS/dev.log" "$ROOT/build/YourHarnessGPUI.app" &
OPENER=$!
caffeinate -d -i -w $OPENER &!
( while kill -0 $OPENER 2>/dev/null; do caffeinate -u -t 1; sleep 5; done ) &!
wait $OPENER || true
grep '^PERF' "$RESULTS/dev.log" || true
grep -q '^PERF INVALID' "$RESULTS/dev.log" && echo "NOT A VALID RUN: see the INVALID lines."
grep -q 'PERF DONE' "$RESULTS/dev.log" && echo "Finished." || echo "THE APP DIED BEFORE FINISHING."
