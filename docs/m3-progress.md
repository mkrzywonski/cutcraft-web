# M3 implementation status

## First slice: update drain (0.7.0)

The cutter service owns a shared `UpdateGate`. An updater calls `begin()` before
waiting for idle cutters. Every HTTP and MCP Start already uses `Service::start`:
known requests replay their original answer first, new requests are refused while
waiting, and final admission is serialized with beginning drain. Plan preparation
and disk access stay outside that admission lock. Stop never acquires it.

Shared status reports `update.waiting` and `update.ready`. Readiness requires every
cutter's activity to be idle. A paused execution, an active execution and stop
confirmation all prevent readiness. A queued turn may remain while ready. The
updater must check readiness again immediately before graceful restart.

This provides the admission primitive and status, not an updater or restart button.
The running executable continues serving its embedded assets. Deployment/restart
wiring must use this gate; nothing here initiates a restart or hardware motion.

Tests cover admission racing drain, accepted Start replay, refusing a new Start,
Stop during drain, waiting for settling, and a queued turn remaining without an
execution. They use the virtual-clock simulator. They do not establish the 200 ms
Stop acknowledgement target under preparation or memory pressure.

## Second slice: immutable cut copies (0.8.0)

The shared controller registry's `cut.copy` command validates and exports only the
current sheet's project and plan. The preparation worker uses that command; generic
worker commands cannot return its raw project/source payload to a browser.

HTTP `POST /api/v1/design/command` with `id: "cut.copy"`, empty params and the usual
key/incarnation/revision, and MCP `design_cut_copy`, call the same action. MCP
`design_status` supplies the current session identity. Preparation does not queue
or move anything: existing queue and Start calls explicitly name the returned
`value.copy.id` at revision 1. Layout integration follows in M4/M5.

Copies and their original request receipts are one atomic schema-1 record in
private cutter-service storage, independent of drafts and the editable library.
Retries return the first result before checking current session state. Reused keys
with a changed body are refused, including after losing the worker checkpoint.
Copy IDs are scoped to the principal and request key, never filesystem paths.

Queue, loading metadata, preview, geometry and Start resolve the same copy. Library
saves cannot advance it. Original project/artwork downloads are refused; only the
chosen sheet's cut preview is shared. The simulator regression compares the actual
execution record's digest with the original copy after changing force, saving a
new library revision and closing the source design. Other tests cover store restart,
HTTP/MCP transport parity, absent/stale designs, changed keys and path rejection.
The process-worker script exercises creation and loading of copies too.

This does not yet persist queue positions; that remains cutter-authority work.

## Third slice: durable Start admission (0.9.0)

Start stages its execution under the cutter lock, then releases that lock while
syncing the execution record. The device worker cannot take a staged job. Only a
successful sync releases it and permits a successful Start acknowledgement. Lookup
serializes with acceptance, so it cannot announce success early either.

Stop does not take the acceptance or journal lock. Its control flag survives
persistence completion. A disconnect or observed mat change during persistence
cancels release; failed storage leaves no job available to the driver. Driver-side
recording is idempotent and never overwrites a terminal record. The first terminal
outcome is retained.

Execution records are owned by the service and recovered there, independently of
sessions and ordinary library history. Restart restores request answers, marks
unfinished records interrupted and never restores pending motion. Schema-1 records
are explicit; the imported unversioned form remains readable as schema 1. Damaged,
unreadable or unsupported records fail startup rather than silently dropping keys.

Regressions cover restart immediately after acknowledgement before the driver takes
the job, source-library removal, changed-body replay, failed record writes, mat
change/disconnection and Stop while persistence is deliberately blocked. The latter
checks that no drawing occurs before sync or after the intervening Stop. These are
simulator/software checks; they do not establish target-Pi load latency.

## Next slices required by the spec

- Review and extend durable cutter records for accepted requests, Stop intent and
  terminal outcomes independently of sessions and deletable history. Exercise
  restart, lost replies, Stop racing Start and interrupted executions.
- Bound heavy preparation concurrency, evict/unload idle workers safely, and add
  backpressure/cancellation. Measure Stop under malformed and heavy imports,
  memory pressure, slow readers and worker failures on the target Pi.
- Connect the deployment updater to drain and verify its final all-cutter idle
  check before restart. Do not replace served assets during draining.
- Complete the previously declared M2 carryovers in `docs/reviews/m2-codex.md`;
  starting M3 does not certify those acceptance gates.

Independent review is required before integrating these safety-related changes.
