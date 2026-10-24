#!/bin/zsh
# Makes a scratch data folder from a preset in presets/, for a Dev copy to run against
# (scripts/dev-run.sh). Paths in a preset are written as {{ROOT}}, the folder it's made in.
# Usage: scripts/preset.sh <preset> [dir]     (dir: test-harness-root/<preset>)
set -euo pipefail
ROOT="${0:A:h:h}"
NAME="${1:?usage: scripts/preset.sh <preset> [dir]   presets: $(ls "$ROOT/presets" | grep -v README | tr '\n' ' ')}"
SOURCE="$ROOT/presets/$NAME"
[[ -d $SOURCE ]] || { echo "No preset named $NAME" >&2; exit 1 }
DEST="${${2:-$ROOT/test-harness-root/$NAME}:A}"
rm -rf "$DEST"
mkdir -p "$DEST/home"
rsync -a --exclude setup.sh --exclude generate.py --exclude README.md "$SOURCE/" "$DEST/"
[[ -f $SOURCE/generate.py ]] && python3 -I "$SOURCE/generate.py" "$DEST" "${@:3}"
[[ -x $SOURCE/setup.sh ]] && "$SOURCE/setup.sh" "$DEST"
find "$DEST" -name '*.json' -not -path '*/.git/*' -type f -print0 | xargs -0 sed -i '' "s|{{ROOT}}|$DEST|g"
echo "$DEST"
