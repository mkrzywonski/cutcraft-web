#!/usr/bin/env bash
# Backs up a scratch cameo-pi (practice cutter only) to a scratch OpenSSH server on 127.0.0.1:2222,
# run as you with its own host key, and checks the whole cycle: key, identity, Test connection,
# incremental backups, a removed job, restore, encryption, another Pi's folder and a changed
# identity (spec §15). Nothing is installed; your own SSH setup isn't touched.
#   nix develop -c nix-shell -p openssh python3 --run scripts/backup-check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

work=$(mktemp -d)
sshd_pid= server_pid=
cleanup() {
  [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null || true
  [[ -n $sshd_pid ]] && kill "$sshd_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

cargo build --locked -q -p cameo-pi
planner=$(cargo run --locked -q -p cameo-pi-core --features fixtures --example write_fixture -- "$work")

start_sshd() {
  cat >"$work/sshd_config" <<CONF
Port 22320
ListenAddress 127.0.0.1
HostKey $work/host_key
AuthorizedKeysFile $work/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
StrictModes no
PidFile $work/sshd.pid
Subsystem sftp internal-sftp
CONF
  "$(command -v sshd)" -D -f "$work/sshd_config" -E "$work/sshd.log" &
  sshd_pid=$!
  sleep 1
}
ssh-keygen -q -t ed25519 -N "" -f "$work/host_key"
touch "$work/authorized_keys"
start_sshd

port=18552
target/debug/cameo-pi --practice --data "$work/data" --listen "127.0.0.1:$port" >"$work/server.log" 2>&1 &
server_pid=$!
sleep 2

WORK="$work" PORT=$port PLANNER="$planner" USER_NAME="$(id -un)" python3 -I - <<'PY'
import json, os, ssl, time, urllib.request, http.cookiejar, subprocess, signal, shutil
work, port, planner, user = os.environ["WORK"], os.environ["PORT"], os.environ["PLANNER"], os.environ["USER_NAME"]
base = f"https://localhost:{port}"
ctx = ssl._create_unverified_context(); jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx), urllib.request.HTTPCookieProcessor(jar))
op.open(base + "/").read()
def call(method, path, body=None, expect=200):
    token = next(c.value for c in jar if c.name == "cpi_csrf")
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method,
        headers={"Content-Type": "application/json", "Origin": base, "x-csrf-token": token})
    try:
        r = op.open(req); status, text = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read()
    value = json.loads(text or b"null")
    assert status == expect, f"{method} {path}: {status} {value}"
    return value
def ok(what):
    print("ok  ", what)
def upload(name):
    boundary = "b0undary"
    parts = []
    for field, value in [("folder", b""), ("name", name.encode()), ("planner", planner.encode()), ("key", name.encode()),
                         ("project", open(f"{work}/square.cutcraft", "rb").read()), ("plan", open(f"{work}/square-plan.json", "rb").read())]:
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{field}"; filename="{field}"\r\n\r\n'.encode() + value + b"\r\n")
    body = b"".join(parts) + f"--{boundary}--\r\n".encode()
    token = next(c.value for c in jar if c.name == "cpi_csrf")
    req = urllib.request.Request(base + "/api/v1/jobs", data=body, method="POST",
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}", "Origin": base, "x-csrf-token": token})
    return json.loads(op.open(req).read())
def run():
    call("POST", "/api/v1/backup/run")
    for _ in range(100):
        time.sleep(0.2)
        v = call("GET", "/api/v1/backup")
        if not v["running"] and v["last_attempt"]:
            return v
    raise AssertionError("the backup didn't finish")

first = upload("Square")
call("POST", "/api/v1/library/folders", {"name":"Empty", "parent":""})
call("POST", "/api/v1/library/folders", {"name":"Nested", "parent":"Empty"})
folder = f"{work}/backup"
call("PATCH", "/api/v1/backup", {"host": "127.0.0.1", "port": 22320, "user": user, "folder": folder, "every": "manual"})
key = call("POST", "/api/v1/backup/key")["public_key"]
open(f"{work}/authorized_keys", "w").write(key + "\n")
private = f"{work}/data/private/backup"
call("PATCH", "/api/v1/backup", {"sign_in": "password", "password": "a backup password"})
assert "a backup password" not in open(f"{private}/settings.json").read()
assert "a backup password" in open(f"{private}/secrets.json").read()
assert call("GET", "/api/v1/backup")["has_password"]
call("PATCH", "/api/v1/backup", {"sign_in": "key"})
modes = {n: oct(os.stat(f"{private}/{n}").st_mode & 0o777) for n in os.listdir(private)}
assert all(m == "0o600" for m in modes.values()) and oct(os.stat(private).st_mode & 0o777) == "0o700", modes
ok("the key, settings and password are cameo-pi's alone, the password apart from the settings")
t = call("POST", "/api/v1/backup/test")
assert not t["ok"] and t["identity_unknown"]["fingerprint"].startswith("SHA256:"), t
ok("a first connection asks to accept the computer's identity")
call("POST", "/api/v1/backup/identity", t["identity_unknown"])
t = call("POST", "/api/v1/backup/test")
assert t["ok"] and t["created"] and os.path.isdir(folder), t
ok("Test connection signs in with the key and makes the folder")

