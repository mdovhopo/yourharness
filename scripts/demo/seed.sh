#!/bin/zsh
# Demo data from real turns: each demo project opened in a scratch data folder and given the
# prompts in seed/<project>.json, one launch per project.
#   scripts/demo/seed.sh [data dir] [model]   (test-harness-root/demo/data, sonnet)
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
data=${1:-$root/test-harness-root/demo/data}
model=${2:-sonnet}
app="$root/test-harness-root/demo/app/YourHarness Next.app"
work=${DEMO_WORK:-/Users/Shared/Projects}
mkdir -p "$data/home"
for p in ${=PROJECTS:-atlas-api orbit lumen}; do
  log=$data/seed-$p.log
  echo "seeding $p → $log"
  open -W -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR="$data" --env YH_PATH="$PATH" \
    --env YH_NOTIFICATIONS=log --env YH_PROJECT="$work/$p" --env YH_NEW_THREAD=1 --env YH_MODEL="$model" \
    --env YH_DEMO="$here/${SEED:-seed}/$p.json" --stderr "$log" "$app"
  grep -E "DEMO done|panic" "$log" | tail -2
done
