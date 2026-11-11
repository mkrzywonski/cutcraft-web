#!/usr/bin/env bash
# Build bundled offline desktop assets before compiling the server.
# Run inside the pinned nix-shell, which provides the matching wasm-bindgen CLI.
set -euo pipefail
cd "$(dirname "$0")/.."
version=0.2.129
bindgen=wasm-bindgen
if [[ $("$bindgen" --version) != "wasm-bindgen $version" ]]; then
  echo "Enter the pinned nix-shell: this build needs wasm-bindgen $version." >&2
  exit 1
fi
cargo build --release --locked -p cutcraft-ui --target wasm32-unknown-unknown
"$bindgen" --target web --out-dir apps/server/web/desktop/pkg target/wasm32-unknown-unknown/release/cutcraft_ui.wasm

# A release must not embed assets from a previous application version.
sed -n 's/^version = "\([0-9]*\.[0-9]*\.[0-9]*\)"/\1/p' Cargo.toml | head -n 1 > apps/server/web/desktop/pkg/bundle-version.txt
