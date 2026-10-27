#!/usr/bin/env python3
"""M3 real-process simulator checks. Never opens USB; uses isolated temporary state.

python3 scripts/m3-check.py BINARY FIXTURE_DIR [--out results.json] [--large 2000]
FIXTURE_DIR is produced by cameo-pi-core's write_fixture example. Results contain
only timings/counts, never credentials, user files or machine addresses.
"""
import argparse
import concurrent.futures
import http.client
import json
import os
from pathlib import Path
import signal
import shutil
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import uuid


def key():
    return uuid.uuid4().hex


class Client:
    https = False

    def __init__(self, port):
        self.port, self.cookie = port, ""

    def call(self, method, path, body=None, content=None, allow_error=False):
        connection = (http.client.HTTPSConnection("127.0.0.1", self.port, timeout=90, context=ssl._create_unverified_context()) if self.https else http.client.HTTPConnection("127.0.0.1", self.port, timeout=90))
        headers = {"Host": "localhost", "Origin": "http://localhost",
                   "Cookie": "cpi_csrf=t; " + self.cookie, "X-CSRF-Token": "t",
                   "X-CutCraft-Protocol": "5"}
        if body is not None:
            if isinstance(body, bytes):
                headers["Content-Type"] = content or "application/octet-stream"
            else:
                body = json.dumps(body).encode()
                headers["Content-Type"] = "application/json"
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        cookie = response.getheader("Set-Cookie")
        if cookie and cookie.startswith("cpi_session="):
            self.cookie = cookie.split(";", 1)[0]
        raw = response.read()
        status = response.status
        connection.close()
        try:
            value = json.loads(raw)
        except ValueError:
            value = {"bytes": len(raw)}
        if status >= 400 and not allow_error:
            raise AssertionError((method, path, status, value))
        return status, value

    def get(self, path):
        return self.call("GET", path)[1]

    def post(self, path, body):
        return self.call("POST", path, body)[1]

    def snapshot(self):
        return self.get("/api/v1/design/snapshot")

    def upload(self, data, name="shape.svg"):
        return self.call("POST", "/api/v1/design/upload?name=" + name, data)[1]["design"]["id"]

    def open(self, design, request_key=None):
        snapshot = self.snapshot()
        return self.call("POST", "/api/v1/design/open", {
            "design": design, "key": request_key or key(), "replace_unsaved": True,
            "incarnation": snapshot["incarnation"], "revision": snapshot["revision"]}, allow_error=True)

    def command(self, command, params):
        snapshot = self.snapshot()
        return self.post("/api/v1/design/command", {"id": command, "params": params,
                         "key": key(), "incarnation": snapshot["incarnation"],
                         "revision": snapshot["revision"]})

    def cutter(self):
        return self.get("/api/v1/state")["cutters"][0]


def slow_connection(port, small_window=False):
    sock = socket.socket()
    if small_window:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
    sock.connect(("127.0.0.1", port))
    if Client.https:
        sock = ssl._create_unverified_context().wrap_socket(sock, server_hostname="localhost")
    return sock


