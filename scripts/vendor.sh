#!/bin/zsh
# GPUI crates we patch, from crates.io plus patches/<crate>-<version>.patch, into vendor/
# (not committed; Cargo.toml's [patch.crates-io] points there). Run by scripts/bundle.sh.
# gpui-pre-macos: GPUI_KEEP_DRAWING=1 keeps drawing a window parked off screen (test runs).
set -euo pipefail
ROOT="${0:A:h:h}"
for spec in gpui-pre-macos:0.3.11; do
    name=${spec%%:*} version=${spec##*:}
    patch_file="$ROOT/patches/$name-$version.patch"
    stamp="$ROOT/vendor/$name/.yh-patched"
    sum=$(shasum "$patch_file" | cut -d' ' -f1)
    [[ -f $stamp && $(cat "$stamp") == "$sum" ]] && continue
    rm -rf "$ROOT/vendor/$name" "$ROOT/vendor/$name-$version"
    mkdir -p "$ROOT/vendor"
    curl -sSfL "https://static.crates.io/crates/$name/$name-$version.crate" | tar -xz -C "$ROOT/vendor"
    mv "$ROOT/vendor/$name-$version" "$ROOT/vendor/$name"
    (cd "$ROOT/vendor" && patch -s -p0 < "$patch_file")
    echo "$sum" > "$stamp"
    echo "vendor/$name: $version, patched"
done