v = run()
assert v["last_attempt"]["ok"], v["last_attempt"]
assert os.path.exists(f"{folder}/cameo-pi-backup.json") and os.path.exists(f"{folder}/manifest.json")
assert os.path.exists(f"{folder}/library/Square/job.json")
assert "5 files sent" in v["last_attempt"]["message"], v["last_attempt"]
ok("the first backup sends every file, with the marker and the manifest")
v = run()
assert "nothing had changed" in v["last_attempt"]["message"], v["last_attempt"]
ok("a second backup sends nothing when nothing changed")
call("PATCH", f"/api/v1/jobs/{first['id']}", {"based_on": first["details_revision"], "notes": "a note"})
second = upload("Top")
v = run()
assert "6 files sent" in v["last_attempt"]["message"], v["last_attempt"]
ok("a changed job and a new one send only their files")
call("DELETE", f"/api/v1/jobs/{second['id']}")
v = run()
removed = [d for d in os.listdir(f"{folder}/removed")]
assert removed and os.path.exists(f"{folder}/removed/{removed[0]}/library/Top/job.json"), removed
assert not os.path.exists(f"{folder}/library/Top")
ok("a deleted job moves to the backup's removed folder")

# Restore: the backup replaces the library.
call("DELETE", f"/api/v1/jobs/{first['id']}")
summary = call("POST", "/api/v1/backup/restore", {})
assert summary["jobs"] == 1, summary
call("POST", "/api/v1/backup/restore/apply")
listing = call("GET", "/api/v1/library")
assert [j["name"] for j in listing["jobs"]] == ["Square"], listing
assert os.path.isdir(f"{work}/data/library/Empty/Nested")
ok("Restore fetches the whole backup, including nested empty folders")

def checkpoint():
    return sorted(os.listdir(folder + "/commits"))[-1]

# Refuse publication after new files are uploaded. The last committed snapshot must survive.
before = checkpoint()
interrupted = upload("Interrupted")
os.chmod(folder + "/commits", 0o500)
try:
    v = run()
    assert not v["last_attempt"]["ok"] and v["next_retry"] is None, v
    assert checkpoint() == before
    assert call("POST", "/api/v1/backup/restore", {})["jobs"] == 1
    call("DELETE", "/api/v1/backup/restore")
finally:
    os.chmod(folder + "/commits", 0o700)
assert run()["last_attempt"]["ok"]
ok("failed publication leaves the previous plain backup restorable; explicit retry succeeds")

# Removing files is also a new generation, never a move out of the old snapshot.
call("DELETE", f"/api/v1/jobs/{interrupted['id']}")
before = checkpoint()
os.chmod(folder + "/commits", 0o500)
try:
    assert not run()["last_attempt"]["ok"]
    assert checkpoint() == before
    assert call("POST", "/api/v1/backup/restore", {})["jobs"] == 2
    call("DELETE", "/api/v1/backup/restore")
finally:
    os.chmod(folder + "/commits", 0o700)
assert run()["last_attempt"]["ok"]
ok("a failed retirement preserves the old snapshot and can be retried")

# A convenience-mirror failure after commit must be visible without damaging restore.
job = call("GET", f"/api/v1/jobs/{first['id']}")["details"]
call("PATCH", f"/api/v1/jobs/{job['id']}", {"based_on":job["details_revision"], "notes":"new committed note"})
before = checkpoint()
os.chmod(folder + "/library/Square", 0o500)
try:
    v = run()
    assert v["last_attempt"]["ok"] and v["health"] == "warning", v
    assert checkpoint() != before
    assert call("POST", "/api/v1/backup/restore", {})["jobs"] == 1
    call("DELETE", "/api/v1/backup/restore")
