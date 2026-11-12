# Independent M4 implementation

This branch is **in progress**, not an M4 completion or merge approval. It starts at M3
`b9ea099` and carries the revised design/parity documents from `ffc8821`.
The reference remains CutCraft 1.0.5 (`39654a9465ecdef7478703ee586276304dbc29c6`).

## Working foundation

The server bundles an egui/WebAssembly desktop at `/desktop`, with the reference vector
icons and panel headers. Both desktop and existing station pages use the service's shared
project and cutter state. Desktop edits carry the session incarnation and revision;
Stop bypasses the browser mutation queue.

The workspace includes the menu/toolbar, three views and ten panel groups, readable
preparation choices, material/mat libraries and editors, source rendering, local vector
drawing, media/design/test positioning, media/design resizing, material adjustment,
Save/Save As, download/export, library folders, and separate cut/test screens.

Saving no longer depends on cut readiness. Embedded project sources are rebound into the
owner's private area without rewriting uploaded bytes. Undo/Redo survive worker recovery.
A test-only frozen copy can be prepared when the artwork has unresolved questions; it
cannot be used for a main cut. Preview and execution share test-placement planning.

## Repeatable checks

From the pinned `nix-shell`:

```sh
scripts/build-desktop.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --locked -p cutcraft-ui --target wasm32-unknown-unknown -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
scripts/desktop-browser-check.sh
```

The browser check uses authored artwork and a fresh practice-only service. It exercises
typed dimensions, material force, first Save opening Save As, clean view navigation, drag
resizing/Undo, explicit test preparation, two test cuts followed by a design cut on the
same loaded mat, and reload without restarting motion. It captures 1280×800 and
1920×1080 views in its scratch directory. This is interaction evidence, **not** the full
paired reference screenshot review.

## Remaining completion gates

These are required work, not exclusions or waived acceptance:

- Finish advanced preparation controls and isolated font management; exercise all authored
  regression fixtures and Show me cases.
- Complete region-at-zoom artwork rendering, rulers, gesture/settling parity, occupied test
  overlays, and cut-order playback fidelity.
- Finish cutter recovery/finished states and preparation replacement after settings edits;
  verify actor changes, unknown Start outcomes, reload during cuts, and immediate Stop.
- Complete picker/save conflicts/default editing, account integration, error presentation,
  keyboard accessibility and large-font/narrow-desktop layouts.
- Verify settings scope, new-design defaults, preset adoption, two-device updates and
  two-person isolation through browser interactions.
- Finish asset versioning/compression, command parity inventory, dependency policy,
  compact-layout regression checks and clean-checkout packaging.
- Paired reference screenshots and the complete acceptance catalogue; independent review
  of exact commits. Pi performance, Safari, hardware and release gates remain unverified.

Do not start M5 or merge this branch as a completed M4 while these requirements remain.
No real cutter has been moved for this implementation.

## Foundation checkpoint evidence (2026-10-29)

On the development workstation, version 0.14.0 passed formatting, native workspace
Clippy, browser-target Clippy for UI/protocol, all **449 Rust tests**, the workspace build,
`cargo deny check`, **40 existing compact-browser assertions**, and the desktop workflow
script above. The two history/upload-preservation regressions pass as part of that suite.
Checks used only authored fixtures and the simulator. The remaining gates above are still open.

## Preview ownership follow-up

Private cut-copy previews now require their owner's authenticated stable principal. An HTTP
regression queues an authored design as one account and checks that a second account receives
404 with no geometry. Browser transport regressions cover waiting behind someone else's turn,
a refused preview without losing the workspace connection, discarding a previous session's
preview, and avoiding replay of an old Save result into a later dialog.

Version 0.14.1 passed formatting, native and wasm Clippy, all 450 Rust tests, workspace
build, dependency policy, the desktop simulated workflow and 40 compact-browser assertions.
The transport regression script also passed after adding the session-change case. These checks
ran locally against authored fixtures; they do not close the remaining M4 gates above.

## Prepared cut recovery

