#!/usr/bin/env bash
# Design sessions against the real server binary, with its workers as child processes (spec §5,
# §6, §7): the protocol version, upload and open by owned id, keyed changes, stale revisions and
# incarnations, the authority policy, pictures by artwork identity, unsaved work, Save and its
# conflicts, a crashed worker coming back from its draft, and a lost change looked up truthfully.
# The server's own tests cover the same rules with workers on threads.
#   nix develop -c scripts/design-check.sh
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d)
server_pid=
cleanup() { [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT
cargo build --locked -q -p cameo-pi
port=18495
target/debug/cameo-pi --practice --insecure-http --data "$work/data" --listen "127.0.0.1:$port" \
  >"$work/log" 2>&1 &
server_pid=$!
B=http://127.0.0.1:$port
for _ in $(seq 50); do curl -s -o /dev/null "$B/" -H 'Host: localhost' && break; sleep 0.1; done
H=(-H 'Host: localhost' -H 'Origin: http://localhost' -H 'Cookie: cpi_csrf=t' -H 'x-csrf-token: t'
  -H 'x-cutcraft-protocol: 4')
c() { curl -s "${H[@]}" "$@"; }
code() { curl -s -o /dev/null -w '%{http_code}' "${H[@]}" "$@"; }
failures=0
check() { if [[ $2 == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1: got $2, wanted $3"; failures=$((failures + 1)); fi; }
field() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval('v' + sys.argv[1]))" "$1"; }
now() { c "$B/api/v1/design/snapshot"; }
post() { c -H 'content-type: application/json' -d "$2" "$B/api/v1/design/$1"; }
cmd() { # ID PARAMS KEY [REVISION|none]
  local s inc rev; s=$(now); inc=$(field "['incarnation']" <<<"$s"); rev=$(field "['revision']" <<<"$s")
  [[ ${4:-} == none ]] && rev=null
  post command "{\"id\":\"$1\",\"params\":$2,\"key\":\"$3\",\"incarnation\":\"$inc\",\"revision\":$rev}"
}
open_upload() { # ID KEY [replace]
  local s; s=$(now)
  post open "{\"design\":\"$1\",\"key\":\"$2\",\"incarnation\":\"$(field "['incarnation']" <<<"$s")\",\"revision\":$(field "['revision']" <<<"$s")${3:+,\"replace_unsaved\":true}}"
}
worker() { pgrep -a cameo-pi | grep "worker --area $work" | cut -d' ' -f1; }

check "another protocol version is refused first" "$(curl -s -o /dev/null -w '%{http_code}' "${H[@]:0:8}" -H 'x-cutcraft-protocol: 99' "$B/api/v1/design/snapshot")" 409
up=$(c -H 'content-type: application/octet-stream' --data-binary @crates/import/tests/fixtures/one-inch.svg "$B/api/v1/design/upload?name=one-inch.svg")
id=$(field "['design']['id']" <<<"$up")
check "an upload is named by an owned id" "$([[ $up != *"$work"* && -n $id ]] && echo yes)" yes
check "it opens by that id" "$(open_upload "$id" o1 | field "['answer']")" applied
check "the worker is a child process" "$([[ -n $(worker) ]] && echo yes)" yes
check "a change with its revision applies" "$(cmd sheet.set '{"material":"Removable"}' k1 | field "['answer']")" applied
check "an edit without a revision is refused" "$(cmd sheet.place '{"x":20,"y":20}' k2 none | field "['code']")" revision-required
check "a command that names files is refused" "$(cmd project.save '{"path":"/tmp/x"}' k3 | field "['code']")" forbidden
check "a cutter command is refused" "$(cmd cut.start '{}' k4 | field "['code']")" forbidden
art=$(now | field "['design']['art']['id']")
check "a picture of the open design" "$(code "$B/api/v1/design/picture?art=$art&x0=0&y0=0&x1=30&y1=30&zoom_step=0&width=100&height=100")" 200
check "a picture of no current design is refused" "$(code "$B/api/v1/design/picture?art=nope&x0=0&y0=0&x1=30&y1=30&zoom_step=0&width=100&height=100")" 404
check "opening over unsaved work asks first" "$(open_upload "$id" o2 | field "['code']")" unsaved-changes
save_as=$(post save "{\"key\":\"s1\",\"incarnation\":\"$(now | field "['incarnation']")\",\"revision\":$(now | field "['revision']"),\"as\":{\"folder\":[],\"name\":\"One inch\"}}")
check "Save As publishes a library project" "$(field "['answer']" <<<"$save_as")" applied
check "…and nothing is unsaved" "$(now | field "['design']['unsaved']")" False

# Preparing a copy goes through the registry in the real child. It is independently stored.
copied=$(cmd cut.copy '{}' copy1)
copy_id=$(field "['value']['copy']['id']" <<<"$copied")
copy_digest=$(field "['value']['copy']['digest']" <<<"$copied")
check "cut.copy prepares an immutable copy" "$(field "['answer']" <<<"$copied")" applied
cmd sheet.adjust '{"force":22}' copy_edit >/dev/null
copy2=$(cmd cut.copy '{}' copy2)
check "a later edit makes a different cut copy" "$([[ $(field "['value']['copy']['digest']" <<<"$copy2") != "$copy_digest" ]] && echo yes)" yes
check "the loading view still names the original copy" "$(c "$B/api/v1/jobs/$copy_id" | field "['revision']['digest']")" "$copy_digest"
check "copy previews are available" "$(code "$B/api/v1/jobs/$copy_id/revisions/1/preview")" 200
check "a cut copy does not expose the source project" "$(code "$B/api/v1/jobs/$copy_id/revisions/1/project")" 404

# A worker that crashes after its draft comes back from it; a change after the draft is lost and
# looked up as not applied.
cmd sheet.set '{"material":"Permanent"}' k5 >/dev/null
sleep 6
cmd sheet.set '{"material":"Siser EasyWeed"}' k6 >/dev/null
old=$(now | field "['incarnation']")
kill -9 "$(worker)"
check "a crashed worker fails one request" "$(code "$B/api/v1/design/snapshot")" 503
recovered=$(now)
check "…then comes back from its draft" "$(field "['design']['sheet']['material']" <<<"$recovered")" "Permanent vinyl (e.g. Oracal 651)"
check "…under a new incarnation" "$([[ $(field "['incarnation']" <<<"$recovered") != "$old" ]] && echo yes)" yes
check "the change in the draft is looked up as applied" "$(c "$B/api/v1/design/lookup/k5" | field "['answer']")" applied
check "the change after the draft is looked up as not applied" "$(c "$B/api/v1/design/lookup/k6" | field "['answer']")" not_applied
check "the saved revision came back with it" "$(field "['design']['saved']['revision']" <<<"$recovered")" 1
echo
[[ $failures == 0 ]] && echo "all design checks passed" || { echo "$failures design check(s) failed"; exit 1; }
