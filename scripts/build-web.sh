#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/console-ui"

if [[ ! -f "$frontend/pnpm-lock.yaml" ]]; then
  echo "missing console-ui/pnpm-lock.yaml" >&2
  exit 2
fi

pnpm --dir "$frontend" install --frozen-lockfile
pnpm --dir "$frontend" run build

test -f "$repo_root/dist/web/index.html" || {
  echo "Vite did not produce dist/web/index.html" >&2
  exit 3
}
