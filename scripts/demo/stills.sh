#!/bin/zsh
# Framed screenshots from a recorded stills scene: each caption in it names a still, taken just
# before the caption ends.
#   scripts/demo/stills.sh <take.mp4> <out dir> [theme] [width]
set -euo pipefail
here=${0:A:h}
take=${1:A}
out=${2:A}
theme=${3:-light}
width=${4:-2560}
mkdir -p "$out"
py=${PYTHON:-/tmp/yh-media-venv/bin/python}
python3 -c '
import json, sys
for c in json.load(open(sys.argv[1]))["captions"]:
    print(c["text"], max(c["start"], c["end"] - 0.25))
' "$take.captions.json" | while read -r name at; do
  "$py" -I "$here/compose.py" --still "$take" "$at" "$out/$name-$theme.png" "$theme" "$width"
done