The desktop can recover the current owned cut copy through the read-only preview response.
Reloading before loading or between successful tests does not enqueue another turn or start
motion. Cutter selection is a local display preference; Start still uses the current server
boot, turn, revision, mat generation and action epoch. Waiting copies show their queue state.
The desktop browser workflow now reloads both before loading and after the first test and
checks that the turn and occupied test area survive.

This does not yet close recovery acceptance: active-motion reload, unknown Start outcomes,
Stop/account changes, and replacing a prepared copy after edits still need their complete
implementation and failure tests. Replacement must compare source/session identity as well as
project revision; a revision number alone is not an identity for a different opened design.

Version 0.14.2 also passed formatting, native/wasm Clippy, all 450 Rust tests, workspace
build and dependency policy. The expanded desktop workflow passed both reload points and
continued through two tests and a design cut on the same simulated mat. Transport tests cover
read-only recovery and dropping an in-flight preview after switching cutters. Compact-browser
checks last passed at 0.14.1; the recovery change does not modify the compact client.

## Comparison canvas follow-up (2026-11-04)

Addressed Zulip #126: the checkerboard now uses its pane's paint clip, so panning cannot
paint across the comparison divider. The regression inspects actual canvas paint clips in
both pan directions; it fails when the old whole-canvas clip is restored. Comparison views
now pan with the wheel (Shift for horizontal motion), support middle-button drag, and anchor
modifier-wheel/pinch zoom to the pointer, including zoom limits. The panned browser capture
was inspected visually. These display gestures do not change project state.

This closes the reported pane bleed and the pointer-anchor issue. Region rendering, full
reference gesture acceptance, rulers, settling overlays and playback remain open as listed above.

Version 0.14.3 passed formatting, native/wasm Clippy, all 452 Rust tests, workspace build,
dependency policy, transport regressions and the expanded desktop browser workflow (including
pan/Fit, reload recovery and simulated cuts). No hardware or Safari acceptance is claimed.

## Retained-load account identity (0.14.4)

The cutter now binds retained test areas and manual fresh-mat confirmations to the authenticated
stable account id. A second browser for that account can continue; another account or a legacy
record without identity must load fresh material. A new physical load resets that binding.
The service rechecks that the account still exists and is allowed to act immediately before
releasing a staged Start, under the account lock. Deleted/recreated names and transition out of
open mode do not inherit authority. HTTP queue tests verify the authenticated id reaches the
cutter; core tests cover same-account reuse, another account, legacy identity, manual confirmation
and fresh loading. Account tests cover removal/recreation and forced password changes.

Formatting, native/wasm Clippy, all 456 Rust tests, workspace build, dependency policy,
transport regressions, the desktop simulated workflow and 40 compact-browser assertions passed.
This addresses the account-identity portion of retained-load reuse; compatible prepared-copy
replacement and its failure tests remain required. No physical cutter was used.

## Reviewed replacement after test adjustments (0.15.0)

An owned idle turn can now replace its immutable copy through one service operation, used by
the desktop and MCP `cut_replace`. It preserves the turn, waiting neighbours and occupied test
areas. Boot, epoch, mat generation, previous copy and principal are checked atomically before
publication. Changed physical material/mat setup requires fresh loading. Successful receipts
are persisted and replay after restart without restoring physical load authority. Start still
validates the reviewed kind and occupied-area clearance independently.

The desktop detects outdated copies by source hash, incarnation and project revision; Start
waits for explicit Prepare updated cut. Protocol 7 prevents older clients silently interpreting
the new identity contract. The browser workflow now changes force after a successful test,
prepares again without unloading or duplicating its queue turn, and runs the next test and
main cut. HTTP and MCP share a replay test; incompatible physical setup changes nothing.

All 459 Rust tests, formatting, native/wasm Clippy, build, dependency policy, desktop/compact
browser checks and the real-process design-session script passed. Further M4 work remains,
including active-motion reload, uncertain outcomes, modal Stop, full UI parity and acceptance.

## Desktop interaction recovery (0.15.1)

