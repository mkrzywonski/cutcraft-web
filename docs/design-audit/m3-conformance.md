# M1–M3 requirement matrix

Code: `b9ea0990d2d950aa3725124561a2e3ab109c26c9` (0.13.1).
Contract: [detailed specification](../cutcraft-ui.md) and [web specification](../spec.md).
Findings, reproduction and scope: [review](m3-review.md).

**Implemented** means the named backend behaviour is present with passing relevant tests; it does
not certify an entire browser workflow. **Missing** means a required path/state is absent.
**Conflicting** means present behaviour contradicts the contract. **Unverified** names acceptance
not established here, often a later milestone. A row can identify a working backend and still be
Missing at the service boundary. F-numbers refer to review findings; W/F acceptance-case IDs are
in [the regression catalogue](acceptance.md). No row waives later browser acceptance.

Evidence abbreviations:

- **I:** `crates/import/src/{inspect,prepare,picture,dxf,source}.rs` and `tests/inspection.rs`;
  baseline suite plus all 21 authored initial reference outcomes.
- **C:** `crates/control/src/{lib,commands,history,settings}.rs`; shared controller regressions.
- **P:** `crates/{job,path,device,sim}`; project, planner, encoder and virtual-driver regressions.
- **W:** `apps/server/src/design/{mod,worker,ipc}.rs` and `crates/protocol/src/lib.rs`;
  worker tests and `scripts/design-check.sh` with real processes.
- **S:** `apps/server/src/{service,copies,queues,tests}.rs`, `crates/cameo-pi-core`;
  HTTP/service/state-machine tests including durable Start/Stop/queue and update drain.

## All detailed requirement IDs

