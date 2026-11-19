#!/usr/bin/env python3
"""Build deterministic, content-addressed desktop assets using only the Python standard library."""
import gzip
import hashlib
from pathlib import Path
import sys


def package(desktop):
    pkg = desktop / "pkg"
    records = []

    def emit(stem, extension, mime, content):
        digest = hashlib.sha256(content).hexdigest()
        name = f"{stem}.{digest}.{extension}"
        (pkg / name).write_bytes(content)
        (pkg / (name + ".gz")).write_bytes(gzip.compress(content, compresslevel=9, mtime=0))
        records.append((name, mime))
        return name

    wasm = emit("cutcraft_ui_bg", "wasm", "application/wasm", (pkg / "cutcraft_ui_bg.wasm").read_bytes())
    glue = (pkg / "cutcraft_ui.js").read_text()
    if "'cutcraft_ui_bg.wasm'" not in glue:
        raise ValueError("wasm-bindgen's WebAssembly URL has changed; update the packager")
    glue = glue.replace("'cutcraft_ui_bg.wasm'", repr(wasm))
    glue_name = emit("cutcraft_ui", "js", "text/javascript; charset=utf-8", glue.encode())
    main = (desktop / "desktop.js").read_text()
    source = "/desktop/pkg/cutcraft_ui.js"
    if source not in main:
        raise ValueError("The desktop runtime import has changed; update the packager")
    main = main.replace(source, "/desktop/assets/" + glue_name)
    main_name = emit("desktop", "js", "text/javascript; charset=utf-8", main.encode())
    css = emit("desktop", "css", "text/css; charset=utf-8", (desktop / "desktop.css").read_bytes())
    html = (desktop / "desktop.html").read_text().replace("/desktop.js", "/desktop/assets/" + main_name).replace("/desktop.css", "/desktop/assets/" + css)
    (pkg / "index.html").write_text(html)
    rust = ['pub const HTML: &str = include_str!("index.html");',
            'pub const ASSETS: &[(&str, &str, &[u8], &[u8])] = &[']
    for name, mime in records:
        rust.append(f'    ("{name}", "{mime}", include_bytes!("{name}"), include_bytes!("{name}.gz")),')
    rust.append('];\n')
    (pkg / "assets.rs").write_text("\n".join(rust))
    files = ["index.html", "assets.rs"] + [part for name, _ in records for part in (name, name + ".gz")]
    (pkg / "asset-files.txt").write_text("\n".join(files) + "\n")
    return records


if __name__ == "__main__":
    package(Path(sys.argv[1]))
