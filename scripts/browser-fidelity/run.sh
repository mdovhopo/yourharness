#!/bin/zsh
# Checks that the built-in browser shows pages exactly as a plain, unscaled page of its engine
# does (a WKWebView for WebKit, a plain Chromium window for Chrome) at every page size, and that
# its input acts like a
# real mouse: hover, clicks, pointer drags, HTML drag and drop, scrolling. It runs three
# times: the page in the right sidebar (it opens there), parked (the sidebar kept closed:
# browserAutoShow off), and in a window of its own, each drawing it at a different scale.
# Each run is a test copy on a scratch data folder and an empty project, in the background,
# driving the browser with YH_BROWSER_PROBE (the agent's own tool calls).
# Usage: scripts/browser-fidelity/run.sh [out dir]
#   ENGINE=webkit  the WebKit engine (Chrome, the default, otherwise); run both after a change
#   SKIP_BUILD=1   reuse <out dir>/YourHarness Next.app
#   RUNS="panel"   only some of panel, parked, window
#   YH_CEF_FRAMEWORK=<folder>  the Chrome engine's framework, not downloaded (~/.cache's by default)
set -euo pipefail
HERE=${0:A:h}
ROOT=${HERE:h:h}
OUT=${${1:-$(mktemp -d)}:A}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
mkdir -p "$OUT"
APP="$OUT/YourHarness Next.app"
[[ -n ${SKIP_BUILD:-} && -d $APP ]] || APP="$APP" FEATURES=snapshots "$ROOT/scripts/bundle.sh" >/dev/null
ENGINE=${ENGINE:-chrome}
CEF=${YH_CEF_FRAMEWORK:-$(print -r -- $HOME/.cache/yourharness-cef/*/cef_macos_aarch64(N[1]))}
engine=(--env "YH_BROWSER_ENGINE=$ENGINE")
[[ -n $CEF ]] && engine+=(--env "YH_CEF_FRAMEWORK=$CEF")
swiftc -O "$HERE/imgdiff.swift" -o "$OUT/imgdiff"
if [[ $ENGINE == webkit ]]; then
    swiftc -O "$HERE/reference.swift" -o "$OUT/reference"
    "$OUT/reference" "$HERE" "$OUT"
else
    # The app's own Chrome engine, a plain page at each size (webview/chrome.rs `reference`).
    mkdir -p "$OUT/data-reference"
    : > "$OUT/reference.log"
    open -g -n --env YH_BACKGROUND=1 --env "YH_DATA_DIR=$OUT/data-reference" --env "YH_PATH=$PATH" --env YH_NOTIFICATIONS=log \
        $engine --env "YH_CEF_REFERENCE=$HERE:$OUT" --stderr "$OUT/reference.log" "$APP"
    for i in {1..120}; do grep -q '^REFERENCE done' "$OUT/reference.log" && break; sleep 1; done
    sleep 1
fi
python3 -I "$HERE/check.py" steps "$HERE" "$OUT"
RUNS=(${=RUNS:-panel parked window})
for run in $RUNS; do
    for d in "$OUT/$run" "$OUT/data-$run" "$OUT/project-$run"; do [[ -d $d ]] && rm -r "$d"; done
    mkdir -p "$OUT/$run" "$OUT/data-$run" "$OUT/project-$run"
    env=(--env YH_BACKGROUND=1 --env "YH_DATA_DIR=$OUT/data-$run" --env "YH_PATH=$PATH" --env YH_NOTIFICATIONS=log
         --env "YH_PROJECT=$OUT/project-$run" --env "YH_BROWSER_PROBE=$OUT/steps.json" --env YH_BROWSER_PROBE_QUIT=1 $engine)
    [[ $run == window ]] && env+=(--env YH_OPEN_BROWSER_WINDOW=1 --env YH_DRAFT_DELAY=0.5)
    [[ $run == parked ]] && env+=(--env YH_BROWSER_AUTO_SHOW=0)
    : > "$OUT/$run.log"
    open -g -n $env --stderr "$OUT/$run.log" "$APP"
    for i in {1..180}; do grep -q '^== done' "$OUT/$run.log" && break; sleep 1; done
    sleep 2
    # Whatever's left of this run's copy, by its data folder (never by name: others run theirs).
    for pid in $(lsof -t "$OUT/data-$run/harness.sqlite" 2>/dev/null); do kill $pid 2>/dev/null || true; done
    sleep 1
    for f in "$OUT"/steps.json.*.jpg(N); do mv "$f" "$OUT/$run/"; done
done
python3 -I "$HERE/check.py" report "$OUT" $RUNS