| Requirement | Status at M3 | Code/test evidence and remaining boundary |
|---|---|---|
| BASE-01 | Implemented, scoped | Imported engine/controller retains final workflow; I/C/P baseline passes. Detailed omissions below; not a blanket parity certification. |
| BASE-02 | Implemented, scoped | C roots/source-write guards and project-file tests; no design editor added. F01/F10 still block complete saving/update workflow. |
| BASE-03 | Implemented, backend | `Project::new`, project migration and rows/grouping tests preserve one sheet and inclusion. Browser colour workflow remains W07. |
| BASE-04 | Implemented, backend | Local HTTP/practice/process checks require no cloud. Complete offline bundled help/client assets still M4/M6 acceptance. |
| BASE-05 | Unverified, M4/M5 | User-facing wording/technical disclosure requires completed layouts; F02 must supply safe structured data. |
| OPEN-01 | Conflicting | F03: restricted upload list and no uploaded-project dispatch. Backend format support exists in I. |
| OPEN-02 | Missing, adapter | W cancellation/stale guards and source fingerprints pass; F02 hides required reports/choices. |
| OPEN-03 | Implemented, backend | I pixel/point/viewBox/physical-unit/Unicode/DTD/CSS/ambiguous-size tests; F01 initial size decision reproduced. Web answer controls await F02. |
| OPEN-04 | Implemented, backend | I automatic sizes, minimum/oversized picture, resolution and low-resolution tests; F17–19 outcomes match. W03 browser resize still required. |
| OPEN-05 | Implemented, backend | I PDF/AI page/native-editing/font/appearance diagnostics; F16 initial page question matches. Complete chosen-page source comparison not re-proven here. |
| OPEN-06 | Implemented, backend | I off-page fit/trim logic and tests; F07 initial outcome matches. Geometry alignment and upload-update route remain F10/W04/W08. |
| OPEN-07 | Implemented, backend | I DXF unit, layer, block, curve, complexity/nonplanar/binary tests; C DXF round-trip. Source update delivery missing (F10). |
| OPEN-08 | Missing, adapter | I EPS/PSD source/trace support and tests exist; PSD/PSB upload rejected (F03). |
| OPEN-09 | Implemented, scoped | Owned-ID/roots/refusal tests and linked fixture blocker retained. No new bundle resolver promised; broad hostile-input/security testing not repeated. |
| OPEN-10 | Missing, adapter | I font substitution/glyph/embedded-outline safeguards pass; owned font upload/registration missing (F10). Cross-person uploaded-font acceptance unverified. |
| PREP-01 | Conflicting | C recipe replay/failed-answer/Undo passes in one live worker; W loses history on eviction (F04). Per-step Advanced UI and public choices remain gaps. |
| PREP-02 | Missing, adapter | `TraceSettings` has colours/threshold/detail/smoothing/ignore-white; I trace/frame tests pass. Required controls unavailable without F02/M4; GIF upload missing. |
| PREP-03 | Implemented, backend | I exact-colour/alpha/flatness/light-art tests pass; F17/18 photo warning reproduced. Public answer route requires safe report projection. |
| PREP-04 | Implemented, supported backend | I text/path-overflow/multiple-paints/font/instance and unsupported-appearance checks. No claim every experimental expansion is supported. W04/05 still needed. |
| PREP-05 | Implemented, backend | C stroke role overrides/Undo; I thick stroke/dash/paint tests; F12/15/21 match. Public choice/affected-object display missing. |
| PREP-06 | Implemented, supported backend | I clipping/holes/unions/text weld and supported centreline behaviour; unsupported conversions block. Broader per-object UI inventory not accepted here. |
| PREP-07 | Implemented, backend | I exact deduplication/background and near-duplicate questions; F08/F11 match. UI must not fabricate default answers. |
| PREP-08 | Missing, adapter | I small/thin/near-duplicate questions retained, F09–11 match; F02 prevents presenting choices/highlights coherently. |
| UI-01 | Unverified, M4 | Native egui module reuse and visual fidelity are not M3 outcomes. F02/projection changes are prerequisites. |
| UI-02 | Unverified, M4/M5 | Full menu inventory not delivered by M3. Save, update, fonts, reports and test-handoff need server changes first. |
| UI-03 | Unverified, M4 | Toolbar/context sizing/Exit/navigation behaviour requires completed UI; backend handoff alone is not evidence. |
| UI-04 | Missing, adapter | W names/owned upload listing exist; previous-draft reopening absent (F10), complete title/recent route not validated. |
| UI-05 | Unverified, M4/M5 | Keyboard/touch/numeric field behaviours belong to client acceptance. Units storage currently conflicts (F09). |
| UI-06 | Implemented backend / Unverified UI | C font-size validation/persistence tests pass; dialog padding, focus, responsive waits and clipping require browser acceptance. |
| UI-07 | Implemented protocol / Unverified UI | W typed applied/refused/key lookup and error codes pass. M3 does not prove every client surfaces refusals; F02 lacks complete reasons. |
| UI-08 | Missing, projection | W revision/incarnation/SSE/coalescing/revocation tests pass. Snapshot omits full settings/reports; geometry lacks several semantic fields. No full multi-device UI acceptance. |
| VIEW-01 | Unverified, M4 | W art-region identity and P assembly geometry exist; matching pan/zoom/transforms/source render across all formats needs W08, beyond smoke pictures. |
| VIEW-02 | Missing, projection | W filters excluded assembly lines; C row/inclusion commands exist. Semantic source/piece IDs and transient-highlight geometry are incomplete. |
| VIEW-03 | Missing, projection | W exposes outer/sticky/origin and a basic test spot; full material/ruler metadata and execution reservations missing. F05; W09 acceptance pending. |
| VIEW-04 | Implemented commands / Unverified UI | C placement/resize/material/test drag validation exists; local drag/one-Undo/settling depends on M4. F05 affects displayed next test. |
| PANEL-01 | Missing, adapter | C commands cover much of ten-group data; W cannot supply required Issues/Info/current full state coherently (F02). Layout itself is M4. |
| PANEL-02 | Missing, adapter | Empty/blocking design reports zero issues (F02); collapsed attention and all-view controls require M4. |
| PANEL-03 | Conflicting | F09: native per-person folded state has not been split from shared order/locks. C layout validation passes, but scope is wrong for web. |
| PANEL-04 | Implemented, backend | C rows.list/include/group; P single-sheet inclusion/assembly tests. W07 keyboard/touch/hover acceptance not performed. |
| MAT-01 | Implemented, backend | C sheet.adjust ranges, mirror, offset, save refusal and plan invalidation tests; active S copies immutable. F06 prevents tuned test continuation. |
| MAT-02 | Implemented recovery / Missing close contract | New probe preserves force/mirror through checkpoint/recreation/new import. Preset library changes remain explicit. Named session-close reset and full login lifecycle acceptance still needed. |
| MAT-03 | Implemented, backend | C no certification/result-wizard gates; legacy status compatibility retained. M4 must not reintroduce obsolete UI. |
| MAT-04 | Implemented, data | `Material::defaults` retains required vinyl/Siser catalog and Off default. No published starting setting is certified by this review as a physical cut result. |
| MAT-05 | Conflicting | W global catalog concurrency/reload tests pass, but library Adjust reads another catalog (F08). Modal list/editor behaviour remains M4. |
| MAT-06 | Implemented backend / Unverified UI | C material/mat schemas/edit/delete/rename and mat-profile tests; typed metadata delivery and library dialog acceptance not complete. |
| MAT-07 | Implemented backend / Conflicting integration | C defaults/new-design/saved-project precedence tests pass; F08 catalog split and F09 settings scope remain. |
| PLAN-01 | Implemented main-cut / Missing test integration | P deterministic planning; S copy digest survives edits/library updates/close. F05–07 and independent test handoff must use same validation. |
| PLAN-02 | Implemented planner / Conflicting service | P bounds/material/compensation/roll guards pass; service uses wrong test fallback area and bounding collision (F05). |
| PLAN-03 | Implemented planner / Missing reservations | P weed/invert/marks/order tests pass. W does not integrate cutter-owned test areas into preparation previews; F05. |
| PLAN-04 | Implemented, backend | P curve tolerance/determinism/collapsed geometry/device-grid validation tests pass. Exhaustive golden geometry/browser comparisons not performed. |
| PLAN-05 | Implemented, backend | P containment/overcut/software-vs-machine compensation/FC0/stream tests pass. Hardware roll and physical blade calibration remain excluded gates. |
| PLAN-06 | Implemented estimate / Unverified UI | P timing/profile/return-travel data retained. Playback/legend/estimated-progress presentation remains M4, physical accuracy unmeasured here. |
| CUT-01 | Implemented handoff / Unverified navigation | S `cut.copy` creates independent immutable storage and does not queue or start. Toolbar navigation vs handoff/Start must be verified in M4. |
| CUT-02 | Missing, handoff | Main-art readiness is required by `cut.copy`; no independent test-copy route for unresolved artwork. Pattern generation exists; service adaptation needed. |
| CUT-03 | Missing, integration | Native C Exit preservation tests pass; W denies cut.exit/dismiss/reset and has no equivalent full session/execution transition. M4 controls need shared service semantics. |
| CUT-04 | Implemented safety / Unverified presentation | S mat generation/settling/disconnection/simulation and load steps present. All manual loading instructions/cutter-picker states require UI acceptance. F07 actor fence missing. |
| CUT-05 | Missing, integration | S freezes active copies; pause/resume progress exists. F06 idle replacement, full execution geometry/progress projection and M4 screen still needed. |
| CUT-06 | Implemented receipts / Missing project association | S durable Stop/terminal records/restart/no-replay/update tests pass. Saved draft cut-record/reset integration absent; actor rule in F07 still required. |
| CUT-07 | Conflicting | Basic test→main same-mat path passes. F05 placement/reservations, F06 tuned-copy transition and F07 user invalidation fail. |
| CUT-08 | Implemented, tested backend | `gpgl/stream.rs` full allowlist test plus encoder/discovery/driver tests pass. No hardware acceptance performed. |
| CUT-09 | Implemented, simulator | Sim tests cover false Ready/setup/startup/buffer/pause/interruption. No claim of measured physical stopping, USB reliability or current Pi timings. |
| SAVE-01 | Conflicting | Ready-project Save/base-conflict/receipt tests pass, first Save asks name. Unfinished project export fails (F01); user-facing cancellation still M4. |
| SAVE-02 | Conflicting | P self-contained files/unknown schema/source safety tests pass; units/snap absent (F09), unfinished library projects blocked, web cut records not associated. M7 full migration remains pending. |
| SAVE-03 | Missing, adapter | C update/diff/settings/Undo exists; FileRead blocked without owned-update route (F10). |
| HELP-01 | Unverified, M4/M6 | Bundled imported guides exist; headless help action/window-only mechanics need browser adapter. Manual URLs and all links not accepted here. |
| HELP-02 | Missing, web integration | S cut histories/logs exist; complete authorised redacted problem-report download/project-safe export contract not demonstrated. No support upload is added. |
| AUTO-01 | Missing, adapter | C registry/validation/atomic edit batches/Undo available; W hardcodes user origin and blocks journal, no complete batch/discovery projection. S shared safety paths pass. |
| AUTO-02 | Unverified, M6 | Basic M3 HTTP/MCP copy/result parity tests pass; full tools/resources/prompts/progress/capture port is deliberately M6. Do not call a rendered plan a live screenshot. |
| AUTO-03 | Unverified, M6/UI | Imported per-user keys/revocation tests pass; one-service AI settings/instructions/activity UI and full MCP acceptance remain. |
| ACCEPT-01 | Missing, acceptance | 440 existing tests plus reference outcomes do not cover recovered workflow; nine failing review probes establish missing cases. |
| ACCEPT-02 | Unverified, M4 gate | No M4 sign-off from this review; resolve foundation findings and independent reviews first. |

