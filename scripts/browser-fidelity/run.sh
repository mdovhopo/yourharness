#!/bin/zsh
# Checks that the built-in browser shows pages exactly as a plain, unscaled WebKit view does
# (Safari's engine at the same size), at every page size, and that its input acts like a
# real mouse: hover, clicks, pointer drags, HTML drag and drop, scrolling. It runs three
# times: the page in the side panel (it opens there), parked off screen (the panel kept
# closed), and in a window of its own, each drawing it at a different scale.
# Usage: scripts/browser-fidelity/run.sh [out dir]    (SKIP_BUILD=1 to reuse build/)
set -euo pipefail
HERE=${0:A:h}
ROOT=${HERE:h:h}
OUT=${1:-$(mktemp -d)}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
mkdir -p "$OUT"
[[ -n ${SKIP_BUILD:-} ]] || "$ROOT/scripts/bundle.sh" >/dev/null
"$ROOT/scripts/dev-app.sh" "$OUT" >/dev/null
swiftc -O "$HERE/reference.swift" -o "$OUT/reference"
swiftc -O "$HERE/imgdiff.swift" -o "$OUT/imgdiff"
"$OUT/reference" "$HERE" "$OUT"
python3 "$HERE/check.py" steps "$HERE" "$OUT"
APP="$OUT/YourHarness Dev.app"
for run in panel parked window; do
    rm -rf "$OUT/$run" "$OUT/data-$run"
    mkdir -p "$OUT/$run" "$OUT/data-$run"
    env=(--env "YH_DATA_DIR=$OUT/data-$run" --env "YH_PATH=$PATH" --env YH_NOTIFICATIONS=log --env "YH_BROWSER_PROBE=$OUT/steps.json")
    [[ $run == window ]] && env+=(--env YH_OPEN_BROWSER_WINDOW=1)
    [[ $run == parked ]] && echo '{"browserAutoShow": false}' > "$OUT/data-$run/state.json"
    : > "$OUT/$run.log"
    open -g -n $env --stderr "$OUT/$run.log" "$APP"
    for i in {1..120}; do grep -q '^== done' "$OUT/$run.log" && break; sleep 1; done
    pkill -f "$APP/Contents/MacOS/" || true
    sleep 1
    mv "$OUT"/steps.json.*.jpg "$OUT/$run/"
done
python3 "$HERE/check.py" report "$OUT" panel parked window