Escape and a visible Stop remain available through modal dialogs and pending requests. Text
editing keeps its own Undo; Save As, Redo and view shortcuts use the same availability as menus.
Cutter selection is synchronized and cannot change during motion. Completed/removed turns release
the old prepared preview; queued copies and replacement previews survive late responses. The
simulator's Load button waits for confirmed unloading rather than the first no-media reading.

All 462 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The desktop browser workflow prepares another test after the design cut and stops it through
Troubleshooting with Escape; its durable execution receipt confirms `stopped`. Transport tests
cover completion cleanup and a late refused preview after replacement. No physical motion occurred.

That exercise exposed a separate inherited gap: a stopped test can leave the mat eligible for
reuse. The spec requires fresh loading after Stop; this remains open until the cutter-state fix
and its tests land. Active-cut preview recovery and the other completion gates above remain open.

## Stop ends retained-load reuse (0.15.2)

The cutter now retires the observed mat generation after Stop, including a driver completion
that races the request. A fresh request with the current epoch is refused until an observed
unload/load creates a new generation. Occupied test areas remain recorded. A core regression
first failed on the old implementation, then passed for both stopped and racing-completion
outcomes and verified fresh loading restores Start. The desktop no longer calls a used mat a
successful completed cut or offers another test based on its occupied-area count.

All 463 Rust tests, native/wasm Clippy, formatting, workspace build and dependency policy passed.
The desktop browser checks confirm the durable stopped outcome and used mat; all 40 compact
browser checks passed. This closes the retained-load Stop gap noted above, not the remaining M4
completion gates. No real cutter was used.

## Active preview and occupied test areas (0.16.0)

A reload during motion now recovers the owned execution's frozen geometry without queueing or
starting anything. The service uses the active job/kind, snapshots its occupied areas and rejects
an intervening cutter-state change. The material outline comes from that immutable copy, never
from edits in the preparation draft. A missing preview does not substitute draft cut lines.
Earlier test areas are solid, numbered overlays on the matching mat; hiding them does not free
space. The authored-fixture screenshot was inspected.

All 465 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The desktop workflow reloads during an active test, verifies the execution/copy identity, then
stops through Troubleshooting and checks the durable outcome and fresh-load requirement.
The compact browser suite also passed. This is simulator/workstation evidence; full reference
parity, unknown outcomes, platform/hardware acceptance and the other M4 gates remain outstanding.

## Reference mat rulers (0.17.0)

The physical mat drawing now reuses the 1.0.5 ruler/grid renderer, adapted to shared geometry.
It includes the outer edge, per-edge inch/centimetre/none rulers, the printed grid zero and the
approximate-measurement note. Both preparation geometry and frozen cut geometry carry the saved
profile. The printed zero remains separate from the cutter's start corner. Custom mixed-ruler
HTTP and paint-level regressions pass; the browser mat screenshot was inspected visually.

All 467 Rust tests, formatting, native/wasm Clippy, build, dependency policy and the desktop
simulated workflow passed. Compact browser checks last passed at 0.16.0; its rendering is unchanged.
This does not close the complete mat/view contract: feed/start annotations, gestures, settling,
progress drawing and paired reference acceptance remain to be completed.

## Artwork retention and identity boundaries (0.17.1)

Same-source artwork remains visible in its original region while a replacement render loads.
Original-source identity is distinct from the preparation fingerprint. Source, session and
account changes clear private artwork, lists and interaction drafts; queued actions cannot cross
those boundaries. Size choices use the selected units, selected controls have readable text and
single pieces/lines use singular labels.

All 471 Rust tests, formatting, native/wasm Clippy, build and dependency policy passed. All 22
authored fixtures pass open/answer/Undo/Redo/Save, and both desktop and compact browser suites
passed. The PDF fixture assertion was corrected: before its first page choice there is no image
to retain. A separate worker regression verifies page changes retain original-source identity.
Transport regressions cover delayed renders, account changes, authentication loss and queued
private actions. This closes these findings, not the remaining M4 gates or platform acceptance.

## Responsive reference toolbar and text scaling (0.17.2)

