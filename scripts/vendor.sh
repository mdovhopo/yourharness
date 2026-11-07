#!/bin/zsh
# GPUI crates we patch, from crates.io plus patches/<crate>-<version>.patch, into vendor/
# (not committed; Cargo.toml's [patch.crates-io] points there). Run by scripts/bundle.sh.
# gpui-pre-macos: GPUI_KEEP_DRAWING=1 keeps drawing a window parked off screen (test runs).
#
# Each crate is checked against crates.io's SHA-256 (its index's `cksum`), taken from cargo's
# own download cache when it's there, else downloaded with retries; a failed run leaves no
# half-unpacked crate behind, since the stamp is only written once it's patched.
set -euo pipefail
ROOT="${0:A:h:h}"
# name:version:sha256
for spec in gpui-pre-macos:0.3.11:d7410cfdaa1fa55a414346cc492d18ce340e3a8b6d0ac3213b984ed690c82b92; do
    name=${spec%%:*} rest=${spec#*:}
    version=${rest%%:*} cksum=${rest#*:}
    patch_file="$ROOT/patches/$name-$version.patch"
    stamp="$ROOT/vendor/$name/.yh-patched"
    sum=$(shasum "$patch_file" | cut -d' ' -f1)
    [[ -f $stamp && $(<"$stamp") == "$sum" ]] && continue
    tmp="$(mktemp -d)"
    crate="$tmp/$name-$version.crate"
    cached=(${CARGO_HOME:-$HOME/.cargo}/registry/cache/*/$name-$version.crate(N))
    if (( ${#cached} )) && [[ "$(shasum -a 256 "${cached[1]}" | cut -d' ' -f1)" == "$cksum" ]]; then
        cp "${cached[1]}" "$crate"
    else
        curl -sSfL --retry 5 --retry-all-errors --retry-delay 2 --connect-timeout 20 --max-time 300 \
            -o "$crate" "https://static.crates.io/crates/$name/$name-$version.crate"
    fi
    got="$(shasum -a 256 "$crate" | cut -d' ' -f1)"
    [[ "$got" == "$cksum" ]] || { echo "vendor/$name: $name-$version.crate's SHA-256 is $got, crates.io says $cksum" >&2; rm -rf "$tmp"; exit 1; }
    tar -xzf "$crate" -C "$tmp"
    mv "$tmp/$name-$version" "$tmp/$name"
    (cd "$tmp" && patch -s -p0 < "$patch_file")
    rm -rf "$ROOT/vendor/$name" "$ROOT/vendor/$name-$version"
    mkdir -p "$ROOT/vendor"
    mv "$tmp/$name" "$ROOT/vendor/$name"
    rm -rf "$tmp"
    echo "$sum" > "$stamp"
    echo "vendor/$name: $version, patched"
done