finally:
    os.chmod(folder + "/library/Square", 0o700)
assert run()["health"] == "ok"
assert json.load(open(folder + "/library/Square/job.json"))["notes"] == "new committed note"
ok("a post-commit mirror failure warns, restore uses immutable files, and retry repairs it")

# A dropped connection while writing the first ownership marker leaves only a recognisable part.
partial = work + "/partial-owner"
os.mkdir(partial)
part_name = "cameo-pi-backup.json.0123456789abcdef.part"
open(partial + "/" + part_name, "w").write("incomplete")
call("PATCH", "/api/v1/backup", {"folder":partial})
assert run()["last_attempt"]["ok"]
assert not os.path.exists(partial + "/" + part_name)
call("PATCH", "/api/v1/backup", {"folder":folder})
ok("an interrupted first ownership marker does not block retry or hide unrelated files")

# A first upload interrupted before publication leaves an ownership marker, not a dead end.
bootstrap = work + "/bootstrap"
os.mkdir(bootstrap)
owner = json.load(open(folder + "/cameo-pi-backup.json"))
assert owner["updated"] == 0
json.dump(owner, open(bootstrap + "/cameo-pi-backup.json", "w"))
os.mkdir(bootstrap + "/commits")
call("PATCH", "/api/v1/backup", {"folder":bootstrap})
os.chmod(bootstrap + "/commits", 0o500)
try:
    assert not run()["last_attempt"]["ok"]
    assert call("POST", "/api/v1/backup/restore", {}, expect=409)["error"] == "no-backup"
finally:
    os.chmod(bootstrap + "/commits", 0o700)
assert run()["last_attempt"]["ok"]
assert call("POST", "/api/v1/backup/restore", {})["jobs"] == 1
call("DELETE", "/api/v1/backup/restore")
call("PATCH", "/api/v1/backup", {"folder":folder})
ok("a failed first publication can resume from its ownership marker")

# Version-one plaintext remains readable and migrates without replacing local work.
legacy = work + "/legacy-plain"
os.mkdir(legacy)
shutil.copytree(folder + "/library", legacy + "/library")
old = json.load(open(folder + "/manifest.json"))
for entry in old["files"].values(): entry.pop("object", None)
old["removed"] = []
for index in range(2):
    stored = f"removed/day{index}/library/Old/job.json"
    os.makedirs(os.path.dirname(legacy + "/" + stored))
    shutil.copyfile(legacy + "/library/Square/job.json", legacy + "/" + stored)
    old["removed"].append({"path":"Old/job.json", "stored":stored, "at":int(time.time()) - index})
json.dump(old, open(legacy + "/manifest.json", "w"))
m = json.load(open(folder + "/commits/" + checkpoint()))
m.update(schema_version=1)
for field in ["generation", "manifest", "key"]: m.pop(field, None)
json.dump(m, open(legacy + "/cameo-pi-backup.json", "w"))
call("PATCH", "/api/v1/backup", {"folder":legacy})
assert call("POST", "/api/v1/backup/restore", {})["jobs"] == 1
call("DELETE", "/api/v1/backup/restore")
assert run()["last_attempt"]["ok"]
assert os.listdir(legacy + "/commits")
converted = json.load(open(legacy + "/manifest.json"))
assert len(converted["removed"]) == 2
assert len({r["stored"] for r in converted["removed"]}) == 2
call("PATCH", "/api/v1/backup", {"folder":folder})
ok("a version-one plaintext backup restores and migrates to immutable generations")

# Another Pi's folder is refused.
marker = json.load(open(f"{folder}/cameo-pi-backup.json"))
json.dump(dict(marker, pi_id="someone-else", pi_name="Other Pi"), open(f"{folder}/cameo-pi-backup.json", "w"))
e = call("POST", "/api/v1/backup/test", expect=409)
assert e["error"] == "other-pi", e
json.dump(marker, open(f"{folder}/cameo-pi-backup.json", "w"))
ok("another Pi's backup folder is refused")
os.makedirs(f"{work}/busy"); open(f"{work}/busy/notes.txt", "w").write("mine")
call("PATCH", "/api/v1/backup", {"folder": f"{work}/busy"})
e = call("POST", "/api/v1/backup/test", expect=409)
assert e["error"] == "folder-not-empty", e
ok("a folder with other things in it is refused")

