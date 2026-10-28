# Original specification audit

Reference: CutCraft **1.0.5, 39654a9465ecdef7478703ee586276304dbc29c6**,
`docs/spec.md` (1,495 lines), final manual, UI/controller/import/planner implementation and
all 162 reachable commits including merges. Web comparison: specification version 1 at
`f17659f` (same spec as `ab6c566`), also inspected as served by M4 `b6a3bf6`.

This is a **design and regression audit**, not a claim to have rerun every historical commit
or validated every hardware hypothesis. Commit messages, spec diffs, changed behaviour/tests,
and final implementation were used to recover intent. Detailed code inspection focused on
superseded rules and UI/import/cutting changes; old build-only changes are recorded as such.
No old Git history is imported. The ledger preserves decisions without requiring the retired repo.

## Coverage comparison

“Explicit” means web-v1 states the rule; “partial” means broad mention but insufficient detail;
“missing” means relying on the absent `docs/cutcraft-ui.md` or unstated inheritance. These are
specification coverage labels, **not implementation or test-pass claims**. Requirement IDs refer
to [the new detailed contract](../cutcraft-ui.md); adaptations/exclusions to
[the exceptions register](../layout-exceptions.md).

| Original specification area | Web-v1 coverage | Disposition in revised specification |
|---|---|---|
| §1 cutting tool, no design editor/plugin dependency, novice workflow | Partial §§1,4 | BASE-01–05; preserve source, explicit uncertainty, self-contained repeats |
| §1 offline/no-account | Partial §§2,17 | BASE-04; local service/open mode; A18 explicitly excludes disconnected-browser operation |
| §2 model, platforms, development vs hardware acceptance | Explicit §§3,21–22 | Server/browser target replaces native deployment; CUT-04,08–09; A04,A11; D02 |
| §3 vocabulary, inches/mm/fractions/rounding, main buttons | Partial §§4,8 | BASE-05, UI-03–06; units parse below; CUT-01–03 |
| §4 one-sheet workflow, all rows ticked, successive colours | Missing detail | BASE-03, PANEL-04, SAVE-02; reject historic sheet-per-colour model |
| §4.1 menus, toolbar, first Save, title/path, dialogs | Missing inventory | UI-01–07, SAVE-01; A01,A06,A11,A12 |
| §4.1 linked comparison/overlay/Show me/exclusions/region rendering | Partial §§9,12 | VIEW-01–02, UI-08; exact shared coordinate transform |
| §4.1 stationary mat, automatic entry fit, size controls/handles/snap | Partial §§9,12 | VIEW-03–04; 1.0.4 cuts disable resizing; no arbitrary planner clamping |
| §4.1 Issues/Info, ten shared groups, reorder/scroll locks/reset | Missing detail | PANEL-01–03; A17 distinguishes fold persistence scope |
| §4.1 picker/readiness/Cut navigation and shortcuts | Partial §§9,14 | UI-03,05; CUT-01–05; cutter discovery must not move |
| §5.1 supported vector/picture/project formats | Missing enumeration | OPEN-01,07–08; D03–04; `.vectorcraft` is not `.vc` |
| §5.1 warnings, preview-only, layer/page identity, immutable source | Partial §§5,10 | OPEN-02,05–06, PREP-01 |
| §5.1 DXF units, lineweights, blocks/limits/curves/layers/nonplanar | Missing | OPEN-07; explicit unit choices/sizes, source rendering, round-trip tests |
| §5.1 AI appearance/editing data and omitted text | Missing | OPEN-05; PREP-04; no filename-based assumptions |
| §5.1 fonts/embedded outlines/substitution/style/glyphs/symbols | Partial §10 | OPEN-10, PREP-04; A03; lost text is never silent success |
| §5.1 page selection, off-page art/SVG exception | Missing | OPEN-06; retain 0.5 mm crossing rule and explicit Trim alternative |
| §5.1 watch/reload, linked assets, fingerprints, remote references | Partial §§6,10 | OPEN-09, SAVE-03; A02; upload ownership, no filesystem watch |
| §5.1 visibility, hidden/guides/nonprinting vs locked art | Missing | OPEN-09, PREP-04 |
| §5.1 extension case, sidecars, alternate formats, Studio/Cricut guidance | Missing | OPEN-01; D03 |
| §5.1.1 physical scale, SVG 96/points72, picture DPI/default/minimum | Missing | OPEN-03–04; original vector ambiguity vs automatic picture size distinguished |
| §5.1.1 linked whole-design size, rotation, no screen calibration claim | Partial §9 | VIEW-03–04, PLAN-04; every format needs scale fixtures |
| §5.1.2 trace modes/controls, low resolution, recipe, photo guidance | Missing detail | PREP-01–03; settings required even if old UI did not expose every one |
| §5.1.2 white art/transparency, colour count, frame/composite policy | Missing | PREP-02–03; OPEN-08; final code alpha 250 supersedes stale alpha 128 prose |
| §5.1.2 embedded pictures/later formats | Missing exclusions | PREP-02; D04; standalone tracing is required, not “later” |
| §5.2 deterministic recipe/replay/Undo/cancel/provenance | Partial §§5,7,10 | PREP-01,04–08; retain prior result on failures |
| §5.2 steps 1–2 symbols/text/effects/brushes | Missing detail | PREP-04; explicitly block unsupported conversions rather than claim expansion |
| §5.2 step 3 stroke roles, fill+outline, per-object/colour/design overrides | Missing | PREP-05; PANEL-04; effective widths/multiple paints/dashes |
| §5.2 steps 4–5 clipping/duplicates/holes/invisible geometry | Missing | PREP-06–08, PLAN-04 |
| §5.2 step 6 required background answer/no white heuristic | Missing | PREP-07 |
| §5.2 step 7 visibility subtraction | Missing scope | D05; explicit exclusions never imply occlusion changes |
| §5.2 step 8 same-layer/colour/operation unions, text weld, pieces | Missing | PREP-06–08; preserve provenance and intentional repeats |
| §5.2 gradient/pattern/opacity/blend policies | Missing | PREP-08; D05 |
| §5.3 Colour/Layer/spot identity/merge choices | Partial §9 | PANEL-04; explicit identity, no implicit visual merging |
| §5.3 one sheet/group changes/inclusion/scope/assembly positions | Missing detail | BASE-03, VIEW-02, PANEL-04; D01,D06 |
| §5.4 mat/material/device bounds and all preview content | Partial §§9,12 | VIEW-03–04, MAT-06, PLAN-01–06 |
| §5.4 warnings/fixes vs blockers/current-plan revalidation | Partial §§13–14 | PANEL-01, PLAN-01–02, CUT-01 |
| §5.4 immutable preview/send plan, tool-motion overlay | Partial §§12,14 | PLAN-01–06; do not confuse blade-holder paths with contours |
| §5.4 weeding border/lines/invert/marks/defaults/mirror | Missing | PLAN-03; D05 overlap remains deferred |
| §5.5 load/Start/Pause/Stop/done/interruption/progress | Partial §14 | CUT-01–09; A04,A05,A08,A14 |
| §5.5 inline tuning, session lifetime, explicit saves/defaults | Partial §§8–9 | MAT-01–07; A13/Q1 resolve lifetime; defaults latest placement retained |
| §5.5 no tested tracking/wizard, optional pattern, Troubleshooting | Missing | MAT-03, CUT-02,07, HELP-01; X01 |
| §5.5 one owner, discovery, unsupported model, double starts | Explicit architecture, missing model detail | CUT-04,06,08–09; server retains authority |
| §5.5 state machine/draining/completion/close/sleep/frozen plan | Partial §14 | CUT-05–09; browser disconnect is not USB interruption; A05,A14 |
| §5.5 unload/raw cutter export | Missing boundary | D07; HELP-02, no arbitrary raw execution |
| §5.6 presets, values/ranges, loading/mirror/roll/manual blade | Partial §9 | MAT-01,04,06; actual reference values retained, no invented certification |
| §5.6 material CRUD/offset/pen workflow | Partial §§8–9 | MAT-05–06, PLAN-05; D07 |
| §5.7 self-contained snapshot, repeat source/fonts missing | Partial §§11,16,20 | SAVE-01–02, PLAN-01; generated plan revalidation without silent reprepare |
| §5.7 source-update matching/ambiguous rows/object choices | Partial §10 | SAVE-03; D06 object-level matching remains deferred |
| §5.7 copied presets/atomic save/newer schema/recent/dirty close | Partial §§7,11,16 | SAVE-01–03, MAT-02, UI-04; web migrations required, old “later” no longer applies |
| §5.8 errors/recovery/reports/USB failures | Partial §§13–14 | UI-07, CUT-04,06, HELP-02; no unsafe generic Retry |
| §5.9 font size/progress/cancel/responsiveness/accessibility | Partial §§8,18–19 | UI-05–08, OPEN-02; web timing targets override native host targets; D09 |
| §5.10 live MCP/help/privacy/capture | Explicit endpoint, missing interaction detail | AUTO-01–03; A09–10; no hidden independent project |
| §6 domain contracts/dependency boundaries/stable diagnostics | Explicit §5, partial contracts | OPEN-02, PREP-01, PLAN-01; web protocol owns browser-safe representations |
| §6.1 pinning/fonts/typed adapter/golden engine tests | Partial §§5,22 | OPEN-10, ACCEPT-01–02; development checks; native font listers replacedA03 |
| §6.2.1 registry/schemas/availability/revision/history/journal | Explicit §§5–7,13 | UI-03,08, AUTO-01; transport cannot replace validated registry |
| §6.2.2 origin parity/authority/AI access/queries/simulator | Partial §§6,14–15 | AUTO-01,03, CUT-04,08; web ownership/admin rules override optional local roots |
| §6.2.3 live/headless/stdio, protocol/errors/annotations/progress | Partial §15 | AUTO-02–03; A09–10; pinned SDK compatibility, no obsolete protocol promise |
| §6.2.4 tools/batches/exports/units/coordinates | Partial §15 | AUTO-01–02, PLAN-01, HELP-02; owned IDs replace paths; explicit capture target |
| §6.2.5 resources/prompts/completions | Missing detail | AUTO-02; D08 only the original later items deferred |
| §6.2.6 control protocol/input/badge/authority/capture | Missing web contract | UI-08, AUTO-01–03; A10; native test harness retained, browser capture not fabricated |
| §6.2.7 roots/source safety/privacy/report exclusions | Explicit ownership but partial reporting | OPEN-09, BASE-02, HELP-02, AUTO-03; A07 |
| §6.2.8 CLI/same-registry batching | Partial §5 | AUTO-01–02; test/developer harness retained, browser capabilities do not require a local CLI install |
| §6.2.9 historical milestones/later MCP | Partial §§15,24 | Web milestones replace old numbers; D08; no automation-motion ban |
| §7.1 engine selection/paint stack/clip/units/provenance/order | Missing detailed contract | PREP-04–08; adapter selects explicit IDs, skips empty selections, retains source; points-to-mm 25.4/72 |
| §7.2 flatten/quantization/offset/overcut/order/geometry | Missing detail | PLAN-04–06; no double compensation, no lost holes |
| §7.3 clean-room protocol/model/status/stream validation/timing | Partial imported driver | CUT-08–09, PLAN-05–06; preserve measured-vs-hypothesis boundary; X08 |
| §7.3 preflight/partial transfers/queue limits/USB3/setup | Partial §14 | CUT-04,06,08–09; tested transport/cable acceptance still needed on server |
| §7.4 simulator parser/buffer/state/faults/virtual mat/clock | Partial §§14,22 | CUT-04,08–09; ACCEPT-01; simulation never proves hardware accuracy or stop time |
| §8 packaging/release evidence/fonts/licences/toolchain | Explicit §§5,21–22 + development | A11,A16; new packaging replaces desktop installers; licence/build checks remain |
| §9 golden/preparation/geometry/protocol/fault/hardware tests | Partial §22 | Acceptance catalogue imports specific cases; reference-only unit success insufficient |
| §9.1 all 15 acceptance cases | Partial §22 | Acceptance catalogue W01–W24 and F01–F22; retain physical acceptance separately |
| §9.2 private corpus/manifest/geometry/reference selection | Explicit §§5,19,22 | Authored public fixtures; local private run with CUTCRAFT_CORPUS_DIR, never commit private outputs |
| §10 exclusions/later tags | Missing inherited inventory | D01–D09, X01–X09; corrected contradictions below |
| §11 milestones | Explicit §24 | Web milestones authoritative; M4 requires detailed inventory/evidence, not a placeholder page |
| §12 open questions | Different web decisions | Old machine/library questions historical; confirmed model/unsupported formats retained; web §23 names confirmed port decisions and remaining measurement questions |
| §13 revision history/research observations | Not requirements themselves | Ledger records progression; no private research/corpus identities or old history copied |

