#!/usr/bin/env python3
"""Exercise the bundle build gate without building another release server."""
import os
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
    (pkg / "cutcraft_ui.js").write_text("// Authored bundle-gate fixture\n")
    (pkg / "cutcraft_ui_bg.wasm").write_bytes(b"\0asm\1\0\0\0")
    (pkg / "bundle-version.txt").write_text("1.2.2\n")
    check("debug", True, False)
    check("release", False, False)
    (pkg / "bundle-version.txt").write_text("1.2.3\n")
    check("debug", True, True)
    check("release", True, True)
print("PASS: missing/stale bundles allow explicit native development builds and refuse releases")