The toolbar uses the reference width/font layout rules, with a compact view selector and More
for hidden Save/history/zoom controls. Cut and Test Cut remain on the row. All text styles scale
from the default each frame; long project titles truncate rather than widening the menu.
Paint tests cover 600/640/800/1024/1280 pixels at 10/14/24 points, including a long title.

All 472 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The full desktop simulated workflow passed, including actual clicks on the compact view selector,
Fit and Save at 600 pixels/24 points. Its authored screenshot was inspected. Compact browser and
22-fixture suites last passed at 0.17.1; this change is desktop presentation only. Narrow desktop
controls do not replace the separate M5 phone layout or establish full M4 acceptance.

## Visible-region artwork rendering (0.18.0)

Comparison views request the visible source region at display pixel density, with the reference
quarter-view margin/grid, 120 ms settling delay and 4096-pixel cap. One render runs at a time;
intervening viewport requests coalesce. Cached images retain their original region until the
latest valid render arrives. Late viewport/account/session results cannot replace them.

All 474 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
All 22 authored fixture workflows and the full desktop browser workflow passed, including real
Zoom/Fit input producing a sharper region without clearing artwork. Unit/transport tests cover
resize scale, pixel density, pan, settling, bounded dimensions, texture replacement and late
cross-account replies. Compact layout was unchanged (last browser pass 0.17.1). Full Show me,
paired reference captures and the other M4 gates remain open.

## Mat orientation and editor diagram (0.18.1)

Mat previews mark the actual cutter start corner independently of ruler zero and indicate the
edge loaded into the cutter. The mat editor draws the draft outer edge, sticky area, mixed rulers
and start corner. Invalid in-progress dimensions do not enter drawing loops. Material/mat editors
use more of the available height; the final dialog screenshot was inspected with measurements
visible below the diagram.

All 476 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The focused desktop presentation workflow passes (mat editor, source-region zoom, compact toolbar
at 600 pixels/24 points). Full cut workflow and 22-fixture suite last passed at 0.18.0. No hardware
was used. Review findings #141–142 remain open: fresh-checkout assets, unprepared cut presentation,
active-test area identity and repeated active preview work. These precede further gesture polish.

## Active/unprepared preview review fixes (0.18.2)

Addresses review #142: idle unprepared cut screens show the draft mat/material with an explicit
Not prepared label. Active or already reviewed cuts never fall back to editable material/geometry.
The active test location is read from the running execution, rather than chosen again; a regression
compares every preview extent with that recorded area. Repeated active updates reuse geometry
for the same execution/copy, and a new execution fetches a fresh preview.

The dependency audit newly flagged rmcp 3.5.1 (RUSTSEC-2026-0457, -0461 and -0462). The exact pin is
now 3.9.0; no advisory exceptions were added. All 477 Rust tests, formatting, native/wasm Clippy,
workspace build and dependency policy pass. Full desktop simulation and compact browser checks
pass; the latter was rerun after an interrupted process. The unprepared-cut screenshot was
inspected. Review #141's clean-checkout asset issue and the other M4 gates remain open.

## Clean native checkout and desktop bundle gate (0.18.3)

Addresses review #141: the four native Cargo checks run from a source-only copy with no generated
desktop directory. Native-only development binaries return an explicit HTTP 503 with build
instructions for desktop routes; they never pretend to serve a working desktop. A build script
requires generated assets stamped with the current app version for release builds. The desktop
build script writes that stamp; README distinguishes native checks from a runnable desktop build.

All 478 tests, formatting, native Clippy and workspace build passed in the source-only copy and
with the bundle present. Wasm Clippy, dependency policy and the focused desktop browser workflow
also passed with the bundle. The build-script matrix tests missing/wrong-version/current bundles
in development and release profiles, and HTTP tests cover both configurations. This is not a
release-package acceptance claim; source/content hashing, compression, platform checks and the
other M4 acceptance gates remain open. Full cutting/compact browser workflows last passed 0.18.2.

## Media handles and printed-grid snapping (0.18.4)