## Contradictions resolved instead of copied

1. Original §5.4 says zoom/pan on the mat; later §4.1/24814c9/88fd158 make it stationary
   and resize the design. 1.0.4 disables those actions on cut/test screens. VIEW-03 wins.
2. Original §5.4 still warns about materials awaiting testing; 0b138cb removed the entire
   status/wizard model. MAT-03 wins. Its Test cut sketch/button is not an extra entry point.
3. Original §5.2 mentions a toolbar Make cut-ready button; final toolbar replaces step screens.
   Keep Run again/Clear answers inside Issues, not another top-level workflow step.
4. Original §10 still excludes automated motion and calls tracing “later”. The explicit later
   decisions enabled authorised automation and standalone tracing. AUTO-01/PREP-02 win.
5. Original §6.2.3 says no token/no motion; later named keys and motion parity superseded that,
   and web authority is stricter still. AUTO-01–03 and web §§6/15 win.
6. Original colour paragraph says alpha 128, but the later white-PSD fix uses alpha 250 for exact
   colours while flatness still uses 128. PREP-03 records both actual tests.
7. Original §9.1 forbids silent resizing; picture defaults and later Cut Preview entry fit are
   intentional exceptions with Info/Undo. No sender or planner resize is allowed.
8. Old issue-example README asks picture sizes; final examples use OPEN-04. Update it here.
9. Old manual says Stop finishes in about a second; measured overrun and uncertain transfers do
   not support that promise. CUT-06/09 require honest state and separate physical acceptance.
