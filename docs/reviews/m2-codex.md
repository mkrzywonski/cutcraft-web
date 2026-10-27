# M2 review and fixes

Reviewed the M2 implementation through the household-list follow-up (0.6.0).
The fixes are in 0.6.1. This is the author's implementation evidence; independent
review remains a separate acceptance gate.

## Findings addressed

- Library Open used a random staging filename as its request identity. An identical
  browser retry was refused as a changed key. Requests now retain the complete
  browser body, including the unsaved-work choice, and replay before resolving files.
- Save checked current state before replay, and Snapshot, Export and Saved could
  interleave with edits. Session mutations now serialize through Save; completion
  also checks incarnation and revision. Retrying an older successful save preserves
  newer unsaved edits.
- The library base check and publication were separate operations. The comparison
  now shares the revision publication lock; competing saves publish only one revision.
- Save keys occupied a server-wide namespace. Design saves now scope their library
  upload key to the principal.
- A crash after library publication but before session acknowledgement could publish
  a second copy on retry, or report not-applied. A private receipt is synced and
  published with the revision, and used for replay and lookup after recovery. It is
  not included in public revision metadata or downloadable files.
- Cancelling a handler could leave its worker response in the process pipe. Worker
  exchanges finish independently of the HTTP waiter; Open and Save transactions also
  finish after disconnection.
- Closing a project deleted its checkpoint, including accepted key records. Draft
  schema 2 represents an empty project and its records; schema 1 remains supported,
  and unknown schemas are set aside.
- First-admin creation could move the household directory while its worker retained
  dirty edits. It now checkpoints and detaches before adoption, and retires the old
  principal. Failed account creation can resume open-mode work.
- Design SSE streams lacked the existing service stream's access recheck. They now
  terminate on revocation or a change of principal.

## Regression coverage

Authored fixtures only: retry after later edits; changed request bodies; competing
saves from two people; publication before lost session acknowledgement; truthful
lookup; closed-session recovery; stale save completion; household adoption with
uncheckpointed edits; revoked streams; cancellation of a worker waiter; worker wire
round trips. The process-worker script separately exercises the real child process,
protocol refusal, authority policy, images and crash recovery.

## Remaining milestone scope

The original M2 handover explicitly leaves origin-labelled Undo, project units,
removed-account retention, and full preference/backup integration unfinished.
Household catalogs are implemented; Adjust and backups still need integration with
them. M3 must add immutable cut copies, durable cutter authority independent of
sessions/history, bounded worker scheduling/unloading, Stop-under-load evidence and
update drain. Starting M3 does not certify these outstanding acceptance gates.

## Validation

Passed: workspace formatting, Clippy with warnings denied, workspace tests and build
(all locked); protocol Clippy for wasm32; cargo-deny (existing duplicate-version
warnings); the real process-worker design checks, browser checks, and scratch SSH
backup/restore checks. These are local simulator/software checks, not Pi latency,
memory-pressure or hardware acceptance evidence.
