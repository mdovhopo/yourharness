#!/bin/zsh
# A workspace: a shell repo holding two clones it ignores, each a git repo of its own.
set -euo pipefail
ROOT="$1/platform"
commit() { git -C "$1" init -q; git -C "$1" add -A; git -C "$1" -c user.name=Preset -c user.email=preset@example.com commit -qm "$2"; }
mkdir -p "$ROOT/docs" "$ROOT/api/src" "$ROOT/api/dist" "$ROOT/web/src"
printf 'services:\n  api:\n    build: ./api\n  web:\n    build: ./web\n' > "$ROOT/docker-compose.yml"
printf '# Architecture\n\nThe web app calls the api.\n' > "$ROOT/docs/architecture.md"
printf 'api/\nweb/\n' > "$ROOT/.gitignore"
printf 'import { login } from "./auth";\n\nexport const routes = { "/api/login": login };\n' > "$ROOT/api/src/index.ts"
printf '// Checks a password.\n\nexport function login(user: string, password: string) {\n  return user.length > 0 && password.length > 0;\n}\n' > "$ROOT/api/src/auth.ts"
printf 'compiled\n' > "$ROOT/api/dist/server.js"
printf 'dist/\n' > "$ROOT/api/.gitignore"
printf 'import { signIn } from "./login";\n\nsignIn("me", "secret");\n' > "$ROOT/web/src/index.ts"
printf 'export async function signIn(user: string, password: string) {\n  return fetch("/api/login", { method: "POST", body: JSON.stringify({ user, password }) });\n}\n' > "$ROOT/web/src/login.ts"
commit "$ROOT/api" API
commit "$ROOT/web" Web
commit "$ROOT" Platform
