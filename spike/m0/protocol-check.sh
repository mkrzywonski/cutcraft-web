#!/usr/bin/env bash
# The slice's protocol rules (spec §6, §7, §13), against a scratch server over HTTPS: sign-in and
# the site's own header, upload, commands with keys, incarnations and revisions, the allowlist, a
# lost reply found by key, pictures by revision, bad uploads, and a crashed worker replaced.
# Inside `nix develop`:  spike/m0/protocol-check.sh
set -euo pipefail
cd "$(dirname "$0")"
work=$(mktemp -d)
server_pid=
cleanup() { [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT
cargo build --locked --release -q -p cutcraft-web-m0
port=18494
CUTCRAFT_WEB_PASSWORD=pc target/release/cutcraft-web-m0 serve --data "$work/data" --web web \
  --listen "127.0.0.1:$port" >"$work/log" 2>&1 &
server_pid=$!
B=https://127.0.0.1:$port
for _ in $(seq 50); do curl -sk -o /dev/null "$B/" && break; sleep 0.1; done
c() { curl -sk -b "$work/jar" -c "$work/jar" -H 'x-cutcraft-protocol: 2' "$@"; }
code() { c -o /dev/null -w '%{http_code}' "$@"; }
failures=0
check() { if [[ $2 == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1: got $2, wanted $3"; failures=$((failures + 1)); fi; }
field() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval('v' + sys.argv[1]))" "$1"; }
cmd() { # ID PARAMS KEY INCARNATION REVISION [extra curl args]
  local body="{\"id\":\"$1\",\"params\":$2,\"key\":\"$3\",\"incarnation\":\"$4\"${5:+,\"revision\":$5}}"
  shift 5
  c -H 'x-cutcraft: 1' -H 'content-type: application/json' -d "$body" "$@" "$B/api/command"
}

check "another protocol version is told to reload" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'x-cutcraft-protocol: 99' $B/api/snapshot)" 409
check "a wrong password is refused" "$(code -H 'content-type: application/json' -d '{"password":"no"}' $B/api/login)" 401
check "the right one signs in" "$(code -H 'content-type: application/json' -d '{"password":"pc"}' $B/api/login)" 200
check "a change without the site's header is refused" "$(code --data-binary @fixtures/star-and-ring.svg "$B/api/upload?name=a.svg")" 403
check "a file that isn't a design is refused" "$(code -H 'x-cutcraft: 1' --data-binary 'x' "$B/api/upload?name=a.exe")" 400
snap=$(c -H 'x-cutcraft: 1' --data-binary @fixtures/star-and-ring.svg "$B/api/upload?name=star.svg")
check "the upload opens and is cut-ready" "$(field "['design']['cut_ready']" <<<"$snap")" True
inc=$(field "['incarnation']" <<<"$snap")
r1=$(cmd sheet.set '{"material":"Removable"}' k1 "$inc" 1)
check "a change with the right revision applies" "$(field "['answer']" <<<"$r1")" applied
check "the same key again answers the same" "$(cmd sheet.set '{"material":"Removable"}' k1 "$inc" 1)" "$r1"
check "the same key with another change is refused" "$(cmd sheet.set '{"material":"Permanent"}' k1 "$inc" 1 | field "['code']")" key-reused
check "a stale revision is refused" "$(cmd sheet.place '{"x":50,"y":40}' k2 "$inc" 1 | field "['code']")" stale-revision
check "a stale incarnation is refused" "$(cmd sheet.place '{"x":50,"y":40}' k3 old 2 | field "['code']")" stale-incarnation
check "a command that names a file is refused" "$(cmd project.save '{"path":"/tmp/x"}' k4 "$inc" "" | field "['code']")" forbidden
before=$(c $B/api/geometry | field "['art_box_mm'][0]")
check "a lost reply comes back as 503" "$(cmd sheet.place '{"x":20,"y":10}' k5 "$inc" 2 -o /dev/null -w '%{http_code}' -H 'x-m0-drop-reply: 1')" 503
check "…and is found by its key, applied" "$(c $B/api/lookup/k5 | field "['answer']")" applied
check "…and was applied once" "$(c $B/api/geometry | field "['art_box_mm'][0]")" "32.7"
check "an unknown key is not applied" "$(c $B/api/lookup/never | field "['answer']")" not_applied
art=$(c $B/api/snapshot | field "['design']['art']['id']")
pic() { echo "$B/api/picture?art=$1&x0=0&y0=0&x1=80&y1=60&zoom_step=0&width=$2&height=150"; }
check "a picture of the design open now" "$(code "$(pic "$art" 200)")" 200
check "a picture of no current design is refused" "$(code "$(pic "nope" 200)")" 404
check "an oversized picture is refused" "$(code "$(pic "$art" 9999)")" 404
check "an edit without a revision is refused" "$(cmd sheet.place '{"x":30,"y":30}' k7 "$inc" "" | field "['code']")" revision-required
check "a read without a revision is answered" "$(cmd rows.list '{}' k8 "$inc" "" | field "['answer']")" applied
check "app.status (server paths) isn't available to a browser" "$(cmd app.status '{}' k9 "$inc" "" | field "['code']")" forbidden
leaks=0
for id in rows.list materials.list mats.list plan.sheet; do
  reply=$(cmd "$id" '{}' "read-$id" "$inc" "")
  grep -q "$work" <<<"$reply" && { echo "     $id reply contains a server path"; leaks=$((leaks + 1)); }
done
# And the replies of the edits used above, and of history.
rev=$(c $B/api/snapshot | field "['revision']")
for reply in "$r1" "$(c $B/api/lookup/k5)" "$(cmd history.undo '{}' undo-scan "$inc" "$rev")"; do
  grep -q "$work" <<<"$reply" && { echo "     an edit's reply contains a server path"; leaks=$((leaks + 1)); }
done
check "no read, edit or history reply contains a server path" "$leaks" 0
# Codex's review: two designs of the same size both start at revision 1.
red='<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="60mm" viewBox="0 0 80 60"><rect x="10" y="10" width="50" height="40" fill="#ff0000"/></svg>'
blue=${red//#ff0000/#0000ff}
art_red=$(c -H 'x-cutcraft: 1' --data-binary "$red" "$B/api/upload?name=red.svg" | field "['design']['art']['id']")
art_blue=$(c -H 'x-cutcraft: 1' --data-binary "$blue" "$B/api/upload?name=blue.svg" | field "['design']['art']['id']")
check "two designs at the same revision have different artwork identities" "$([[ $art_red != "$art_blue" ]] && echo yes)" yes
check "the first design's picture is refused once the second is open" "$(code "$(pic "$art_red" 200)")" 404
check "the second design's picture is served" "$(code "$(pic "$art_blue" 200)")" 200
inc=$(c $B/api/snapshot | field "['incarnation']")
printf '<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L10' >"$work/broken.svg"
check "a broken design is refused, plainly" "$(code -H 'x-cutcraft: 1' --data-binary @"$work/broken.svg" "$B/api/upload?name=b.svg")" 422
worker=$(pgrep -a cutcraft-web-m0 | grep "worker --area $work" | cut -d' ' -f1)
kill -9 "$worker"
check "a crashed worker fails one request" "$(code $B/api/snapshot)" 500
check "…then a fresh one answers" "$(c $B/api/snapshot | field "['incarnation'] != '$inc'")" True
check "…and the old load's changes are refused" "$(cmd sheet.place '{"x":1,"y":1}' k6 "$inc" 0 | field "['code']")" stale-incarnation
echo
[[ $failures == 0 ]] && echo "all protocol checks passed" || { echo "$failures protocol check(s) failed"; exit 1; }
