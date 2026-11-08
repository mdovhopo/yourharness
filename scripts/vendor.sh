#!/bin/zsh
# GPUI crates we patch, from crates.io plus patches/<crate>-<version>.patch, into vendor/
# (not committed; Cargo.toml's [patch.crates-io] points there). Run by scripts/bundle.sh.
# gpui-pre-macos: GPUI_KEEP_DRAWING=1 keeps drawing a window parked off screen (test runs).
# gpui-pre: cached views draw anew while VoiceOver listens (a reused one left its tree).
# gpui-component: an icon-only button is named by its tooltip for VoiceOver; menus on the app's type
# scale (13 pt rows, 22 pt tall), a hairline edge, one shadow, a soft lit row (`list_active`).
#
# Each crate is checked against crates.io's SHA-256 (its index's `cksum`), taken from cargo's
# own download cache when it's there, else downloaded with retries; a failed run leaves no
# half-unpacked crate behind, since the stamp is only written once it's patched.
set -euo pipefail
ROOT="${0:A:h:h}"
# name:version:sha256
for spec in gpui-pre-macos:0.3.11:d7410cfdaa1fa55a414346cc492d18ce340e3a8b6d0ac3213b984ed690c82b92 \
    gpui-pre:0.3.11:9303c771abf2acff2a96d706f69d9a82088c4770173a07cce88f0002431d887d \
    gpui-component:0.9.0:45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb; do
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
