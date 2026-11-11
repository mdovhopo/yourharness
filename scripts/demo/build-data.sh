#!/bin/zsh
# The demo's data from scratch, a few minutes of real turns: the projects as git repos in
# /Users/Shared/Projects, threads, memories and a task from seed/ and seed2/, then tidied (the
# Memory page's "Two memories" notice answered) and kept as data-seed and work-seed, which every
# take starts from. Pair the demo Simulator after (pair-phone.sh).
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
demo=$root/test-harness-root/demo
work=${DEMO_WORK:-/Users/Shared/Projects}
app="$demo/app/YourHarness Next.app"
mkdir -p "$demo"; touch "$demo/.metadata_never_index"
"$here/make-workspace.sh" "$work"
rm -rf "$demo/data"
"$here/seed.sh" "$demo/data" sonnet
SEED=seed2 "$here/seed.sh" "$demo/data" haiku
# The tidy-up: the memory notice answered "Not Now" in each project's Memory page.
cat > "$demo/tidy.json" <<JSON
{"record": false, "lead": 2.5, "tail": 1, "steps": [
 {"select": "Lumen dark mode", "after": 1}, {"key": "shift-cmd-m", "after": 2}, {"click": [493, 213], "after": 1},
 {"select": "Bug Review", "after": 1}, {"key": "shift-cmd-m", "after": 2}, {"click": [493, 213], "after": 1},
 {"select": "Compare API", "after": 1}, {"key": "shift-cmd-m", "after": 2}, {"click": [493, 213], "after": 1}
]}
JSON
open -W -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR="$demo/data" --env YH_PATH="$PATH" --env YH_NOTIFICATIONS=log \
  --env YH_WINDOW_SIZE=1440x900 --env YH_DEMO="$demo/tidy.json" --stderr "$demo/tidy.log" "$app"
rm -rf "$demo/data-seed" "$demo/work-seed"
cp -cR "$demo/data" "$demo/data-seed"
cp -cR "$work" "$demo/work-seed"
sqlite3 "$demo/data-seed/harness.sqlite" "select json_extract(json,'\$.title') from threads"
