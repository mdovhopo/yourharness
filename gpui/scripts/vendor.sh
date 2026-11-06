#!/bin/zsh
# GPUI from crates.io with our patches (patches/), into vendor/gpui, which Cargo.toml points at.
# The patches: keep drawing when the window is parked off screen (GPUI_KEEP_DRAWING=1), and
# a drawn frame into a PNG (GPUI_SNAPSHOTS=1 + gpui::SNAPSHOT_REQUEST), since `screencapture`
# puts a privacy prompt on the screen.
set -euo pipefail
DIR="${0:A:h:h}"
VERSION=0.2.2
STAMP="$DIR/vendor/gpui/.yh-patched"
SUM=$(shasum "$DIR/patches/gpui-$VERSION.patch" | cut -d' ' -f1)
[[ -f $STAMP && $(cat "$STAMP") == "$SUM" ]] && exit 0
rm -rf "$DIR/vendor/gpui" && mkdir -p "$DIR/vendor"
curl -sSfL "https://static.crates.io/crates/gpui/gpui-$VERSION.crate" | tar -xz -C "$DIR/vendor"
mv "$DIR/vendor/gpui-$VERSION" "$DIR/vendor/gpui"
(cd "$DIR/vendor" && mkdir -p gpui-orig && patch -s -p0 < "$DIR/patches/gpui-$VERSION.patch")
rmdir "$DIR/vendor/gpui-orig" 2>/dev/null || true
echo "$SUM" > "$STAMP"
echo "vendor/gpui: gpui $VERSION, patched"
