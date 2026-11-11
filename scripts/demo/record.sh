#!/bin/zsh
# One scene recorded: the seeded data and demo repos put back as they were, then the app run on
# a copy with the scene's script.
#   scripts/demo/record.sh <scene.json> [appearance]   (light; scenes in scripts/demo/scenes)
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
demo=$root/test-harness-root/demo
scene=${1:A}
name=${scene:t:r}
appearance=${2:-light}
out=$demo/takes; mkdir -p "$out"
rsync -a --delete "$demo/work-seed/" "$demo/work/"
rm -rf "$demo/run"; cp -cR "$demo/data-seed" "$demo/run"
# The scene's paths: {{OUT}} is the takes folder, {{DEMO}} the demo folder.
sed -e "s#{{OUT}}#$out#g" -e "s#{{DEMO}}#$demo#g" -e "s#{{NAME}}#$name-$appearance#g" "$scene" > "$out/$name-$appearance.json"
open -W -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR="$demo/run" --env YH_PATH="$PATH" --env YH_NOTIFICATIONS=log \
  --env YH_WINDOW_SIZE=1440x900 --env YH_APPEARANCE="$appearance" ${=EXTRA_ENV:-} \
  --env YH_DEMO="$out/$name-$appearance.json" --stderr "$out/$name-$appearance.log" "$demo/app/YourHarness Next.app"
grep -E "DEMO done|DEMO no|DEMO bad|DEMO unknown|panic" "$out/$name-$appearance.log" || true
