#!/bin/zsh
# Demo projects as git repos with a little history, in a folder of their own:
#   scripts/demo/make-workspace.sh <dir>     (default test-harness-root/demo/work)
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
dir=${1:-$root/test-harness-root/demo/work}
rm -rf "$dir"; mkdir -p "$dir"
commit() { GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" git -c user.name="Alex Rivera" -c user.email="alex@example.com" commit -qm "$2"; }
for p in lumen orbit atlas-api; do
  cp -R "$here/projects/$p" "$dir/$p"
  cd "$dir/$p"
  git init -q -b main
  printf 'node_modules/\npublic/\ntarget/\n__pycache__/\n' > .gitignore
  git add README.md .gitignore; commit "2026-10-01T10:00:00" "Start $p"
  git add -A; commit "2026-10-20T16:30:00" "First working version"
done
echo "$dir"