def wait_for(fn, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = fn()
        if value:
            return value
        time.sleep(.025)
    raise AssertionError("timed out waiting for simulator state")


def rss_tree(pid):
    total, processes = 0, []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            data = (entry / "status").read_text().splitlines()
            fields = {line.split(":", 1)[0]: line.split(":", 1)[1].strip() for line in data if ":" in line}
            processes.append((int(entry.name), int(fields["PPid"]), int(fields.get("VmRSS", "0 kB").split()[0])))
        except (OSError, KeyError, ValueError):
            continue
    children = {pid}
    for _ in range(4):
        children.update(p for p, parent, _ in processes if parent in children)
    for p, _, rss in processes:
        if p in children:
            total += rss
    return total, len(children) - 1


def idle_worker_vm(pid):
    sizes = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            status = (entry / "status").read_text().splitlines()
            fields = dict(line.split(":", 1) for line in status if ":" in line)
            if int(fields["PPid"]) == pid:
                sizes.append(int(fields["VmSize"].split()[0]) / 1024)
        except (OSError, KeyError):
            continue
    assert len(sizes) == 2
    return max(sizes)


def worker_limits(pid):
    limits = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            status = (entry / "status").read_text()
            if f"PPid:\t{pid}\n" not in status:
                continue
            line = next(line for line in (entry / "limits").read_text().splitlines() if line.startswith("Max address space"))
            limit = int(line.split()[3]) / (1024 * 1024)
            score = int((entry / "oom_score_adj").read_text())
            assert limit <= 1536 and score == 500, (limit, score)
            limits.append(limit)
        except FileNotFoundError:
            continue
    assert len(limits) == 2, limits
    return limits


def start(binary, folder, limit=None):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = os.environ.copy()
    if limit:
        env["CUTCRAFT_WEB_WORKER_MB"] = str(limit)
    log = open(folder / "server.log", "wb")
    installed = folder / "installed-server"
    shutil.copy2(binary, installed)
    process = subprocess.Popen([str(installed), "--practice", *([] if Client.https else ["--insecure-http"]), "--data", str(folder / "data"),
                                "--listen", f"127.0.0.1:{port}"], stdout=log, stderr=log, env=env)
    client = Client(port)
    def ready():
        if process.poll() is not None:
            raise AssertionError("server failed: " + (folder / "server.log").read_text()[-1500:])
        try:
            return client.get("/api/v1/state")
        except OSError:
            return False
    wait_for(ready)
    # Model an atomic installation while the old server is still running. New workers
    # must use that server's inode, never this deliberately incompatible replacement.
    replacement = folder / "replacement"
    replacement.write_text("#!/bin/sh\nexit 99\n")
    replacement.chmod(0o700)
    replacement.replace(installed)
    return process, client, log


def library_fixture(client, fixture):
    boundary = "m3-authored-fixture"
    fields = [("folder", b""), ("name", b"Authored square"), ("planner", (fixture / "planner.txt").read_bytes().strip()),
              ("key", key().encode()), ("project", (fixture / "square.cutcraft").read_bytes()),
              ("plan", (fixture / "square-plan.json").read_bytes())]
    body = b"".join(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"; filename="{name}"\r\n\r\n'.encode() + value + b"\r\n" for name, value in fields)
    body += f"--{boundary}--\r\n".encode()
    return client.call("POST", "/api/v1/jobs", body, f"multipart/form-data; boundary={boundary}")[1]["id"]


def ready_cut(client, job):
    status, value = client.call("POST", "/api/v1/cutters/practice/practice", {"load": False}, allow_error=True)
    assert status == 200 or value.get("error") == "no-mat", (status, value)
    wait_for(lambda: client.cutter()["view"]["mat"]["state"] == "none")
    client.post("/api/v1/cutters/practice/practice", {"load": True})
    wait_for(lambda: client.cutter()["view"]["mat"]["state"] == "fresh" and client.cutter()["view"]["mat_loaded"])
    turn = client.post("/api/v1/cutters/practice/queue", {"job_id": job, "revision": 1, "key": key()})["turn"]
    state = client.get("/api/v1/state")
    cutter = state["cutters"][0]["view"]
    body = {"boot": state["boot"], "turn": turn,
                "job_id": job, "revision": 1, "kind": "main", "key": key(),
                "generation": cutter["mat"]["generation"], "action_epoch": cutter["action_epoch"]}
    return body


def begin_cut(client, job):
    body = ready_cut(client, job)
    client.post("/api/v1/cutters/practice/start", body)
    return body


def stop_cut(client, wait=True):
    started = time.perf_counter()
    client.post("/api/v1/cutters/practice/stop", {})
    elapsed = (time.perf_counter() - started) * 1000
    if wait:
        wait_for(lambda: client.cutter()["view"]["activity"] == "idle")
    return round(elapsed, 3)


def svg(count):
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="180mm" height="240mm" viewBox="0 0 180 240">' +
            ''.join(f'<circle cx="{2 + (i % 100) * 1.7:.2f}" cy="{2 + (i // 100) * 1.7:.2f}" r="0.6"/>' for i in range(count)) + '</svg>').encode()


def users(admin):
    admin.post("/api/v1/users/first", {"name": "load-a", "password": "Authored-test-password-A"})
    admin.post("/api/v1/users", {"name": "load-b", "password": "Authored-test-password-B"})
    other = Client(admin.port)
    other.post("/api/v1/login", {"name": "load-b", "password": "Authored-test-password-B"})
    other.post("/api/v1/me/password", {"old": "Authored-test-password-B", "new": "Authored-test-password-C"})
    return other


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("fixtures", type=Path)
    parser.add_argument("--https", action="store_true", help="use isolated self-signed HTTPS")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--require-protection", action="store_true")
    parser.add_argument("--large", type=int, default=2000)
    args = parser.parse_args()
    Client.https = args.https
    result = {"simulator_only": True, "https": args.https, "large_shapes_per_session": args.large}
    with tempfile.TemporaryDirectory(prefix="cutcraft-m3-") as temporary:
        folder = Path(temporary)
        process, a, log = start(os.path.abspath(args.binary), folder)
        running = threading.Event()
        running.set()
        samples = []
        def sample():
            while running.is_set():
                samples.append(rss_tree(process.pid))
                time.sleep(.025)
        monitor = threading.Thread(target=sample, daemon=True)
        monitor.start()
        slow = None
        slow_preview = None
        slow_upload = None
        try:
            result["authority_oom_score_adj"] = int(Path(f"/proc/{process.pid}/oom_score_adj").read_text())
            if args.require_protection:
                assert result["authority_oom_score_adj"] < 0
            b = users(a)
            a.snapshot()
            b.snapshot()
            baseline = idle_worker_vm(process.pid)
            fault_limit = min(1408, max(128, int((baseline + 95) // 32) * 32))
            result["fault_worker_limit_mib"] = fault_limit
            job = library_fixture(a, args.fixtures)
            ids = [client.upload(svg(args.large)) for client in [a, b]]
            # A client that stops reading the SSE connection must not stall cutter controls.
            slow = slow_connection(a.port)
            slow.sendall(("GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\nCookie: " + a.cookie + "\r\n\r\n").encode())
            times = []
            body = ready_cut(a, job)
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                tasks = [pool.submit(client.open, design) for client, design in zip([a, b], ids)]
                wait_for(lambda: a.snapshot().get("busy") and b.snapshot().get("busy"))
                result["worker_address_space_mib"] = worker_limits(process.pid)
                a.post("/api/v1/cutters/practice/start", body)
                result["heavy_preparations_overlapped_stop"] = all(not task.done() for task in tasks)
                times.append(stop_cut(a))
                heavy = [task.result() for task in tasks]
                result["heavy_open_statuses"] = [status for status, _ in heavy]
                assert all(status == 200 and reply.get("answer") == "applied" for status, reply in heavy), heavy
                assert result["heavy_preparations_overlapped_stop"]
            cancellation = key()
            cancelled_design = a.upload(svg(args.large * 3))
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                task = pool.submit(a.open, cancelled_design, cancellation)
                wait_for(lambda: a.snapshot().get("busy_key") == cancellation)
                started = time.perf_counter()
                answer = a.command("design.cancel", {"key": cancellation})
                assert answer["value"]["cancel_requested"], answer
                status, answer = task.result(timeout=15)
                result["cancel_ms"] = round((time.perf_counter() - started) * 1000, 3)
                assert status == 503 or answer.get("answer") == "refused", (status, answer)
            wait_for(lambda: a.call("GET", "/api/v1/design/snapshot", allow_error=True)[0] == 200)
            # Constrain the receive window and leave a geometry response unread. Also
            # leave an upload body unfinished: neither reader nor sender may gate Stop.
            slow_preview = slow_connection(a.port, small_window=True)
            slow_preview.sendall(("GET /api/v1/design/geometry HTTP/1.1\r\nHost: localhost\r\nX-CutCraft-Protocol: 5\r\nCookie: " + a.cookie + "\r\n\r\n").encode())
            slow_preview.settimeout(15)
            first_line = b""
            while not first_line.endswith(b"\r\n"):
                byte = slow_preview.recv(1)
                assert byte, "geometry connection closed before response headers"
                first_line += byte
            assert b"200" in first_line
            slow_upload = slow_connection(a.port)
            slow_upload.sendall(("POST /api/v1/design/upload?name=slow.svg HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nX-CSRF-Token: t\r\nX-CutCraft-Protocol: 5\r\nCookie: cpi_csrf=t; " + a.cookie + "\r\nContent-Length: 67108864\r\n\r\nx").encode())
            result["slow_clients"] = ["unread SSE", "unread geometry", "unfinished upload"]
            malformed = a.upload(b'<svg xmlns="http://www.w3.org/2000/svg"><path d="M ???', "broken.svg")
            body = ready_cut(a, job)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                task = pool.submit(a.open, malformed)
                a.post("/api/v1/cutters/practice/start", body)
                times.append(stop_cut(a))
                result["malformed_result"] = task.result()[1].get("answer", "transport-error")
            result["stop_ms"] = times
            # A SIGTERM while paused must wait, refuse another Start and still accept Stop.
            last_start = begin_cut(a, job)
            a.post("/api/v1/cutters/practice/pause", {})
            process.send_signal(signal.SIGTERM)
            wait_for(lambda: a.get("/api/v1/state")["update"]["waiting"])
            assert process.poll() is None, "updater exited while an execution was active"
            last_start["key"] = key()
            refused, reason = a.call("POST", "/api/v1/cutters/practice/start", last_start, allow_error=True)
            assert refused == 409 and reason["error"] == "update-waiting", (refused, reason)
            result["new_start_refused_during_drain"] = True
            result["stop_during_drain_ms"] = stop_cut(a, wait=False)
            if slow:
                slow.close()
                slow = None
            if slow_preview:
                slow_preview.close()
                slow_preview = None
            if slow_upload:
                slow_upload.close()
                slow_upload = None
            assert process.wait(timeout=20) == 0, (folder / "server.log").read_text()[-2000:]
            result["drain_completed"] = True
        finally:
            if slow_preview:
                slow_preview.close()
            if slow_upload:
                slow_upload.close()
            if slow:
                slow.close()
            running.clear()
            monitor.join()
            if process.poll() is None:
                process.kill()
                process.wait()
            log.close()
        result["peak_tree_rss_mib"] = round(max(n for n, _ in samples) / 1024, 2)
        result["max_workers"] = max(n for _, n in samples)
        assert result["max_workers"] <= 2
        assert result["peak_tree_rss_mib"] < 4096
        assert max(result["stop_ms"] + [result["stop_during_drain_ms"]]) <= 200
        # Separate failure injection: lower the same production-enforced limit, never raise it.
        fault = folder / "fault"
        fault.mkdir()
        process, client, log = start(os.path.abspath(args.binary), fault, fault_limit)
        try:
            job = library_fixture(client, args.fixtures)
            client.snapshot()
            design = client.upload(svg(max(args.large, 40000)))
            body = ready_cut(client, job)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                task = pool.submit(client.open, design)
                client.post("/api/v1/cutters/practice/start", body)
                result["stop_with_worker_memory_failure_ms"] = stop_cut(client)
                status, _ = task.result()
                result["limited_worker_status"] = status
                assert status == 503, "memory failure must be isolated and reported"
            wait_for(lambda: client.call("GET", "/api/v1/design/snapshot", allow_error=True)[0] == 200)
            result["worker_recovered"] = True
        finally:
            process.terminate()
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            log.close()
        assert result["stop_with_worker_memory_failure_ms"] <= 200
    if args.out:
        args.out.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