Media selection has eight handles, with amber selection distinct from the blue design. Edge
handles change only their axis; resizing keeps the opposite edge fixed even when snapped. Moving
media preserves its dimensions. Move snapping uses the mat's printed ruler zero and the actual
selection bounds, including designs with a placement offset. Selected dimensions respect inches.

All 480 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The focused browser workflow verifies a real media edge drag and exactly one Undo step. The full
simulation browser workflow also passes, including two tests followed by a design cut on one
loaded mat, active reload, modal Stop and the narrow toolbar at 24 points. The remaining design-corner,
interactive dimension and gesture-settling acceptance items remain open.

Review #145 additionally reports uncertain-Start messaging/availability, Save As dialog clipping,
dark appearance, stopped outcome presentation, remaining-time formatting, menu shortcut hints,
mat-list units and the test-spot default. These are tracked as defects, not intentional omissions.

## Dialog sizing and dark appearance (0.18.5)

Addresses the Save As clipping and theme findings in review #145. Each dialog kind has its own
layout identity, so a short conflict dialog cannot constrain the following Save As. Conflict
wording also covers another person saving the project. The selected theme is applied to both
the root widget and popup styles rather than overwritten by the previous frame's style.

All 481 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy passed.
The imported independent browser harness passes its save-conflict subset: refusing a stale save
without losing edits, Save As to a separate project, and Reload of the newest revision. The
Save As screenshot was inspected with both action buttons inside the padded dialog. The focused
presentation browser workflow also passes. Its uncertain-Start cases were explicitly skipped;
those findings remain open. Full simulated cutting last passed at 0.18.4.

## Unconfirmed Start recovery (0.18.6)

Addresses the Start findings in review #145. Before sending Start, the desktop remembers its
exact request and key in account-scoped browser session storage. An unknown outcome disables
new Start and survives reload. Check Start status only reads; Retry original Start first looks
up the result and reuses the original body/key only on a deliberate press. A failed lookup sends
nothing. Known acceptance/refusal clears recovery. Unavailable recovery storage prevents sending;
account changes do not inherit or resend another account's request. Stop remains independent.

All 481 Rust tests, formatting, native/wasm Clippy, workspace build and dependency policy pass.
Transport regressions cover persistence, exact retry, blocked new keys, account switches during
lookup and unavailable storage. The full uncertainty/save-conflict browser harness passes:
lost accepted reply, missing request, unavailable reconciliation, reload, deliberate exact retry,
fresh Start plus lookup both lost, and two-person Save As/Reload conflict handling. An initial
browser run found a transient incorrect Connected status; the corrected run passes and the
recovery screenshot was inspected. No hardware was used.

These checks do not close all M4 cutter recovery gates. Late acceptance versus failed Stop,
changes to the reviewed copy/cutter while recovery is open, and the complete stopped/finished
presentation still need their remaining acceptance coverage and independent review.

## Sender-based estimated progress and cut-order playback (0.19.0)

The frozen preview includes optional typed sender pass ranges, quantized points and the model's
units per mm. The cut screen draws estimated completed paths black, remaining paths red, queued
motion orange and an approximate blade marker using the reference display algorithm. The server
regression compares preview ranges/points with the actual encoder output for the recorded active
test area. Older preview payloads remain readable, with the detailed legend omitted when absent.

Startup waits for a real progress estimate instead of displaying zero seconds. Time, percentage,
blade position and legend all identify estimates; Pause shows Pausing until the checkpoint. Mat
cut-order playback advances by physical path length, including repeated passes, rather than by
contour count. The protocol version remains 7: existing request/response fields are unchanged,
and the added motion payload is optional and ignored by older clients.

All 483 Rust tests, formatting, native/wasm Clippy, workspace build, dependency policy and the full
desktop simulated workflow passed. The browser verifies that sender pass metadata reaches the UI;
the startup/progress screenshot was inspected. Unit paint tests exercise estimated completed and
remaining paths, the queued path and marker. Terminal outcome retention and the other acceptance
items remain open. No physical cutter was used.
