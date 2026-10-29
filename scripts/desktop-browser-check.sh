#!/usr/bin/env bash
# In the pinned nix-shell, with Node >=22 and Chromium on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
node scripts/desktop-transport-check.mjs
scripts/build-desktop.sh
cargo build --locked -p cameo-pi
work=$(mktemp -d /tmp/cutcraft-desktop-check.XXXXXX)
server_pid= chrome_pid=
cleanup() {
  [[ -n $chrome_pid ]] && kill "$chrome_pid" 2>/dev/null || true
  [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  echo "Browser logs and authored-fixture screenshots: $work"
}
trap cleanup EXIT
port=18540 debug=18541
target/debug/cutcraft-web --practice --insecure-http --data "$work/data" --listen "127.0.0.1:$port" >"$work/server.log" 2>&1 &
server_pid=$!
"${CHROME:-chromium}" --headless=new --no-sandbox --disable-gpu --remote-debugging-port="$debug" --user-data-dir="$work/chrome" about:blank >"$work/chrome.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 50); do
  curl -sf "http://127.0.0.1:$debug/json/version" >/dev/null && curl -sf "http://127.0.0.1:$port/" >/dev/null && break
  sleep 0.2
done
node scripts/desktop-browser-check.mjs "http://127.0.0.1:$port" "$debug" "$PWD/crates/import/tests/fixtures/one-inch.svg" "$work"
