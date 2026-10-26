# Milestone 0 report: the thin slice (spec §19)

Status: **first results, 2026-10-26.** These are from the developer's desktop: an Intel i9-9900K,
NixOS, and headless Chromium 155. Results on the Pi 5, Safari, Firefox and the iPad are still to
come (§5).

Branch `claude/m0`:
- **The contract** is in `crates/protocol` and is kept.
- **The spike** is in `spike/m0`. It builds against a local CutCraft checkout and is thrown away
  in milestone 1 (spec §5.2).
- **The speed script** is `scripts/m0-speed.py`.

Results that name the developer's archive designs stay in the gitignored `local/`. Only counts and
percentiles are here.

## 1. What was built

- **`cutcraft-web-protocol`:** the commands, refusal codes, key lookups, the session snapshot,
  geometry, pictures and the worker requests, with a version. It builds for wasm, and 4 tests pass.
- **A server** (`spike/m0/server`):
  - HTTPS with its own certificate, a password sign-in (argon2), and a cookie session.
  - A changes-only header (`x-cutcraft`), the spec's CSP, and compression.
  - Hashed assets cached as immutable.
  - Live snapshots over a WebSocket.
- **One preparation worker per session:** a child process running CutCraft's controller, with an
  address-space cap (1.5 GB by default). It speaks JSON lines with the server.
- **Its authority, for now:** an allowlist of registry commands. Paths are never taken from the
  browser: uploads are stored under names the server chooses.
- **Idempotency:** a key returns its first answer; a reused key with a different body is refused;
  stale revisions and stale incarnations are refused; a lost reply is found again by its key.
- **The desktop layout's slice in the browser** (`spike/m0/ui`), with eframe and wgpu (WebGPU,
  falling back to WebGL2):
  - sign-in, and opening a design by the file picker or a drop;
  - the overlay: a server picture of the artwork, asked for by revision, region and zoom step,
    with the cut lines drawn locally every frame, and the old picture kept while a new one loads;
  - the Cut preview: the mat drawn locally, the design dragged locally, then one `sheet.place` on
    the drop, held on screen until the new plan arrives;
  - the material, and a lost-reply test.

## 2. What was checked

| Check | Result |
|---|---|
| Four cargo checks, main workspace and spike, plus wasm clippy for both browser-side crates | pass |
| Unit tests: protocol (4), view arithmetic (5) | pass |
| `spike/m0/protocol-check.sh`: 22 protocol rules against a real server and worker | 22/22 |
| `spike/m0/browser-check.sh`: 20 checks in headless Chromium (below) | 20/20 |

**The browser check covers:**
- **Serving:** the wasm is `application/wasm`, compressed with Brotli, and cached as immutable;
  the CSP is right; there are no CSP violations and no page errors.
- **Using it:** sign-in; an upload with Make cut-ready in the worker; a real mouse drag on the mat.
- **The drop:** it sends the pointer's movement in mm, and the server moves the design by exactly
  that.
- **Changes:** the material (by key); a lost reply found by key and applied once.
- **The overlay:** zooming asks for a new, smaller-region picture.

## 3. Measurements

**Speed on the developer's archive:** `cutcraft-cli plan` (open, Make cut-ready, plan) on the 142
designs of the local acceptance set, one process each:

| | median | 90% | 99% | slowest |
|---|---|---|---|---|
| Wall time | 0.40 s | 1.24 s | 3.88 s | 4.08 s |
| Peak memory | 59 MB | 97 MB | 169 MB | 177 MB |

None failed, and none took over 10 s. A Pi 5 is roughly half as fast single-threaded (published
benchmarks), which suggests about 0.8 s median and 8 s for the slowest. That's inside the 10 s
target (spec §19), **to be confirmed on the Pi 5.**

**The slice, on the desktop over loopback:**

| What | Measured | Target (§19) |
|---|---|---|
| First load to running, wasm compiled | ≈ 0.42 s | ≤ 10 s on the home network |
| Reload with the wasm cached | ≈ 0.55 s | ≤ 2 s |
| A command (`sheet.set`) round trip | 4–5 ms | — |
| Drop to the new plan on the page (command, snapshot, geometry) | 55 ms (3 runs) | ≤ 150 ms |
| A new artwork picture (≈ 1300 × 1000 px PNG) | 50–145 ms | ≤ 300 ms |
| The geometry for the fixture | 5.4 KB, 2 ms | — |
| The wasm (egui, wgpu, PNG decoding; no engine) | 7.4 MB raw, 2.6 MB as sent (Brotli) | — |