# Encryption switched on for the existing plain backup: converted in place.
call("PATCH", "/api/v1/backup", {"folder": folder})
plain = json.load(open(f"{folder}/manifest.json"))
call("POST", "/api/v1/backup/encryption", {"password": "short"}, expect=400)
call("POST", "/api/v1/backup/encryption", {"password": "correct horse battery"})
assert call("POST", "/api/v1/backup/test")["ok"]
v = run()
assert v["last_attempt"]["ok"] and "now encrypted" in v["last_attempt"]["message"], v["last_attempt"]
def names(root):
    out = []
    for r, dirs, files in os.walk(root):
        out += [os.path.relpath(os.path.join(r, f), root) for f in files]
    return out
n = names(folder)
commit = json.load(open(f"{folder}/commits/{sorted(os.listdir(folder + '/commits'))[-1]}"))
assert commit["key"] in n and commit["manifest"] in n, n
assert not any(x.startswith(("library/", "removed/")) or x == "manifest.json" or "Square" in x for x in n), n
blob = open(os.path.join(folder, next(x for x in n if x.startswith("objects/"))), "rb").read()
assert blob.startswith(b"age-encryption.org/v1"), blob[:30]
objects = sorted(x for x in n if x.startswith("objects/"))
assert len(objects) == len(plain["files"]) + len(plain["removed"]), (objects, plain)  # removed files too
ok("turning encryption on encrypts the existing backup in place, removed files included")
e = call("POST", "/api/v1/backup/restore", {}, expect=409)
assert e["error"] == "password", e
e = call("POST", "/api/v1/backup/restore", {"password": "wrong password!"}, expect=409)
assert e["error"] == "password", e
summary = call("POST", "/api/v1/backup/restore", {"password": "correct horse battery"})
assert summary["jobs"] == 1, summary
call("DELETE", "/api/v1/backup/restore")
ok("an encrypted backup restores only with its password")
v = run()
assert "nothing had changed" in v["last_attempt"]["message"], v["last_attempt"]
ok("the next encrypted backup sends nothing when nothing changed")

# An existing encrypted v1 backup can be checked with its password and continued,
# without using Replace (which could overwrite newer local designs).
legacy = work + "/legacy-encrypted"
os.mkdir(legacy)
latest = json.load(open(folder + "/commits/" + checkpoint()))
shutil.copyfile(folder + "/" + latest["manifest"], legacy + "/manifest.age")
shutil.copyfile(folder + "/" + latest["key"], legacy + "/key.age")
shutil.copytree(folder + "/objects", legacy + "/objects")
# Keep generation keys referenced by removed files too.
shutil.copytree(folder + "/generations", legacy + "/generations")
m = dict(latest, schema_version=1)
for field in ["generation", "manifest", "key"]: m.pop(field, None)
json.dump(m, open(legacy + "/cameo-pi-backup.json", "w"))
call("PATCH", "/api/v1/backup", {"folder":legacy})
assert not run()["last_attempt"]["ok"]
assert call("POST", "/api/v1/backup/restore", {"password":"correct horse battery"})["jobs"] == 1
call("DELETE", "/api/v1/backup/restore")
assert run()["last_attempt"]["ok"]
call("PATCH", "/api/v1/backup", {"folder":folder})
call("POST", "/api/v1/backup/restore", {"password":"correct horse battery"})
call("DELETE", "/api/v1/backup/restore")
ok("an encrypted legacy backup migrates after password checking, without replacing the library")

# A replacement Pi discovers encryption from the backup, preserves its key on Replace,
# and continues encrypted scheduled/manual backups without silently switching to plaintext.
original = base, op, jar
replacement = subprocess.Popen(["target/debug/cameo-pi", "--practice", "--data", f"{work}/replacement", "--listen", "127.0.0.1:18553"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    time.sleep(2)
    base = "https://localhost:18553"
    jar = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx), urllib.request.HTTPCookieProcessor(jar))
    op.open(base + "/").read()
    call("PATCH", "/api/v1/backup", {"host":"127.0.0.1", "port":22320, "user":user, "folder":folder, "every":"manual"})
    key = call("POST", "/api/v1/backup/key")["public_key"]
    with open(f"{work}/authorized_keys", "a") as f: f.write(key + "\n")
    identity = call("POST", "/api/v1/backup/test")["identity_unknown"]
    call("POST", "/api/v1/backup/identity", identity)
    assert not call("GET", "/api/v1/backup")["encrypted"]
    assert call("POST", "/api/v1/backup/restore", {}, expect=409)["error"] == "password"
    assert call("POST", "/api/v1/backup/restore", {"password":"correct horse battery"})["jobs"] == 1
    assert call("POST", "/api/v1/backup/restore/apply")["restored"]
    assert call("GET", "/api/v1/backup")["encrypted"]
    assert os.path.isdir(f"{work}/replacement/library/Empty/Nested")
    assert run()["last_attempt"]["ok"]
    newest = json.load(open(f"{folder}/commits/{checkpoint()}"))
    assert newest["encrypted"] and newest["recipient"] == commit["recipient"]
