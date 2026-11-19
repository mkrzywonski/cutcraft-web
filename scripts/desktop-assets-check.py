#!/usr/bin/env python3
"""Exercise the bundle build gate without building another release server."""
import os
import gzip
import hashlib
import runpy
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="cutcraft-assets-check-") as tmp:
    tmp = Path(tmp)
    binary = tmp / "build-check"
    subprocess.run(["rustc", "--edition=2024", str(root / "apps/server/build.rs"), "-o", str(binary)], check=True)
    pkg = tmp / "web/desktop/pkg"
    pkg.mkdir(parents=True)

    def check(profile, success, bundled):
        env = dict(os.environ, CARGO_MANIFEST_DIR=str(tmp), CARGO_PKG_VERSION="1.2.3", PROFILE=profile)
        result = subprocess.run([str(binary)], env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == success, result.stderr
        assert ("cargo:rustc-cfg=desktop_assets" in result.stdout) == bundled, result.stdout
        if not success:
            assert "scripts/build-desktop.sh" in result.stderr

    check("debug", True, False)
    check("release", False, False)
    (pkg / "cutcraft_ui.js").write_text("const wasm = new URL('cutcraft_ui_bg.wasm', import.meta.url);\n")
    desktop = pkg.parent
    (desktop / "desktop.js").write_text("import init from '/desktop/pkg/cutcraft_ui.js';\n")
    (desktop / "desktop.css").write_text("body { color: black; }\n")
    (desktop / "desktop.html").write_text('<link href="/desktop.css"><script src="/desktop.js"></script>')
    package = runpy.run_path(str(root / "scripts/package-desktop.py"))["package"]
    (pkg / "cutcraft_ui_bg.wasm").write_bytes(b"\0asm\1\0\0\0")
    (pkg / "bundle-version.txt").write_text("1.2.2\n")
    check("debug", True, False)
    check("release", False, False)
    (pkg / "bundle-version.txt").write_text("1.2.3\n")
    check("debug", True, False)
    check("release", False, False)
    records = package(desktop)
    check("debug", True, True)
    check("release", True, True)
    first = {name: (pkg / name).read_bytes() for name in (pkg / "asset-files.txt").read_text().splitlines()}
    assert package(desktop) == records
    assert first == {name: (pkg / name).read_bytes() for name in first}
    for name, _ in records:
        content = (pkg / name).read_bytes()
        assert hashlib.sha256(content).hexdigest() == name.split('.')[-2]
        assert gzip.decompress((pkg / (name + ".gz")).read_bytes()) == content
    missing = pkg / (records[0][0] + ".gz")
    missing.unlink()
    check("debug", True, False)
    check("release", False, False)
    package(desktop)
    check("release", True, True)
    (pkg / "cutcraft_ui_bg.wasm").write_bytes(b"changed authored wasm")
    changed = package(desktop)
    assert all(changed[i][0] != records[i][0] for i in range(3)), "Wasm, glue and runtime hashes follow their dependencies"
    assert changed[3] == records[3], "Unchanged CSS retains its cache key"
    assert "/desktop/assets/" + changed[2][0] in (pkg / "index.html").read_text()
print("PASS: missing/stale/incomplete bundles refuse releases; hashed dependencies and gzip output are deterministic")
