#!/usr/bin/env bash
# Runs the slice's browser check (browser-check.mjs) against a scratch server, over HTTPS, in
# headless Chromium. Inside `nix develop`, with Node 22+ and Chromium:
#   nix develop -c nix-shell -p nodejs chromium binaryen --run spike/m0/browser-check.sh
set -euo pipefail
cd "$(dirname "$0")"

chrome=${CHROME:-chromium}
work=$(mktemp -d)
server_pid= chrome_pid=
cleanup() {
  [[ -n $chrome_pid ]] && kill "$chrome_pid" 2>/dev/null || true
  [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

./build-web.sh
cargo build --locked --release -q -p cutcraft-web-m0
port=18492 debug=18493 password=browser-check
CUTCRAFT_WEB_PASSWORD=$password target/release/cutcraft-web-m0 serve --data "$work/data" \
  --web web-dist --listen "127.0.0.1:$port" >"$work/server.log" 2>&1 &
server_pid=$!
# SwiftShader stands in for a GPU, so WebGL2 works headless; the certificate is the server's own.
"$chrome" --headless=new --no-sandbox --ignore-certificate-errors --use-angle=swiftshader \
  --window-size=1280,800 --force-device-scale-factor=2 \
  --enable-unsafe-swiftshader --remote-debugging-port=$debug --user-data-dir="$work/chrome" \
  about:blank >"$work/chrome.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 100); do
  curl -sf "http://127.0.0.1:$debug/json/version" >/dev/null && curl -skf "https://127.0.0.1:$port/" >/dev/null && break
  sleep 0.2
done

# The serving headers (spec §13), straight from the server.
wasm=$(ls web-dist/assets/*.wasm | head -1)
headers=$(curl -sk -o /dev/null -D - -H 'accept-encoding: br, gzip' "https://127.0.0.1:$port/assets/$(basename "$wasm")")
grep -qi '^content-type: application/wasm' <<<"$headers" && echo "ok   wasm served as application/wasm" || { echo "FAIL content type"; exit 1; }
grep -qi '^content-encoding: \(br\|gzip\)' <<<"$headers" && echo "ok   wasm compressed ($(grep -i '^content-encoding' <<<"$headers" | tr -d '\r'))" || { echo "FAIL not compressed"; exit 1; }
grep -qi '^cache-control: .*immutable' <<<"$headers" && echo "ok   hashed files cached as immutable" || { echo "FAIL cache-control"; exit 1; }
grep -qi "^content-security-policy: .*wasm-unsafe-eval" <<<"$headers" && echo "ok   the CSP allows wasm and nothing inline" || { echo "FAIL CSP"; exit 1; }

node browser-check.mjs "https://127.0.0.1:$port" "$debug" fixtures/star-and-ring.svg "$password" || {
  echo "--- server log"; tail -20 "$work/server.log"; exit 1
}
