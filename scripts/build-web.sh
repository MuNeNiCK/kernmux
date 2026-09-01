#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/web"

test -f "$frontend/pnpm-lock.yaml" || {
  echo "missing web/pnpm-lock.yaml" >&2
  exit 2
}

pnpm --dir "$frontend" install --frozen-lockfile
pnpm --dir "$frontend" run build

test -f "$repo_root/dist/web/index.html" || {
  echo "Vite did not produce dist/web/index.html" >&2
  exit 3
}