10. Old §10/11 suggests validated roll mode ships; actual production roll remains gated. D07 wins.
11. Old §5.7 defers schema migration; web §20 now requires migration/recovery. SAVE-02 applies.
12. Late defaults in library windows override earlier duplicate panel checkboxes; selecting defaults
   for a new import is distinct from preserving a saved project's own snapshot. MAT-02/07 apply.

## Specified capabilities not proven by the final UI

The old prose contains promises broader than its visible 1.0.5 controls: full trace sliders,
per-step Advanced toggles/keeping script pieces separate, near-colour merge, some per-object
preparation alternatives, complete clipped-centreline support, and diagnostic/export/UI-command
coverage. Carry their intent forward (PREP/PANEL/AUTO); do not claim all were implemented or tested.
Implement the supported safe choices; explicitly report/refuse unsupported geometry. Before M4
is signed off, each specified user capability must have a demonstrated control and acceptance case
or a developer-approved scope amendment. This proposal does **not** silently promote every
experimental engine feature to supported conversion.

## Units and source safety details applying throughout

UI lengths accept explicit mm/cm/in and fractions (including mixed numbers) independently of the
chosen display unit; default display inches/two decimals, precise values retained internally.
Parameters crossing the browser use canonical mm and explicit coordinate spaces. Mat x is across,
y is feed from the cut start corner; displayed ruler zero may differ. Assembly coordinates are
separate. Never round stored geometry because a label rounds. Read-only inspection never claims
cut readiness, and every source write/export path is guarded against overwriting the input.