## Web foundation requirements outside the numbered companion

| Requirement | Status | Evidence / follow-up |
|---|---|---|
| Self-contained imported workspace/pins (§5.2) | Implemented, scoped | Locked baseline builds/tests without old repositories; provenance records origins. Desktop harness/corpus tool not yet imported; later acceptance. |
| Principal/owned files/Open-mode adoption (§6) | Implemented, tested routes | HTTP two-person isolation, owned upload, adoption and revoked-stream tests pass; full hostile-font/bundle routes absent, not implicitly safe. |
| Shared material/mat catalog (§8) | Conflicting consumers | Worker list locking/reload passes; Adjust split F08. Spec corrected to retain decision #76. |
| Session key/incarnation/revision/Save receipts (§7) | Implemented, tested | Worker replay/stale/close/Save races and real process script pass; scope/bounded checkpoint durability documented. |
| Undo/desktop screen/provenance/session closure (§7–8) | Missing | F04/F09 and explicit-close contract noted in review. Current draft and accepted keys survive; not the whole session. |
| Resource bounds/cancellation/slow clients (§18) | Implemented, tested locally | Baseline resident-budget/backpressure/fenced-cancellation/slow-body tests pass. Earlier Pi measurements not rerun. |
| Immutable handoff/Start/Stop/queue/drain (§14) | Implemented core, conflicting edges | Durable records/admission/recovery tests pass; F05–07 and test-ready export are required additions. |
| Removed-account retention (§17) | Missing carryover | M2 explicitly left retention integration unfinished; current remove_user revokes account/keys, no complete retained-area management/restore workflow. Not repaired here. |
| Backup/schema migration/private areas/catalog (§20) | Unverified, M7 | Imported library backup is not proof of the new per-person areas/catalog schema. Keep migration acceptance explicit; do not delete old receipts/state. |
| Performance targets/browser/Pi hardware (§19/22) | Unverified here | Historical M3 simulator Pi evidence retained separately; this review supplies no new latency/USB/browser acceptance. |
| Compact/full MCP/packaging (§24) | Later milestones | M5/M6/M8 remain separate; absence is not called an M3 implementation defect, but does not waive shared backend prerequisites. |
