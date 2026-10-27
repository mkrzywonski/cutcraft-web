# Draining a running server

The server executable is `cutcraft-web` (the imported Cargo package is still
`cameo-pi`). `cutcraft-web.service` is the reference service policy; packages and
installers belong to milestone 8. Keep `KillMode=mixed`, `TimeoutStopSec=infinity`,
`OOMPolicy=continue`, `OOMScoreAdjust=-500` and `MemoryLow=512M` when deploying it.
The reservation applies to the service cgroup; workers additionally enforce their
own address-space cap and increase their OOM score to +500.

Install a new binary by atomic replacement, then request a normal service restart.
Do not truncate the running executable or replace its embedded assets separately.
The old process serves its own assets, and on Linux launches workers from its
running inode even after the installation path changes. Its SIGTERM handler:

1. Closes Start admission for every cutter and publishes update waiting.
2. Keeps status, the existing assets and Stop available. Active and paused cuts,
   and cuts still confirming Stop, prevent restart. Waiting turns remain queued.
3. Once idle, closes preparation admission, cancels active preparation and
   checkpoints/unloads workers. It checks every cutter for idle again.
4. Closes listeners, allows responses/streams to finish, joins device and history
   threads, and checkpoints queues before exit. Abandoned HTTP readers have a
   six-second final drain limit, after cutters are idle and workers checkpointed.

A paused cut must be resumed or stopped before the service can restart. A normal
service stop has the same drain behaviour. A forced process kill or power loss is
recovered as interrupted; it never restores runnable motion.

# Simulator check

Use authored fixtures, temporary state and only `--practice`. The harness does not
open USB, install a service or change the machine configuration:

```sh
mkdir -p /tmp/m3-fixtures
nix-shell --run 'cargo run -q -p cameo-pi-core --features fixtures --locked --example write_fixture -- /tmp/m3-fixtures' > /tmp/m3-fixtures/planner.txt
nix-shell --run 'cargo build -p cameo-pi --locked'
python3 scripts/m3-check.py target/debug/cutcraft-web /tmp/m3-fixtures --large 400
```

The full target-Pi run uses the release binary and `--large 2000`; add `--https`
to exercise the production transport with its isolated self-signed certificate. Run it under a
transient systemd service with the reference OOM/reservation properties and pass
`--require-protection` to verify the authority's OOM score. It records conservative
HTTP round-trip Stop timings, RSS for the server and its descendants, enforced
worker limits, cancellation, slow readers/senders, malformed input, lowered-limit
worker failure/recovery, and SIGTERM draining an execution. It also atomically
replaces its own isolated executable copy with an incompatible program and proves
that the old server still launches matching workers.

The fault phase lowers the worker limit to its observed idle address space plus
roughly 64 MiB (at least 128 MiB), accommodating different executable and allocator
mappings. It deliberately provokes failure through the production limit mechanism
and never raises the 1.5 GiB cap. These simulator
checks measure acknowledgement, not physical blade stopping or hardware acceptance.
