#!/usr/bin/env bash
# Builds the slice's web shell into web-dist/: the wasm (size-optimised), its wasm-bindgen glue,
# the start script and the stylesheet, each named by its content hash (cached for good), and
# index.html (never cached). Inside `nix develop`, with wasm-bindgen 0.2.129 in ../../local/tools.
#   spike/m0/build-web.sh
set -euo pipefail
cd "$(dirname "$0")"
bindgen=${WASM_BINDGEN:-../../local/tools/bin/wasm-bindgen}
out=web-dist
rm -rf "$out.tmp" && mkdir -p "$out.tmp/assets"
cargo build --locked --profile wasm --target wasm32-unknown-unknown -p cutcraft-web-m0-ui
"$bindgen" --target web --no-typescript --out-dir "$out.tmp/gen" \
  target/wasm32-unknown-unknown/wasm/cutcraft_web_m0_ui.wasm
wasm="$out.tmp/gen/cutcraft_web_m0_ui_bg.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals "$wasm" -o "$wasm.opt" && mv "$wasm.opt" "$wasm"
fi
hashed() { # FILE STEM EXT -> the hashed name, copied into assets/
  local h; h=$(sha256sum "$1" | cut -c1-16)
  cp "$1" "$out.tmp/assets/$2-$h.$3"
  echo "$2-$h.$3"
}
w=$(hashed "$wasm" m0 wasm)
g=$(hashed "$out.tmp/gen/cutcraft_web_m0_ui.js" m0 js)
sed -e "s|@GLUE@|$g|" -e "s|@WASM@|$w|" web/start.js.in >"$out.tmp/start.js"
s=$(hashed "$out.tmp/start.js" start js)
c=$(hashed web/style.css style css)
sed -e "s|@START@|$s|" -e "s|@STYLE@|$c|" web/index.html >"$out.tmp/index.html"
rm -rf "$out.tmp/gen" "$out.tmp/start.js" "$out"
mv "$out.tmp" "$out"
raw=$(stat -c %s "$out/assets/$w")
gz=$(gzip -9 -c "$out/assets/$w" | wc -c)
br=$( (command -v brotli >/dev/null && brotli -c -q 11 "$out/assets/$w" | wc -c) || echo "?")
echo "Built $out: wasm $((raw / 1024)) KB, gzip $((gz / 1024)) KB, brotli $([[ $br == "?" ]] && echo "?" || echo $((br / 1024))) KB"
