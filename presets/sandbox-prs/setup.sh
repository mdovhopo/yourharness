#!/bin/zsh
# Clones the sandbox repo PRs can be opened against (github.com/mdovhopo/yourharness-test-prs).
set -euo pipefail
cd "$1"
gh repo clone mdovhopo/yourharness-test-prs yourharness-test-prs -- --quiet
