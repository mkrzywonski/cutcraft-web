# M1–M3 conformance review — 2026-10-28

Reviewed code: **0.13.1, `b9ea0990d2d950aa3725124561a2e3ab109c26c9`** (`codex/m2-m3`), before M4.
Contract: version 2 proposal at `b3c9956`, including developer-confirmed Q1/Q2, corrected here
to retain the earlier household-catalog decision (Zulip #76). Reviewer: Codex.

**Verdict: foundation changes needed before M4 can meet the recovered contract.** Most imported
geometry/preparation behaviour survives. The main gaps are the web adapter, project persistence,
session recovery and the integration of optional test cuts with immutable copies.

This is an author conformance audit: Codex wrote portions of M2/M3. It does not replace the two
independent reviews required for safety/persistence changes. Findings distinguish verified
failures, code-inspected missing routes and later milestone acceptance. No real cutter was opened.

## Evidence and reproducibility

- Unmodified reviewed code: `nix-shell --run 'cargo test --workspace --locked'` — **440 passed**,
  none failed or ignored. This includes driver/simulator, import, planner, controller, worker,
  HTTP, shared-catalog and durable Start/Stop/queue regressions.
- `nix-shell --run 'scripts/design-check.sh'` — **passed**, real child workers with isolated
  temporary data and practice mode. Covers owned upload/open, protocol and authority refusals,
  Save/receipt recovery, cut copies, crash recovery and truthful key lookup.
- [Review probes](m3-regression-probes.patch), applied only in an isolated review checkout:
  **11 tests: 2 passed, 9 failed**. Failing tests express the missing behaviour and are not fixes.
  Two positive tests cover all 21 authored issue fixtures and force/mirror checkpoint carry-over.
- All 21 fixture initial readiness, pending decision codes, diagnostic codes/severities and
  change codes match the recorded 1.0.5 outcomes. This does not compare all geometry, fonts on
  other hosts, later answers, or browser reachability.
- No new Pi, Safari, browser-layout, physical cut, backup/migration or latency acceptance was
  performed. Earlier M3 author evidence remains historical evidence at its stated commit.
  No application implementation changed; the probe patch is review evidence, not production code.

Reproduce on a clean checkout of `b9ea099` with the pinned Nix environment:

```sh
git apply /path/to/m3-regression-probes.patch
# Point at the design-proposal checkout containing the manifest and authored examples.
export CUTCRAFT_PARITY_FIXTURES=/path/to/design-proposal
nix-shell --run 'cargo test -p cameo-pi -p cameo-pi-core --locked parity_review -- --nocapture'
```

Cargo may stop after the server's expected failures; run the core package separately to see the
changed-user failure. The server probes produce 2 passes/8 failures; the core probe produces
1 failure. The fixture-root variable contains only authored examples, never the private archive.
The [conformance matrix](m3-conformance.md) covers all 71 detailed requirement IDs and the main
web foundation requirements. “Implemented” is scoped evidence, not full UI acceptance.

## P1 F01 — Save incorrectly requires a cut-ready design

**Where:** `apps/server/src/design/worker.rs:730` (`export`),
`apps/server/src/design/mod.rs:1648` (`save_inner`), `crates/control/src/commands.rs:2632`
(`cut.copy`). Requirements: SAVE-01/02, web §11.

Save uses `WorkerRequest::Export`, which now calls `cut.copy`. That command rejects unresolved
preparation and invalid plans. A vector with a pending size answer cannot be saved; neither can
a project with placement problems that the person wants to finish later. The worker returns
“Review the design's issues before cutting” to a save request. Native `project.save` has no such
cut-readiness prerequisite. The library's uploaded-plan validation adds a second coupling to
cuttable jobs, so changing one condition in the worker is insufficient.

**Shown by:** `parity_review_unfinished_project_can_be_exported_for_save` fails. Existing Save
regressions use ready stars/squares and miss this case.

**Fix:** separate editable project persistence from validated execution exports. Library revisions
must represent unfinished projects; preview/plan metadata can be absent or invalid without making
Save fail. Keep strict validation at handoff/Start. Preserve atomic publication, saved-base
conflicts and durable save receipts. Test unresolved issues, empty imports, off-material placement,
first Save/Save As/reopen, and conflicts in addition to the ready-project cases.

## P1 F02 — The adapter hides required Issues/Info and miscounts blockers

**Where:** `apps/server/src/design/worker.rs:52` (REFUSED), `:776` (snapshot),
`crates/protocol/src/lib.rs:102` (DesignView). Requirements: OPEN-02, PREP-01–08,
PANEL-01/02, UI-07/08, web §12.

The worker refuses `prepare.report`, `design.inspection` and `project.inspect`; the replacement
snapshot contains a pending-question count, not the question messages, allowed answers, affected
objects or Info changes. It counts `pending` only: a blocked empty design has `cut_ready=false`
but **zero issues**. Some other reads such as rows and plan summaries are available through the
registry; they do not replace the absent preparation report. An M4 client cannot recover the
missing choices by drawing better panels.

**Shown by:** `parity_review_question_details_are_available_to_clients` and
`parity_review_blocking_diagnostics_count_as_issues` fail.

**Fix:** a typed, privacy-filtered preparation/view projection with question choices, blockers,
Info, affected IDs and coherent revision identity. Keep server paths/font paths/source payloads
out; do not simply remove the denylist. Count and display blockers even with no pending question.
Run F01–F21 through the public adapter and exercise answers/Undo, not just internal inspection.

## P1 F03 — Supported imports are rejected before reaching the engine

**Where:** `apps/server/src/design/mod.rs:43` (DESIGNS), `:1326` (upload),
`apps/server/src/design/worker.rs:644` (replace). Requirements: OPEN-01/08/10, SAVE-02.

The HTTP allowlist contains SVG/SVGZ/PDF/AI/EPS/DXF/PNG/JPG/JPEG only. GIF, BMP, PSD/PSB,
VectorCraft/DrawCraft and `.cutcraft` cannot enter by the owned-upload route despite reference
support. The format check is case-insensitive; casing is not the defect. Project uploads also
need `project.open` dispatch rather than treating every upload as `design.open`.

**Shown by:** a valid authored uppercase `.BMP` upload returns HTTP 400 in
`parity_review_upload_accepts_supported_bmp`. The other omitted extensions are code-inspected.

**Fix:** one supported-format contract across picker/upload/open/MCP/help; format-specific safe
open paths, owned IDs, resource limits and original-byte preservation. Test every advertised
format, unsupported-format guidance and previous-project preservation on failure.

## P1 F04 — Worker eviction destroys the session's Undo history

**Where:** `apps/server/src/design/worker.rs:34` (Draft), `:347` (restore), `:405` (checkpoint).
Requirements: PREP-01, AUTO-01, web §7.

Drafts retain the current project and request receipts but not Undo/Redo, journal provenance or
the shared desktop screen. Recreating a worker restores via `project.open`, making Undo empty.
M3 deliberately evicts idle workers, including under its two-worker cap, so this can happen
without the person closing their design session. Restoring force 23 succeeds, but Undo then says
“There is nothing to undo.”

**Shown by:** `parity_review_idle_worker_recovery_preserves_undo` fails. The existing LRU test
asserts the current material and key lookup only, so it passes despite the lost history.

**Fix:** checkpoint the bounded session state needed for Undo/Redo, provenance and shared screen,
with schema migration and matching command receipts. Preserve it across ordinary eviction and
planned restart; do not reintroduce a cut as motion while restoring its screen. Test eviction by
a third session and idle unloading, not just process-crash recovery.

## P1 F05 — Test-cut placement and reservations diverge across the handoff

**Where:** `apps/server/src/service.rs:378`, `apps/server/src/design/worker.rs:822`,
`crates/cameo-pi-core/src/cutter.rs:990`. Requirements: CUT-07, PLAN-01–03, W18.

Three code paths disagree:

- Preparation computes markers with an empty `CuttingView`; it never receives the cutter's
  occupied test areas. Its next-spot preview therefore remains unaware of completed tests.
- The service's fallback placement uses the device rectangle, rather than the planner's
  `usable_area_mm()` for the actual material. A fitting scrap can get an off-scrap candidate
  and a refusal instead of the valid preview spot. Validation still blocks the off-scrap cut.
- The controller allows a test square in a gap between shapes, but the service's main Start
  compares the whole design's bounding rectangle with test reservations. It rejects the valid
  subsequent design even though no artwork intersects that test.

**Shown by:** `parity_review_test_in_artwork_gap_does_not_block_main_cut`: two separated authored
rectangles, explicit gap spot, simulated test succeeds; main Start returns `area-taken`.
The stale-marker and usable-area discrepancies are additionally established by code inspection.
The imported planner's `a_test_cut_may_sit_in_a_gap_but_never_on_the_artwork` already passes;
the regression is in the service integration.

**Fix:** one cutter-owned, generation-fenced reservation/placement result for preview and Start;
carry current/occupied spots into the public projection. Validate actual tool paths and aids
against reserved areas and material bounds, including compensation, without weakening collision
checks. Test moved spots after multiple tests, gaps, offset scraps, no-room and stale previews.

## P1 F06 — Tuning settings cannot continue the current immutable-copy test turn

**Where:** `apps/server/src/copies.rs:56` (identity), `crates/cameo-pi-core/src/cutter.rs:961`
(turn/job match), `:1181` (job_updated). Requirements: MAT-01/02, CUT-05/07, web §23 Q2.

An immutable copy is correctly frozen. After testing it, changing force and preparing a new copy
creates a different job ID. The current turn remains attached to the first ID; starting the new
one returns `stale-revision`. Queueing it creates another turn, and ending the old turn retires the
mat. The legacy library-revision update path cannot replace one immutable copy ID with another.
Thus the core permits repeated tests only with the original frozen settings, not the normal
“test, adjust force, try again” workflow with the mat retained.

**Shown by:** `parity_review_tuned_copy_can_continue_same_test_turn` changes force only, freezes
again, and gets HTTP 409 at explicit Start. It does not suggest making the old copy mutable.

**Fix:** an explicit reviewed replacement of the idle turn's copy, retaining occupied areas only
for the same person/cutter/load and compatible physical setup. Fence old requests with an epoch;
revalidate the new geometry/settings. Incompatible setup still requires fresh loading. Test
force-only replacement, old-key replay, queued contenders, Stop and disconnect races. This is a
missing transition, not a reason to remove the stale-revision guard.

## P1 F07 — Loaded-mat reuse does not invalidate on user change

**Where:** `crates/cameo-pi-core/src/cutter.rs:926` (start_checked),
`apps/server/src/http.rs:567` and `apps/server/src/service.rs:410` (Start forwarding).
Requirement: newly confirmed web §23 Q2 / CUT-07.

Start checks boot, connection, epoch, turn/job, mat generation, settling and occupied areas, but
not whether the retained load belongs to the same person. A different authenticated actor can
name that turn and start the main cut on the still-loaded mat. The HTTP path supplies the actor
but adds no retained-load owner check. This is a gap against the newly approved invalidation
rule, not a claim that an unauthenticated caller bypassed login.

**Shown by:** `parity_review_changed_user_invalidates_retained_load`: the second user's Start is
accepted by the cutter state machine after the first user's test. Simulator only; no hardware.

**Fix:** bind retained-load authority to stable principal identity, not just a display name or
client device. Another device belonging to the same person may continue; a different person must
fresh-load. Check at final admission too, alongside the existing staged-Start fences. Test HTTP
and MCP actor changes, account deletion/recreation, open-mode adoption, reload and Stop races.
Keep this guard while implementing F06.

## P1 F08 — Library Adjust still reads the old, separate preset catalog

**Where:** `apps/server/src/design/worker.rs:323` (shared lists),
`apps/server/src/adjusting.rs:67–107` (legacy catalog), `apps/server/src/http.rs:1130`.
Requirements: MAT-05–07, web §8 and developer decision #76.

Preparation correctly shares `data/catalog` across workers. Library Adjust's `/api/v1/catalog`
reads `private/catalog.json`, the old per-sender synced catalogs. A preset saved through the
right-panel command appears in other design sessions but is absent from library Adjust.
This was explicitly a carryover in the M2 handover and remains unfinished at M3.

**Shown by:** `parity_review_library_adjust_uses_household_material_catalog` saves an authored
preset and then fails to find it in the Adjust catalog.

**Fix:** a single household catalog service for preparation, library Adjust, defaults, backup and
migration. Test edits/rename/delete through both entry paths and concurrent sessions; saved
project snapshots must remain unchanged.

## P2 F09 — Settings ownership is still native, not the web contract

**Where:** `crates/job/src/project.rs:615`, `crates/control/src/settings.rs:386–440`,
`crates/control/src/commands.rs:2389–2482`. Requirements: web §8, PANEL-03, SAVE-02.

Code inspection: `Project` has no units/snap fields. Those commands still write per-person
preferences, so two projects cannot retain different choices when exchanged or reopened.
Panel `open`/fold state is stored with shared order/locks instead of remaining browser-local.
Theme has no corresponding stored field. Origin in worker commands is always `Origin::User`,
so phone versus AI provenance is also absent. These are documented M2 carryovers, not finished
just because its core session tests pass.

**Fix:** schema-backed project units/snap with explicit defaults/migration; separate browser folds
from person-wide order/locks; carry verified caller origin through the protocol/history. Test
project A/B reopening and two devices with independent folds. This finding is structural, not a
new dynamic reproduction, and the complete theme/controls presentation belongs to M4/M5.

## P2 F10 — Owned source-update, font and previous-draft routes are missing

**Where:** `apps/server/src/design/worker.rs:82` (file-command policy),
`apps/server/src/design/mod.rs:1443–1598` (upload/library Open and Save),
`apps/server/src/people.rs:20` (font directory only). Requirements: OPEN-10, SAVE-03, web §§7/10.

Code inspection: `project.update` is refused as FileRead, with no equivalent owned-upload update
handler. Opening a new upload replaces the project instead of applying the reference update/diff/
Undo logic. Font directories exist, but no owned font upload/registration route feeds each worker;
the importer uses its process-global font database. Previous projects are written as
`drafts/previous-…json`, but there is no owned draft listing/reopen route. M4 cannot surface these
capabilities without server work. No cross-person font leak is asserted by this review.

**Fix:** owned-ID adapters for Update from Design, fonts and previous drafts, preserving the
existing controller's validation/replay and isolation. Test stale updates, ambiguous rows, failed
font extraction, revoked users, and reopening the prior draft. Keep full backup/import migration
acceptance in M7, but do not hide these ordinary workflow requirements there.

## Additional integration limits and positive findings

- **Test readiness:** `cut.copy` also gates the only new-design execution handoff on main artwork
  readiness. There is no independent test-copy export for an open design whose artwork has
  unresolved issues. CUT-02 requires pattern validation independent of main-art readiness.
  Resolve this with F01/F06 while keeping material/load/bounds validation strict.
- **Completion records:** service execution receipts survive restart, but preparation has no
  completed/interrupted-cut feedback path, and `cut.reset` is denied. The saved project's native
  cut record therefore does not track web executions. Connect records by frozen copy/source
  revision without marking a newer edited draft completed; never delete safety receipts on Reset.
- **Geometry projection:** `Geometry/Stroke` contain polylines/colour/aid and a simple mat, but not
  semantic piece IDs, selected-source highlight mapping, occupied areas, ruler-edge metadata or
  material rectangle. Some can be fetched through existing reads, others need a typed projection.
  Do not promise all Show me/drag/tool-motion UI from the present payload alone.
- **Q1 largely works:** force and mirror survive a clean checkpoint, worker recreation and new
  import. Logout only revokes the browser login; it does not erase the person’s draft. Sudden
  crash durability remains the documented checkpoint boundary. Explicit design-session closure
  still needs a named reset contract: `design.close` keeps native session material in RAM, while
  an empty-project checkpoint does not serialize it. Do not let worker eviction decide its life.
- **Basic reuse already works:** `a_test_then_the_main_cut_on_one_mat` and the HTTP practice-cutter
  workflow pass. My earlier suspicion that M3 required a fresh mat after every test was wrong.
  The defects are F05–F07 and the independent test handoff, not absence of reuse altogether.
- **Safety foundation passes:** whole-stream allowlist, transient Ready/setup, buffer/partial-write,
  pause/Stop, frozen execution digest, durable Start/Stop/queue, stale epochs, update drain,
  cancellation, two-worker bound and slow-response isolation have existing passing regressions.
  None of these must be weakened to fix usability. This audit adds no hardware acceptance claim.

## Recommended implementation order

1. **Close the design review and preserve decisions.** In particular, household catalogs are an
   already approved decision; the version-2 draft was wrong to revert them to per-person lists.
2. **M2 completion slice:** safe typed Issues/Info/settings/geometry projection; supported-format,
   source-update/font/draft routes; project units/snap, session Undo/provenance and catalog unification.
   Define schema migration now, while full migration/backup acceptance remains M7.
3. **Persistence versus execution slice:** save unfinished projects independently of cuttable
   copies; add independent test handoff and correct completion/reset association. Preserve receipt
   durability and original-key replay tests.
4. **M3 test-cut integration slice:** one placement/reservation authority, explicit idle-copy
   replacement, same-person retained-load fence, final admission checks and invalidations.
   Require the independent safety reviews and make the failing probes permanent regressions.
5. **Resume M4 in complete workflow slices**, proving the actual browser routes against the
   conformance matrix and F/W acceptance cases. M5/M6/packaging/hardware gates remain explicit.

The imported engine does not need to be discarded. The review shows where its capabilities were
lost or incompletely adapted at the new service boundary.
