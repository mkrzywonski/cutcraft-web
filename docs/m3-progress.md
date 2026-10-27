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

## Next slices required by the spec

- Freeze the current sheet and its plan into a validated immutable cut copy; name
  that copy throughout queue/loading/Start so later session edits cannot change it.
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
