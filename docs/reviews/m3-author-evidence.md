# M3 author evidence — 2026-10-27

This is implementation/acceptance evidence, not an independent approval.
Branch: `codex/m2-m3`; final implementation version: 0.13.0.

## Software checks

Passed on the development host:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`: 438 passed, none failed/ignored.
- `cargo build --workspace --locked`
- `cargo clippy -p cutcraft-web-protocol --target wasm32-unknown-unknown --locked -- -D warnings`
- `cargo deny check`: advisories, bans, licenses and sources passed (existing duplicate-version warnings).
- `scripts/design-check.sh`: real worker processes, owned uploads, protocol/authority refusals,
  immutable copies, checkpoints, crash recovery and truthful lookup.
- `scripts/browser-check.sh`: Chromium, including update notice and clearing it.
- `scripts/backup-check.sh`: scratch SSH backup/restore, encrypted generations and failure injection.
- `cargo build -p cameo-pi --release --target aarch64-unknown-linux-musl --locked`

## Fault coverage

| Failure or race | Evidence |
|---|---|
| Mutable design/source changes after handoff | Actual simulator execution digest remains the immutable copy's digest; HTTP/MCP shared action tests |
| Start reply lost or source/history deleted | Durable request replay after restart, source independent execution records |
| Stop races a blocked Start record write | Storage deliberately blocked; Stop accepted without its lock; staged job cannot draw |
| Mat changes or disconnects during Start sync | Admission fenced again before runnable release; no drawing |
| Process restarts with outstanding Stop | Confirmation restored, no runnable; original turn fence cannot consume the next turn |
| Late Start/Stop writes or damaged records | First terminal outcome wins; damaged/unknown authority formats fail closed |
| Stale queue snapshot or consumed turn | Monotonic publication, ordered restart, execution records filter consumed turns |
| Second process uses the data directory | Exclusive authority lock refuses startup |
| Worker unload/crash/client disconnection | LRU/idle checkpoints preserve edits/keys; new incarnation advertised even for lookup-triggered reload; detached pipe exchanges drain replies |
| Preparation saturation/slow readers | Bounded requests/uploads/response bodies; slow preview unit test reaches Stop independently; SSE coalesces |
| Cancellation arrives late | Process-and-call identity fence; completed edits never rewritten as cancelled |
| Update while busy | Start gate, active/paused/confirmation idle tests and real SIGTERM simulator run; assets and matching workers retained |
| Import limits | 100-page PDF/AI guard, inherited stricter picture decoder limit, per-worker address-space limit |

## Desktop real-process acceptance

`python3 scripts/m3-check.py target/debug/cutcraft-web /tmp/m3-fixtures --large 400`
passed, with two concurrent busy sessions, cancellation, malformed input, unread
SSE/geometry and an unfinished upload, memory failure/recovery and update draining.
Stop round trips: 8.717, 6.216, 8.372 and 8.482 ms. Peak server-plus-descendant RSS:
283.28 MiB; maximum resident workers: two. The reduced fault limit was 384 MiB,
chosen above the idle executable/allocator mappings. The release Pi test uses a
larger authored design; it is separate evidence, not a desktop/Pi speed comparison.

## Pi 5 real-process acceptance

Release arm64-musl binary, 8 GiB Pi 5, two sessions with 2,000 authored circles each.
The transient test service ran as an unprivileged user. While active, systemd
reported `MemoryLow=536870912`, `OOMScoreAdjust=-500`, `OOMPolicy=continue`,
`KillMode=mixed` and `TimeoutStopUSec=infinity`. The harness independently checked
the authority's -500 score and both workers' +500 scores and 1536 MiB AS limits.

The HTTP run passed all phases. Stop round trips: **0.740, 0.951, 0.840 and 0.543 ms**
(heavy imports, malformed/slow-client phase, drain and memory failure respectively).
Peak server-plus-descendant RSS: **171.75 MiB**, at most two workers. The two opens
were still active when Stop was sent and both completed successfully. Cancellation
completed in 2004.584 ms through the bounded fallback. The separate fault phase
used 160 MiB (above the measured idle mappings): failure returned 503 and the next
request recovered. Draining refused new Start, retained Stop and exited cleanly.
No persistent service was installed or existing application data changed.

The final **HTTPS** run also passed the entire harness with 2,000 shapes per
session. Stop round trips were **3.379, 4.137, 3.401 and 3.092 ms**. Peak RSS was
**173.70 MiB**, with two workers maximum; both heavy opens applied, cancellation
completed in 2009.382 ms, the 160 MiB fault was isolated/recovered, and update drain
exited cleanly. TLS responses receive a bounded final drain as well. The equivalent
desktop HTTPS check passed with Stop below 50 ms and peak RSS 288.04 MiB. Neither
transport run measures network transit from another computer.

## Review and platform gates

Two non-author reviews are required before integration. No real USB device was
opened and no real cutter moved. HTTP round-trip timings conservatively include
local client overhead; they measure acknowledgement, never physical stopping.
These targeted runs are not a soak test, a filesystem-failure latency guarantee,
or acceptance of the future desktop/compact layouts and packages.