**Workers:**
- **Memory:** 8.5 MB idle, 24 MB with the fixture open.
- **Start-up:** too small to see against a request's own time. Each curl request was about 40 ms,
  which is mostly a fresh TLS handshake per request; a browser reuses its connection.

## 4. Findings

1. **The controller already resists bad input.** A truncated SVG, a corrupt PDF and a
   24000 × 24000 PNG were each refused with a plain message. Pictures over 40 megapixels are
   refused before decoding. Under a lowered memory cap (30 and 60 MB), opening the archive's
   heaviest design failed with "out of memory", and the worker survived.
2. **A crashed worker is contained.**
   - **What happened:** killing the worker failed one request with "The design stopped working.
     Open it again."
   - **After it:** the next request got a fresh worker with a new incarnation, so changes made for
     the old load are refused, never applied to the wrong state.
   - **Still to do:** restoring the session from its draft is milestone 2.
3. **`Origin::User` skips CutCraft's file roots,** so a command taking a path would read anywhere
   the service can. The slice uses an allowlist, and the server resolves its own names. Milestone 2
   replaces this with the authority policy (spec §6).
4. **The worker's snapshot plans the sheet more than once:**
   - **Where:** the snapshot, `prepare.report` and the geometry each plan, and the art bounds walk
     the assembly again.
   - **What it costs:** most of the 55 ms drop-to-plan is this.
   - **The fix:** cache the plan per project revision.
5. **wasm-bindgen must match exactly:** the pinned nixpkgs has 0.2.121, and CutCraft's lock has
   0.2.129. The spike builds the 0.2.129 CLI into `local/tools`. Milestone 1 needs it in the Nix
   shell, pinned (docs/development.md §3.1).
6. **Headless Chromium's device emulation breaks eframe's canvas sizing.** With
   `Emulation.setDeviceMetricsOverride`, egui saw half the window, and pointer positions didn't
   match the drawing. Sizing the window with Chromium's own flags (`--window-size`,
   `--force-device-scale-factor`) works. Browser checks must not use device emulation for the
   desktop layout.
7. **Two things the browser check had to learn:**
   - **egui starts a drag only after a few points of movement,** so drags are measured from where
     the button went down.
   - **The layout moves as status messages arrive,** so the check waits for a steady view before
     aiming.
8. **Headless Chromium redraws lazily.** The page's own timing shows the plan 55 ms after a drop,
   but the check saw it in the page's state only about 1.2 s later. Whether a real browser shows
   the move as quickly is one of the checks by eye below.

## 5. Not done yet

| Item (spec §19) | Status |
|---|---|
| Safari, Firefox and an iPad, under the real headers | **Not run.** They need the developer's Mac and iPad. Firefox's automation isn't set up here. |
| Timing on the Pi 5 | **Not run:** the board has just arrived. Next: build `cutcraft-cli` for aarch64 and rerun `scripts/m0-speed.py` there. |
| Stop acknowledged under preparation load (§18) | **Not run.** The slice has no cutter service. Next: cameo-pi's cutter worker with the Simulated Cameo, timing Stop while two workers prepare. |
| Is a dropped move on screen quickly in a real browser? | **Not checked** (finding 8). |
| Which renderer wgpu chose (WebGPU or WebGL2) | **Not recorded:** the browser had `navigator.gpu`, but the slice doesn't report which backend started. |
| Reconnecting the WebSocket after a dropped connection | Coded, not tested. |
| Worker processes against threads | Processes worked as intended in every test here. Threads weren't tried, since the decision was already processes. |

**The provisional limits (spec §7, §18), from what was measured:**
- **Memory:** the 1.5 GB worker cap is about 8 times the archive's peak. Keep it until the Pi 5 run.
- **Pictures:** CutCraft's own 40-megapixel limit already applies, under the spec's 100 megapixels.
- **Upload size and concurrency:** not exercised yet.

## 6. How to run it

```sh
# in ~/cutcraft-web-worktrees/claude-m0, with ~/cutcraft beside it
nix develop -c cargo install --locked wasm-bindgen-cli --version 0.2.129 --root local/tools   # once
nix develop -c spike/m0/protocol-check.sh
nix develop -c nix-shell -p nodejs chromium binaryen brotli --run spike/m0/browser-check.sh
CUTCRAFT_CORPUS_DIR=… nix-shell --run "python3 scripts/m0-speed.py CLI MANIFEST local/out.jsonl"   # in ~/cutcraft's shell
```

To try it by hand: build the shell with `spike/m0/build-web.sh`, then start the server:
`CUTCRAFT_WEB_PASSWORD=… spike/m0/target/release/cutcraft-web-m0 serve --data DIR --web spike/m0/web-dist`.
Open `https://localhost:8443`.