finally:
    base, op, jar = original
    replacement.terminate(); replacement.wait()
# The original Pi safely learns the newly committed manifest without replacing its own library.
call("POST", "/api/v1/backup/restore", {"password":"correct horse battery"})
call("DELETE", "/api/v1/backup/restore")
ok("a fresh Pi restores empty folders and keeps encryption for its next backup")

# Interrupt password rotation after the new key is uploaded but before new objects complete.
old_commit = checkpoint()
call("POST", "/api/v1/backup/encryption", {"password":"another long password"})
os.chmod(folder + "/objects", 0o500)
# Existing object buckets also need to reject writes.
for bucket in os.listdir(folder + "/objects"):
    os.chmod(folder + "/objects/" + bucket, 0o500)
try:
    assert not run()["last_attempt"]["ok"]
    assert checkpoint() == old_commit
    assert call("POST", "/api/v1/backup/restore", {"password":"correct horse battery"})["jobs"] == 1
    call("DELETE", "/api/v1/backup/restore")
    call("POST", "/api/v1/backup/restore", {"password":"another long password"}, expect=409)
finally:
    os.chmod(folder + "/objects", 0o700)
    for bucket in os.listdir(folder + "/objects"):
        os.chmod(folder + "/objects/" + bucket, 0o700)
ok("failed password rotation preserves the original key and committed ciphertext")

# A new password: everything active is encrypted again to the new key.
call("POST", "/api/v1/backup/encryption", {"password": "another long password"})
v = run()
assert v["last_attempt"]["ok"] and "now encrypted" in v["last_attempt"]["message"], v["last_attempt"]
assert len(set(objects) & set(x for x in names(folder) if x.startswith("objects/"))) == len(plain["removed"]), "only retained removed files may keep their old key"
call("POST", "/api/v1/backup/restore", {"password": "correct horse battery"}, expect=409)
assert call("POST", "/api/v1/backup/restore", {"password": "another long password"})["jobs"] == 1
call("DELETE", "/api/v1/backup/restore")
ok("a new password re-encrypts the backup, and only the new password opens it")

# Encryption off: plain folders again.
call("POST", "/api/v1/backup/encryption", {"password": None})
v = run()
assert v["last_attempt"]["ok"] and "plain folders" in v["last_attempt"]["message"], v["last_attempt"]
n = names(folder)
assert "manifest.json" in n and "library/Square/job.json" in n, n
assert not any(x.startswith("objects/") or x in ("key.age", "manifest.age") for x in n), n
ok("turning encryption off makes the backup plain folders again")

# A changed identity is refused until it's forgotten.
os.environ["WORK"] = work
open(f"{work}/rotate", "w").write("1")
PY
# Rotate the server's host key and check the refusal.
kill "$sshd_pid"; wait "$sshd_pid" 2>/dev/null || true
rm -f "$work/host_key" "$work/host_key.pub"
ssh-keygen -q -t ed25519 -N "" -f "$work/host_key"
start_sshd
WORK="$work" PORT=$port python3 -I - <<'PY'
import json, os, ssl, urllib.request, http.cookiejar
base = f"https://localhost:{os.environ['PORT']}"
ctx = ssl._create_unverified_context(); jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx), urllib.request.HTTPCookieProcessor(jar))
op.open(base + "/").read()
token = next(c.value for c in jar if c.name == "cpi_csrf")
req = urllib.request.Request(base + "/api/v1/backup/test", data=b"{}", method="POST",
    headers={"Content-Type": "application/json", "Origin": base, "x-csrf-token": token})
try:
    op.open(req); raise SystemExit("a changed identity was accepted")
except urllib.error.HTTPError as e:
    body = json.loads(e.read())
    assert e.code == 409 and body["error"] == "identity-changed", body
print("ok   a changed identity is refused")
print("all backup checks passed")
PY
