#!/usr/bin/env bash
# Runs the web interface's browser checks (scripts/browser-check.mjs) against a scratch server
# with only the practice cutter, in headless Chromium. Needs `chromium` (or CHROME=...) and Node
# 22 or newer; on NixOS: nix-shell -p nodejs chromium --run scripts/browser-check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

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

cargo build --locked -q -p cameo-pi
planner=$(cargo run --locked -q -p cameo-pi-core --features fixtures --example write_fixture -- "$work")

port=18490 debug=18491
target/debug/cutcraft-web --practice --insecure-http --data "$work/data" --listen "127.0.0.1:$port" \
  >"$work/server.log" 2>&1 &
server_pid=$!
"$chrome" --headless=new --no-sandbox --disable-gpu --remote-debugging-port=$debug \
  --user-data-dir="$work/chrome" about:blank >"$work/chrome.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 50); do
  curl -sf "http://127.0.0.1:$debug/json/version" >/dev/null && curl -sf "http://127.0.0.1:$port/" >/dev/null && break
  sleep 0.2
done

node scripts/browser-check.mjs "http://127.0.0.1:$port" "$debug" "$work" "$planner"
