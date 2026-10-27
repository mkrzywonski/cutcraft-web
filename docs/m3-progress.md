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

## Fourth slice: durable Stop intent (0.10.0)

A Stop sets the existing driver control flag immediately, then syncs its own request
receipt outside the cutter lock and the Start-record transaction. A successful Stop
acknowledgement therefore survives restart, without waiting for a blocked Start
record write. A storage failure reports an unconfirmed Stop so it can be retried;
it does not revoke the control flag already set.

On startup, unresolved Stop intents restore confirmation state, not an execution
or runnable. Reconnection continues waiting for settled status or no media. This
uses the existing Stop semantics (stop feeding the cutter and establish idle); it
does not add an unverified physical abort command. The ordinary fresh-mat rule
still applies before another Start. No source project is needed for confirmation.

Confirmed receipts remain terminal. A delayed request cannot turn one back into a
pending Stop, even if confirmation reached storage before the request did. A batch
of outstanding intents captured at startup is confirmed together after settling;
new requests are not swept into that batch. Confirmation is fenced to the original
turn ID, so it cannot consume the next queued person's turn. The original cut record retains its
first outcome, including an interrupted/uncertain cut across restart.

Tests cover blocked Start persistence with Stop, request/confirmation ordering,
recovery with the source removed, no resumed drawing, stable Start reconciliation
and quiet-state confirmation of a startup batch. Hardware Stop timing and the
200 ms acknowledgement target under Pi load remain separate acceptance gates.

## Fifth slice: persistent queue authority (0.11.0)

Queue order, the current turn and original Queue request results are atomically
checkpointed outside the cutter lock. A dedicated sequence prevents a delayed
checkpoint from replacing a newer queue. Queue, release, skip, removal, job updates,
Start admission and shutdown all checkpoint; completed executions also checkpoint
on the service's history thread, away from the device worker.

Startup restores waiting turns and original results, clears old mat/test authority,
and restores no runnable. Main-cut turns consumed by execution records are removed
even if the last queue checkpoint predates completion or interruption. Known not-run
attempts may remain waiting. A data-directory lock prevents two authorities from
writing or claiming the same state. Corrupt queue/record formats fail closed.

Tests exercise waiting-turn order and attribution, duplicate/reused keys after
restart, stale checkpoints, consumed turns and exclusive directory ownership.

## Sixth slice: bounded preparation (0.12.0)

At most two workers are resident. Other opens wait and snapshots report that state.
The least recently used idle worker is checkpointed before its slot is reused;
workers unused for an hour unload as well. Reload changes the incarnation and
recovers the saved draft and acknowledged keys. A failed checkpoint prevents normal
unloading. Detached exchanges retain their replies after a caller disconnects.

The worker wire protocol now correlates calls and replies. Targeted cancellation
can bypass an import without taking its worker lock. `design.cancel` and MCP
`preparation_cancel` share the same session-scoped action. Cancellation never
rewrites an already applied edit. Before cancellable work, the worker checkpoints;
an unresponsive cancelled process is killed after two seconds, fenced to that
specific process and call. Superseded pictures are discarded. All workers enforce
a maximum 1.5 GiB address-space limit and lower their Linux OOM priority.

Pending exchanges are bounded to 32, simultaneous upload bodies to two (120-second
timeout), and large preview/download responses to four, with the response permit
held until consumption or disconnection. SSE coalesces snapshots. These limits do
not gate cutter controls. Regressions cover LRU/idle recovery, request pressure,
call-fenced cancellation and slow preview bodies while the Stop route responds.

## Next slices required by the spec

- Independent review of the complete cutter-authority fault matrix.
- Target-Pi simulator measurements under heavy and malformed imports, memory
  failure and slow readers.
- Connect deployment shutdown to drain and verify the final all-cutter idle check.
  Keep the running executable's own assets and worker protocol until restart.

Independent review is required before integrating these safety-related changes.
The M2 carryovers in `docs/reviews/m2-codex.md` remain explicitly separate scope.
