#!/bin/zsh
# Prints one version's notes from CHANGELOG.md: scripts/release-notes.sh 0.0.1 (or Unreleased).
set -euo pipefail

ROOT="${0:A:h:h}"
awk -v heading="## [${1:?usage: scripts/release-notes.sh <version>}]" '
    /^## \[/ { if (found) exit; if (index($0, heading) == 1) { found = 1; next } }
    found { lines[++n] = $0 }
    END {
        first = 1; while (first <= n && lines[first] ~ /^[[:space:]]*$/) first++
        last = n; while (last >= first && lines[last] ~ /^[[:space:]]*$/) last--
        for (i = first; i <= last; i++) print lines[i]
    }
' "$ROOT/CHANGELOG.md"
