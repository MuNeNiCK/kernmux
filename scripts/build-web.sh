#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

rust_toolchain="nightly-2026-08-24"
bindgen_version="0.2.126"
bindgen_bin="${KERNMUX_WASM_BINDGEN:-$repo_root/target/web-tools/bin/wasm-bindgen}"

if [[ ! -x "$bindgen_bin" ]] || [[ "$($bindgen_bin --version 2>/dev/null || true)" != "wasm-bindgen $bindgen_version" ]]; then
  cargo +1.98.0 install \
    --root "$repo_root/target/web-tools" \
    --version "=$bindgen_version" \
    --locked \
    wasm-bindgen-cli
fi

cargo +"$rust_toolchain" build \
  --locked \
  --release \
  --target wasm32-unknown-unknown \
  -p kernmux-web \
  --bin web \
  --features web

install -d "$repo_root/dist/web"
"$bindgen_bin" \
  "$repo_root/target/wasm32-unknown-unknown/release/web.wasm" \
  --target web \
  --out-dir "$repo_root/dist/web" \
  --out-name app
install -m 0644 "$repo_root/web/index.html" "$repo_root/dist/web/index.html"
install -m 0644 "$repo_root/web/bootstrap.js" "$repo_root/dist/web/bootstrap.js"
