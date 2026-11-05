#!/usr/bin/env bash
# Runs every authored issue example through the desktop against a fresh practice-only service.
# In the pinned nix-shell, with Node >=22 and Chromium on PATH. Optional argument: a fixture
# name prefix, e.g. `scripts/desktop-fixture-check.sh 07`.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-desktop.sh
cargo build --locked -p cameo-pi
work=$(mktemp -d /tmp/cutcraft-fixture-check.XXXXXX)
server_pid= chrome_pid=
cleanup() {
  [[ -n $chrome_pid ]] && kill "$chrome_pid" 2>/dev/null || true
  [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  echo "Results, browser logs and screenshots: $work"
}
trap cleanup EXIT
port=18550 debug=18551
target/debug/cutcraft-web --practice --insecure-http --data "$work/data" --listen "127.0.0.1:$port" >"$work/server.log" 2>&1 &
server_pid=$!
"${CHROME:-chromium}" --headless=new --no-sandbox --disable-gpu --remote-debugging-port="$debug" --user-data-dir="$work/chrome" about:blank >"$work/chrome.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 50); do
  curl -sf "http://127.0.0.1:$debug/json/version" >/dev/null && curl -sf "http://127.0.0.1:$port/" >/dev/null && break
  sleep 0.2
done
node scripts/desktop-fixture-check.mjs "http://127.0.0.1:$port" "$debug" "$PWD" "$work" "${1:-}"
